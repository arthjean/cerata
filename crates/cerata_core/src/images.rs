//! Image import (US-059): what a dropped, picked or pasted image becomes
//! before it reaches a note, without GPUI.
//!
//! [`plan_import`] checks every source with the renderer's own rules
//! (format by signature, pixel ceiling, self-contained SVG), links a file
//! already inside the workspace, and names a copy in `{stem}-assets/` for
//! anything else. [`run_import`] performs one planned copy through
//! [`WorkspaceFs`]. Both read or write files: run them off the UI thread.
//!
//! Names follow the OWASP File Upload Cheat Sheet: sanitized, type by
//! signature, never by the declared extension.

use std::collections::HashSet;
use std::io::{Cursor, Read as _};
use std::path::{Component, Path, PathBuf};

use crate::fs::{FsError, WorkspaceFs};

/// GPUI decodes an image to RGBA at its natural size: a header claiming
/// more pixels than this (a decompression bomb) is never shown.
pub const MAX_PIXELS: f32 = 50_000_000.0;
/// Pasted bytes larger than this are refused (Writer's paste limit,
/// `editor-clipboard.ts:9-10`). Files have no byte limit.
pub const MAX_PASTE_BYTES: usize = 5 * 1024 * 1024;
/// A local SVG larger than this is never shown: it is read whole, to be
/// checked before GPUI sees it.
pub const MAX_SVG_BYTES: u64 = 8 * 1024 * 1024;
/// Bytes read to find a raster image's format and size.
const HEADER_BYTES: u64 = 64 * 1024;
/// The longest base name a copy gets, in UTF-8 bytes.
const MAX_NAME_BYTES: usize = 120;

/// The image formats Cerata displays.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ImageKind {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
    Tiff,
    Ico,
    Svg,
}

impl ImageKind {
    /// The extension a file of this format is saved with.
    pub fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Gif => "gif",
            Self::Webp => "webp",
            Self::Bmp => "bmp",
            Self::Tiff => "tiff",
            Self::Ico => "ico",
            Self::Svg => "svg",
        }
    }
}

/// The image format of `bytes`, by signature, SVG included.
pub fn sniff(bytes: &[u8]) -> Option<ImageKind> {
    if is_svg(bytes) {
        return Some(ImageKind::Svg);
    }
    Some(match image::guess_format(bytes).ok()? {
        image::ImageFormat::Png => ImageKind::Png,
        image::ImageFormat::Jpeg => ImageKind::Jpeg,
        image::ImageFormat::WebP => ImageKind::Webp,
        image::ImageFormat::Gif => ImageKind::Gif,
        image::ImageFormat::Bmp => ImageKind::Bmp,
        image::ImageFormat::Tiff => ImageKind::Tiff,
        image::ImageFormat::Ico => ImageKind::Ico,
        _ => return None,
    })
}

/// Whether `bytes` start like an SVG document.
pub fn is_svg(bytes: &[u8]) -> bool {
    let head = &bytes[..bytes.len().min(1024)];
    std::str::from_utf8(head).is_ok_and(|s| s.contains("<svg"))
}

/// Natural size of an image, read from its header without decoding it.
pub fn natural_size(bytes: &[u8]) -> Option<(f32, f32)> {
    if is_svg(bytes) {
        return svg_size(bytes);
    }
    let (w, h) = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()?
        .into_dimensions()
        .ok()?;
    (w > 0 && h > 0).then_some((w as f32, h as f32))
}

/// `width` and `height` of the root `<svg>`, else its `viewBox`.
fn svg_size(bytes: &[u8]) -> Option<(f32, f32)> {
    let text = String::from_utf8_lossy(&bytes[..bytes.len().min(8192)]);
    let start = text.find("<svg")?;
    let tag = &text[start..start + text[start..].find('>')?];
    let attr = |name: &str| -> Option<&str> {
        let key = format!(" {name}=");
        let at = tag.find(&key)? + key.len();
        let quote = tag[at..].chars().next()?;
        let rest = &tag[at + quote.len_utf8()..];
        Some(&rest[..rest.find(quote)?])
    };
    let number = |s: &str| s.trim().trim_end_matches("px").parse::<f32>().ok();
    if let (Some(w), Some(h)) = (
        attr("width").and_then(number),
        attr("height").and_then(number),
    ) {
        return Some((w, h));
    }
    let view_box: Vec<f32> = attr("viewBox")?
        .split([' ', ','])
        .filter_map(|v| v.parse().ok())
        .collect();
    (view_box.len() == 4 && view_box[2] > 0.0 && view_box[3] > 0.0)
        .then(|| (view_box[2], view_box[3]))
}

/// Whether GPUI can render `svg` without reaching outside it. Its SVG
/// renderer (usvg's default resolver) reads any `href` naming a file,
/// network shares included, and parses nested SVG data with the same rule;
/// FR-13 of the port PRD forbids an SVG's external resources. Only fragment
/// references and inline raster images pass; entity declarations,
/// character references and anything else reject the whole image.
pub fn svg_is_self_contained(svg: &[u8]) -> bool {
    const INLINE_RASTER: &[&str] = &[
        "data:image/png",
        "data:image/jpeg",
        "data:image/jpg",
        "data:image/gif",
        "data:image/webp",
    ];
    let Ok(svg) = std::str::from_utf8(svg) else {
        return false;
    };
    if svg.contains("<!ENTITY") {
        return false;
    }
    let mut rest = svg;
    while let Some(at) = rest.find("href") {
        rest = &rest[at + "href".len()..];
        let Some(value) = rest.trim_start().strip_prefix('=') else {
            continue;
        };
        let value = value.trim_start();
        let Some(quote) = value.chars().next().filter(|c| matches!(c, '"' | '\'')) else {
            return false;
        };
        let value = &value[1..];
        let value = value[..value.find(quote).unwrap_or(value.len())]
            .trim()
            .to_ascii_lowercase();
        if !value.starts_with('#') && !INLINE_RASTER.iter().any(|p| value.starts_with(p)) {
            return false;
        }
    }
    true
}

/// The format of `bytes` and its natural size, when the renderer would
/// show it: under the pixel ceiling and, for an SVG, self-contained.
/// `Err(None)` when it is no image at all, `Err(Some(kind))` when it is
/// one Cerata refuses to display.
pub fn check_displayable(bytes: &[u8]) -> Result<(ImageKind, (f32, f32)), Option<ImageKind>> {
    let kind = sniff(bytes).ok_or(None)?;
    let size = natural_size(bytes).ok_or(Some(kind))?;
    if size.0 * size.1 > MAX_PIXELS {
        return Err(Some(kind));
    }
    if kind == ImageKind::Svg && !svg_is_self_contained(bytes) {
        return Err(Some(kind));
    }
    Ok((kind, size))
}

/// A bitmap re-encoded as PNG (lossless), `None` when it does not decode.
fn bmp_to_png(bmp: &[u8]) -> Option<Vec<u8>> {
    let image = image::load_from_memory_with_format(bmp, image::ImageFormat::Bmp).ok()?;
    let mut png = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
        .ok()?;
    Some(png)
}

pub use cerata_markdown::asset_links::markdown_destination;

/// What enters a note.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportSource {
    /// A file: dropped, picked, or copied in the file manager.
    File(PathBuf),
    /// Pasted bytes, with the format the clipboard declared and the base
    /// name to save them under (`YYYYMMDD-HHMMSS-xxxx`).
    Bytes {
        bytes: Vec<u8>,
        declared: ImageKind,
        stem: String,
    },
}

/// Why a source was not added. Its text completes `Image not added: `
/// (see [`Refusal::notice`]).
#[derive(Clone, Debug, PartialEq, thiserror::Error)]
pub enum Refusal {
    #[error("the note is not saved in a folder")]
    NotSaved,
    #[error("{0} is not an image")]
    NotAnImage(String),
    #[error("{0} is not an image Cerata can display")]
    NotDisplayable(String),
    #[error("{0:.1} MB is over the 5 MB limit")]
    TooLarge(f64),
    /// A file that could not be read or written: the OS error.
    #[error("{0}")]
    Io(String),
}

impl Refusal {
    /// The notice for this refusal alone.
    pub fn notice(&self) -> String {
        match self {
            Self::NotAnImage(_) | Self::NotDisplayable(_) => self.to_string(),
            Self::TooLarge(_) => format!("Image not pasted: {self}"),
            Self::NotSaved | Self::Io(_) => format!("Image not added: {self}"),
        }
    }

    fn io(error: &FsError) -> Self {
        Self::Io(match error {
            FsError::Io { source, .. } => source.to_string(),
            other => other.to_string(),
        })
    }
}

/// One notice for the refusals of a gesture: the refusal alone, or
/// `{n} files not added: {first}`. `None` without refusals.
pub fn refusal_notice(refusals: &[Refusal]) -> Option<String> {
    match refusals {
        [] => None,
        [one] => Some(one.notice()),
        [first, ..] => Some(format!("{} files not added: {first}", refusals.len())),
    }
}

/// What a copy writes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CopyFrom {
    File(PathBuf),
    Bytes(Vec<u8>),
}

/// A file to write before the Markdown is inserted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedCopy {
    pub from: CopyFrom,
    /// In `{stem}-assets/`, never taken when planned.
    pub to: PathBuf,
}

/// An accepted source: the Markdown to insert, and the copy that must
/// succeed first (`None` for a file already inside the workspace).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PlannedImage {
    pub markdown: String,
    pub copy: Option<PlannedCopy>,
}

/// Plans `sources` for the note at `note` in the workspace at `root`: for
/// each, in order, the Markdown and copy to perform, or why it is refused.
/// Reads headers and checks names; writes nothing.
pub fn plan_import(
    note: Option<&Path>,
    root: &Path,
    sources: Vec<ImportSource>,
) -> Vec<Result<PlannedImage, Refusal>> {
    let context = match note {
        None => Err(Refusal::NotSaved),
        Some(note) => NoteContext::new(note, root),
    };
    let context = match context {
        Ok(context) => context,
        Err(refusal) => return sources.iter().map(|_| Err(refusal.clone())).collect(),
    };
    let mut taken = HashSet::new();
    sources
        .into_iter()
        .map(|source| context.plan(source, &mut taken))
        .collect()
}

/// Performs `planned`'s copy through `fs` and returns its Markdown. A
/// copied file is checked again, since it may have changed since planning.
pub fn run_import(fs: &WorkspaceFs, planned: PlannedImage) -> Result<String, Refusal> {
    let Some(copy) = planned.copy else {
        return Ok(planned.markdown);
    };
    let bytes = match copy.from {
        CopyFrom::Bytes(bytes) => bytes,
        CopyFrom::File(path) => {
            let bytes = std::fs::read(&path).map_err(|e| Refusal::Io(e.to_string()))?;
            if check_displayable(&bytes).is_err() {
                return Err(Refusal::NotDisplayable(file_name(&path)));
            }
            bytes
        }
    };
    let dir = copy
        .to
        .parent()
        .ok_or_else(|| Refusal::Io("the assets folder has no parent".into()))?;
    fs.ensure_directory(dir).map_err(|e| Refusal::io(&e))?;
    fs.write_new_bytes(&copy.to, &bytes)
        .map_err(|e| Refusal::io(&e))?;
    Ok(planned.markdown)
}

/// The note's folder and assets folder, canonical.
struct NoteContext {
    root: PathBuf,
    dir: PathBuf,
    /// `{stem}-assets`, as written in destinations.
    assets_name: String,
}

impl NoteContext {
    fn new(note: &Path, root: &Path) -> Result<Self, Refusal> {
        let (Some(dir), Some(stem)) = (note.parent(), note.file_stem()) else {
            return Err(Refusal::NotSaved);
        };
        let canonical = |path: &Path| {
            dunce::canonicalize(path).map_err(|e| Refusal::Io(format!("{}: {e}", path.display())))
        };
        Ok(Self {
            root: canonical(root)?,
            dir: canonical(dir)?,
            assets_name: format!("{}-assets", stem.to_string_lossy()),
        })
    }

    fn assets(&self) -> PathBuf {
        self.dir.join(&self.assets_name)
    }

    fn plan(
        &self,
        source: ImportSource,
        taken: &mut HashSet<String>,
    ) -> Result<PlannedImage, Refusal> {
        match source {
            ImportSource::File(path) => self.plan_file(&path, taken),
            ImportSource::Bytes {
                bytes,
                declared,
                stem,
            } => {
                let pasted = format!("image.{}", declared.extension());
                let too_large =
                    |bytes: &[u8]| Refusal::TooLarge(bytes.len() as f64 / (1024.0 * 1024.0));
                let (bytes, kind) = match check_displayable(&bytes) {
                    // A screenshot copied as a bitmap (`CF_DIB`) arrives
                    // uncompressed: saved as PNG, it fits the limit.
                    Ok((ImageKind::Bmp, _)) => match bmp_to_png(&bytes) {
                        Some(png) => (png, ImageKind::Png),
                        None => return Err(Refusal::NotDisplayable(pasted)),
                    },
                    _ if bytes.len() > MAX_PASTE_BYTES => return Err(too_large(&bytes)),
                    Ok((kind, _)) => (bytes, kind),
                    Err(None) => return Err(Refusal::NotAnImage(pasted)),
                    Err(Some(_)) => return Err(Refusal::NotDisplayable(pasted)),
                };
                if bytes.len() > MAX_PASTE_BYTES {
                    return Err(too_large(&bytes));
                }
                let name = self.unique_name(&sanitize_name(&stem), kind.extension(), taken);
                Ok(self.copied(
                    &format!("image.{}", kind.extension()),
                    name,
                    CopyFrom::Bytes(bytes),
                ))
            }
        }
    }

    fn plan_file(&self, path: &Path, taken: &mut HashSet<String>) -> Result<PlannedImage, Refusal> {
        let name = file_name(path);
        let head = read_prefix(path, HEADER_BYTES).map_err(|e| Refusal::Io(e.to_string()))?;
        if sniff(&head).is_none() {
            return Err(Refusal::NotAnImage(name));
        }
        let checked = if is_svg(&head) {
            let whole =
                read_prefix(path, MAX_SVG_BYTES + 1).map_err(|e| Refusal::Io(e.to_string()))?;
            if whole.len() as u64 > MAX_SVG_BYTES {
                return Err(Refusal::NotDisplayable(name));
            }
            check_displayable(&whole)
        } else {
            check_displayable(&head)
        };
        let kind = match checked {
            Ok((kind, _)) => kind,
            Err(None) => return Err(Refusal::NotAnImage(name)),
            Err(Some(_)) => return Err(Refusal::NotDisplayable(name)),
        };
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        let alt = escape_alt(&stem);
        let real = dunce::canonicalize(path).map_err(|e| Refusal::Io(e.to_string()))?;
        if real.starts_with(&self.root) && real.is_file() {
            return Ok(PlannedImage {
                markdown: format!(
                    "![{alt}]({})",
                    markdown_destination(&relative_path(&self.dir, &real))
                ),
                copy: None,
            });
        }
        let copy_name = self.unique_name(&sanitize_name(&stem), kind.extension(), taken);
        Ok(self.copied(&stem, copy_name, CopyFrom::File(path.to_path_buf())))
    }

    fn copied(&self, alt: &str, name: String, from: CopyFrom) -> PlannedImage {
        let destination = format!("{}/{name}", self.assets_name);
        PlannedImage {
            markdown: format!(
                "![{}]({})",
                escape_alt(alt),
                markdown_destination(&destination)
            ),
            copy: Some(PlannedCopy {
                from,
                to: self.assets().join(name),
            }),
        }
    }

    /// `{base}.{ext}`, else `{base} 1.{ext}`, `{base} 2.{ext}`...: free in
    /// the assets folder and not chosen earlier in the same gesture
    /// (compared case-insensitively, as Windows and macOS do).
    fn unique_name(&self, base: &str, ext: &str, taken: &mut HashSet<String>) -> String {
        let assets = self.assets();
        let mut n: u64 = 0;
        loop {
            let name = if n == 0 {
                format!("{base}.{ext}")
            } else {
                format!("{base} {n}.{ext}")
            };
            let key = name.to_lowercase();
            if !taken.contains(&key) {
                if std::fs::symlink_metadata(assets.join(&name)).is_err() {
                    taken.insert(key);
                    return name;
                }
                // Taken on disk: on a case-sensitive file system, a later
                // `SHOT.png` of the gesture must not land beside `shot.png`.
                taken.insert(key);
            }
            n += 1;
        }
    }
}

/// A file name safe on every OS (US-059): reserved characters and control
/// characters become `-`, leading and trailing dots and spaces go, a
/// Windows device name gets a `-` suffix, the name is cut to 120 bytes on a
/// character boundary, and an empty name becomes `image`.
pub fn sanitize_name(name: &str) -> String {
    const RESERVED: &[&str] = &[
        "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
        "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
    ];
    let trim = |s: &str| s.trim_matches(['.', ' ']).to_owned();
    let replaced: String = name
        .chars()
        .map(|c| {
            if r#"<>:"/\|?*"#.contains(c) || c.is_control() {
                '-'
            } else {
                c
            }
        })
        .collect();
    let mut name = trim(&replaced);
    if name.len() > MAX_NAME_BYTES {
        let mut end = MAX_NAME_BYTES;
        while !name.is_char_boundary(end) {
            end -= 1;
        }
        name = trim(&name[..end]);
    }
    if name.is_empty() {
        return "image".into();
    }
    if RESERVED.iter().any(|r| r.eq_ignore_ascii_case(&name)) {
        name.push('-');
    }
    name
}

/// An image's alt text: `\`, `[` and `]` escaped.
fn escape_alt(alt: &str) -> String {
    let mut out = String::with_capacity(alt.len());
    for c in alt.chars() {
        if matches!(c, '\\' | '[' | ']') {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// `target` relative to the folder `dir`, with `/` separators. Both are
/// canonical and inside the same root.
fn relative_path(dir: &Path, target: &Path) -> String {
    let dir: Vec<Component> = dir.components().collect();
    let target: Vec<Component> = target.components().collect();
    let common = dir.iter().zip(&target).take_while(|(a, b)| a == b).count();
    let mut parts: Vec<String> = dir[common..].iter().map(|_| "..".to_owned()).collect();
    parts.extend(
        target[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    parts.join("/")
}

fn file_name(path: &Path) -> String {
    path.file_name().map_or_else(
        || path.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    )
}

/// The first `limit` bytes of a file.
fn read_prefix(path: &Path, limit: u64) -> std::io::Result<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)?
        .take(limit)
        .read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        image::RgbaImage::new(width, height)
            .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Png)
            .unwrap();
        bytes
    }

    /// A PNG header claiming `width` x `height` pixels, without the pixels.
    fn png_header(width: u32, height: u32) -> Vec<u8> {
        let mut bytes = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR".to_vec();
        bytes.extend(width.to_be_bytes());
        bytes.extend(height.to_be_bytes());
        bytes.extend([8, 6, 0, 0, 0]);
        let crc = crc32(&bytes[12..]);
        bytes.extend(crc.to_be_bytes());
        bytes
    }

    fn crc32(bytes: &[u8]) -> u32 {
        let mut crc = !0u32;
        for byte in bytes {
            crc ^= u32::from(*byte);
            for _ in 0..8 {
                crc = if crc & 1 == 1 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    struct Workspace {
        root: tempfile::TempDir,
        outside: tempfile::TempDir,
    }

    impl Workspace {
        fn new() -> Self {
            let root = tempfile::tempdir().unwrap();
            std::fs::create_dir(root.path().join("notes")).unwrap();
            std::fs::write(root.path().join("notes/foo.md"), "# Foo").unwrap();
            Self {
                root,
                outside: tempfile::tempdir().unwrap(),
            }
        }

        fn note(&self) -> PathBuf {
            self.root.path().join("notes/foo.md")
        }

        fn plan(&self, sources: Vec<ImportSource>) -> Vec<Result<PlannedImage, Refusal>> {
            plan_import(Some(&self.note()), self.root.path(), sources)
        }

        fn outside_file(&self, name: &str, bytes: &[u8]) -> PathBuf {
            let path = self.outside.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        }

        fn fs(&self) -> WorkspaceFs {
            WorkspaceFs::new(self.root.path()).unwrap()
        }
    }

    fn markdown(planned: &Result<PlannedImage, Refusal>) -> &str {
        &planned.as_ref().unwrap().markdown
    }

    #[test]
    fn a_file_inside_the_workspace_is_linked_without_a_copy() {
        let ws = Workspace::new();
        std::fs::create_dir(ws.root.path().join("pics")).unwrap();
        std::fs::write(ws.root.path().join("pics/Shot [1].png"), png(3, 2)).unwrap();
        std::fs::write(ws.root.path().join("notes/here.png"), png(3, 2)).unwrap();
        let plan = ws.plan(vec![
            ImportSource::File(ws.root.path().join("pics/Shot [1].png")),
            ImportSource::File(ws.root.path().join("notes/here.png")),
        ]);
        assert_eq!(markdown(&plan[0]), r"![Shot \[1\]](<../pics/Shot [1].png>)");
        assert_eq!(markdown(&plan[1]), "![here](here.png)");
        assert!(plan.iter().all(|p| p.as_ref().unwrap().copy.is_none()));
    }

    #[test]
    fn a_file_outside_is_copied_with_the_detected_extension() {
        let ws = Workspace::new();
        // A JPEG named `.png`: the signature wins.
        let mut jpeg = Vec::new();
        image::RgbImage::new(2, 2)
            .write_to(&mut Cursor::new(&mut jpeg), image::ImageFormat::Jpeg)
            .unwrap();
        let path = ws.outside_file("photo.png", &jpeg);
        let plan = ws.plan(vec![ImportSource::File(path.clone())]);
        let planned = plan[0].clone().unwrap();
        assert_eq!(planned.markdown, "![photo](foo-assets/photo.jpg)");
        let copy = planned.copy.clone().unwrap();
        assert_eq!(copy.from, CopyFrom::File(path));
        assert!(copy.to.ends_with("foo-assets/photo.jpg"));
        assert!(!copy.to.exists(), "planning writes nothing");
        assert_eq!(
            run_import(&ws.fs(), planned).unwrap(),
            "![photo](foo-assets/photo.jpg)"
        );
        assert_eq!(std::fs::read(&copy.to).unwrap(), jpeg);
    }

    #[cfg(unix)]
    #[test]
    fn an_inner_symlink_pointing_outside_is_copied() {
        let ws = Workspace::new();
        let target = ws.outside_file("secret.png", &png(3, 2));
        let link = ws.root.path().join("notes/link.png");
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let plan = ws.plan(vec![ImportSource::File(link)]);
        assert_eq!(markdown(&plan[0]), "![link](foo-assets/link.png)");
        assert!(plan[0].as_ref().unwrap().copy.is_some());
    }

    #[cfg(windows)]
    #[test]
    fn an_inner_symlink_pointing_outside_is_copied() {
        let ws = Workspace::new();
        let target = ws.outside_file("secret.png", &png(3, 2));
        let link = ws.root.path().join("notes/link.png");
        // Creating a symlink needs Developer Mode or an elevated shell.
        if std::os::windows::fs::symlink_file(&target, &link).is_err() {
            eprintln!("skipped: symlinks are not allowed here");
            return;
        }
        let plan = ws.plan(vec![ImportSource::File(link)]);
        assert_eq!(markdown(&plan[0]), "![link](foo-assets/link.png)");
        assert!(plan[0].as_ref().unwrap().copy.is_some());
    }

    #[test]
    fn taken_names_and_batch_duplicates_get_a_counter() {
        let ws = Workspace::new();
        std::fs::create_dir(ws.root.path().join("notes/foo-assets")).unwrap();
        std::fs::write(ws.root.path().join("notes/foo-assets/shot.png"), b"x").unwrap();
        let a = ws.outside_file("shot.png", &png(3, 2));
        std::fs::create_dir(ws.outside.path().join("other")).unwrap();
        let b = ws.outside.path().join("other/SHOT.png");
        std::fs::write(&b, png(3, 2)).unwrap();
        let plan = ws.plan(vec![ImportSource::File(a), ImportSource::File(b)]);
        assert_eq!(markdown(&plan[0]), "![shot](<foo-assets/shot 1.png>)");
        assert_eq!(markdown(&plan[1]), "![SHOT](<foo-assets/SHOT 2.png>)");
    }

    #[test]
    fn names_are_sanitized() {
        assert_eq!(
            sanitize_name("a<b>c:d\"e/f\\g|h?i*j"),
            "a-b-c-d-e-f-g-h-i-j"
        );
        assert_eq!(sanitize_name("tab\there"), "tab-here");
        assert_eq!(sanitize_name(" ..hidden. "), "hidden");
        assert_eq!(sanitize_name("CON"), "CON-");
        assert_eq!(sanitize_name("com7"), "com7-");
        assert_eq!(sanitize_name("console"), "console");
        assert_eq!(sanitize_name(""), "image");
        assert_eq!(sanitize_name("..."), "image");
        let long = "é".repeat(100);
        let cut = sanitize_name(&long);
        assert_eq!(cut.len(), 120);
        assert!(cut.chars().all(|c| c == 'é'));
    }

    #[test]
    fn a_reserved_or_empty_name_is_copied_under_a_safe_name() {
        let ws = Workspace::new();
        // Before Windows 11, `nul.png` still names the NUL device unless the
        // path is verbatim (`\\?\`), so the source is written and read that way.
        let reserved = ws.outside.path().join("nul.png");
        #[cfg(windows)]
        let reserved = {
            let mut verbatim = std::ffi::OsString::from(r"\\?\");
            verbatim.push(reserved.as_os_str());
            PathBuf::from(verbatim)
        };
        std::fs::write(&reserved, png(3, 2)).unwrap();
        let empty = ws.outside_file("....png", &png(3, 2));
        let plan = ws.plan(vec![
            ImportSource::File(reserved),
            ImportSource::File(empty),
        ]);
        assert_eq!(markdown(&plan[0]), "![nul](foo-assets/nul-.png)");
        let copy = plan[1].as_ref().unwrap().copy.as_ref().unwrap();
        assert!(copy.to.ends_with("foo-assets/image.png"), "{copy:?}");
    }

    #[test]
    fn refusals_name_the_file_and_write_nothing() {
        let ws = Workspace::new();
        let text = ws.outside_file("notes.txt", b"hello");
        let leak = ws.outside_file(
            "diagram.svg",
            br#"<svg width="4" height="4"><image href="//host/share/a.png"/></svg>"#,
        );
        let huge = ws.outside_file("huge.png", &png_header(10_000, 6_000));
        let plan = ws.plan(vec![
            ImportSource::File(text),
            ImportSource::File(leak),
            ImportSource::File(huge),
        ]);
        let refusals: Vec<String> = plan
            .iter()
            .map(|p| p.as_ref().unwrap_err().notice())
            .collect();
        assert_eq!(
            refusals,
            [
                "notes.txt is not an image",
                "diagram.svg is not an image Cerata can display",
                "huge.png is not an image Cerata can display",
            ]
        );
        assert!(!ws.root.path().join("notes/foo-assets").exists());
    }

    #[test]
    fn a_note_without_a_path_refuses_everything() {
        let ws = Workspace::new();
        let path = ws.outside_file("a.png", &png(3, 2));
        let plan = plan_import(None, ws.root.path(), vec![ImportSource::File(path)]);
        assert_eq!(plan, [Err(Refusal::NotSaved)]);
        assert_eq!(
            Refusal::NotSaved.notice(),
            "Image not added: the note is not saved in a folder"
        );
    }

    #[test]
    fn pasted_bytes_keep_their_stamp_alt_and_limit() {
        let ws = Workspace::new();
        let pasted = |bytes: Vec<u8>| ImportSource::Bytes {
            bytes,
            declared: ImageKind::Png,
            stem: "20261102-101500-00ab".into(),
        };
        let plan = ws.plan(vec![
            pasted(png(3, 2)),
            pasted(png(3, 2)),
            pasted(vec![0; MAX_PASTE_BYTES + 1]),
            pasted(b"garbage".to_vec()),
        ]);
        assert_eq!(
            markdown(&plan[0]),
            "![image.png](foo-assets/20261102-101500-00ab.png)"
        );
        assert_eq!(
            markdown(&plan[1]),
            "![image.png](<foo-assets/20261102-101500-00ab 1.png>)"
        );
        assert_eq!(
            plan[2].as_ref().unwrap_err().notice(),
            "Image not pasted: 5.0 MB is over the 5 MB limit"
        );
        assert_eq!(
            plan[3].as_ref().unwrap_err().notice(),
            "image.png is not an image"
        );
        // A pasted bitmap over the limit is saved as a PNG under it.
        let mut bmp = Vec::new();
        image::RgbImage::from_pixel(1400, 1400, image::Rgb([40, 90, 160]))
            .write_to(&mut Cursor::new(&mut bmp), image::ImageFormat::Bmp)
            .unwrap();
        assert!(bmp.len() > MAX_PASTE_BYTES);
        let planned = ws
            .plan(vec![ImportSource::Bytes {
                bytes: bmp,
                declared: ImageKind::Bmp,
                stem: "20261102-101500-00ac".into(),
            }])
            .remove(0)
            .unwrap();
        assert_eq!(
            planned.markdown,
            "![image.png](foo-assets/20261102-101500-00ac.png)"
        );
        let Some(PlannedCopy {
            from: CopyFrom::Bytes(saved),
            ..
        }) = &planned.copy
        else {
            panic!("a copy of the bytes");
        };
        assert_eq!(sniff(saved), Some(ImageKind::Png));
        assert!(saved.len() < MAX_PASTE_BYTES);
        // A file has no byte limit.
        let mut big = png(3, 2);
        big.resize(MAX_PASTE_BYTES + 1024 * 1024, 0);
        let file = ws.outside_file("big.png", &big);
        assert!(ws.plan(vec![ImportSource::File(file)])[0].is_ok());
    }

    #[test]
    fn writes_stay_inside_the_root() {
        let ws = Workspace::new();
        let planned = PlannedImage {
            markdown: String::new(),
            copy: Some(PlannedCopy {
                from: CopyFrom::Bytes(png(3, 2)),
                to: ws.outside.path().join("foo-assets/x.png"),
            }),
        };
        assert!(run_import(&ws.fs(), planned).is_err());
        assert!(!ws.outside.path().join("foo-assets").exists());
    }

    #[test]
    fn refusals_are_summarized_once() {
        assert_eq!(refusal_notice(&[]), None);
        let txt = Refusal::NotAnImage("notes.txt".into());
        assert_eq!(
            refusal_notice(std::slice::from_ref(&txt)).unwrap(),
            "notes.txt is not an image"
        );
        assert_eq!(
            refusal_notice(&[txt.clone(), Refusal::NotAnImage("a.pdf".into())]).unwrap(),
            "2 files not added: notes.txt is not an image"
        );
    }

    /// `paths.test.ts:102-110`: a written destination with spaces goes in
    /// angle brackets; one without is left as is.
    #[test]
    fn written_destinations_bracket_spaces() {
        assert_eq!(
            markdown_destination("Writer TODOs-assets/image.png"),
            "<Writer TODOs-assets/image.png>"
        );
        assert_eq!(
            markdown_destination("note-assets/image.png"),
            "note-assets/image.png"
        );
        assert_eq!(markdown_destination("a <b>.png"), "<a %3Cb%3E.png>");
        assert_eq!(markdown_destination("100%20.png"), "100%2520.png");
    }
}
