//! Block widget host: image sources, their measured sizes, the elements
//! drawn for them, and pasted images saved next to the note.
//!
//! Writer's references: `fold/image.ts:12-121` (height cache per
//! destination), `image-src-resolver.ts:5-24` and `paths.ts:217-225`
//! (resolution), `editor-clipboard.ts:9-33` and `commands/images.rs:13-119`
//! (paste), the CSP of `tauri.conf.json:32-38` (`img-src 'self' data: asset:
//! https:`: never `http:`).

use std::collections::HashMap;
use std::io::{Cursor, Read as _};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[cfg(test)]
use gpui::InteractiveElement as _;
use gpui::{AnyElement, Image, ImageFormat, IntoElement, ParentElement, Styled, div, img, px};

use super::layout::ImageBox;
use super::theme;

/// Writer's paste limit (`editor-clipboard.ts:9-10`).
pub const MAX_PASTE_BYTES: usize = 5 * 1024 * 1024;
/// A remote image larger than this is not loaded.
const MAX_REMOTE_BYTES: u64 = 25 * 1024 * 1024;
/// A local SVG larger than this is not loaded (it is read whole, to be
/// checked before GPUI sees it).
const MAX_SVG_BYTES: u64 = 8 * 1024 * 1024;
/// GPUI decodes an image to RGBA at its natural size: a header claiming
/// more pixels than this (a decompression bomb) is not loaded.
const MAX_PIXELS: f32 = 50_000_000.0;

/// Where an image destination points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Local(PathBuf),
    Remote(String),
    Data(Vec<u8>),
}

/// The source an image destination loads from, relative to the note's
/// folder; `None` for schemes that are never loaded (`http:` and others).
pub fn resolve(note_dir: Option<&Path>, url: &str) -> Option<Source> {
    let lower = url.to_ascii_lowercase();
    if lower.starts_with("https://") {
        return Some(Source::Remote(url.to_owned()));
    }
    if lower.starts_with("data:") {
        return decode_data_uri(url).map(Source::Data);
    }
    if lower.starts_with("file://") {
        // Only `file:///path`: a host names a network share.
        let path = percent_decode(url[7..].strip_prefix('/')?);
        let drive = path.as_bytes().get(1) == Some(&b':');
        let path = if drive { path } else { format!("/{path}") };
        return local(PathBuf::from(path));
    }
    // Any other scheme (`http:`, `ftp:`...) is never loaded; a Windows drive
    // letter (`C:\`) is a path, not a scheme.
    if let Some(colon) = url.find(':') {
        let scheme = &url[..colon];
        let drive = colon == 1 && url[colon + 1..].starts_with(['\\', '/']);
        if !drive
            && !scheme.is_empty()
            && scheme
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.".contains(c))
        {
            return None;
        }
    }
    let path = PathBuf::from(percent_decode(url));
    if path.is_absolute() || url.starts_with('/') {
        return local(path);
    }
    Some(Source::Local(
        note_dir.map_or(path.clone(), |dir| dir.join(&path)),
    ))
}

/// A local source, unless the note itself names a network path
/// (`\\host\share`, `//host/share`): touching one makes Windows connect to
/// the host and offer the user's credentials. A note folder on a share
/// stays usable, since only the destination as written is checked.
fn local(path: PathBuf) -> Option<Source> {
    (!names_network_path(&path)).then_some(Source::Local(path))
}

fn names_network_path(path: &Path) -> bool {
    // Windows reads `/` and `\` alike: two leading separators of any mix
    // (`/\host\share`) name a share.
    let s = path.as_os_str().to_string_lossy();
    let mut chars = s.chars();
    matches!(
        (chars.next(), chars.next()),
        (Some('/' | '\\'), Some('/' | '\\'))
    )
}

/// `decodeURIComponent`, keeping the input on invalid sequences.
pub fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && let Some(hex) = s.get(i + 1..i + 3)
            && let Ok(byte) = u8::from_str_radix(hex, 16)
        {
            out.push(byte);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| s.to_owned())
}

/// The bytes of a `data:` URI (base64 or percent-encoded).
fn decode_data_uri(url: &str) -> Option<Vec<u8>> {
    let (meta, payload) = url.get(5..)?.split_once(',')?;
    if meta.ends_with(";base64") {
        base64_decode(payload)
    } else {
        Some(percent_decode(payload).into_bytes())
    }
}

fn base64_decode(s: &str) -> Option<Vec<u8>> {
    let value = |c: u8| -> Option<u32> {
        Some(match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' | b'-' => 62,
            b'/' | b'_' => 63,
            _ => return None,
        } as u32)
    };
    let mut out = Vec::with_capacity(s.len() * 3 / 4);
    let mut acc = 0u32;
    let mut bits = 0;
    for c in s.bytes().filter(|c| !c.is_ascii_whitespace() && *c != b'=') {
        acc = (acc << 6) | value(c)?;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
        }
    }
    Some(out)
}

/// The image format of `bytes`, SVG included.
pub fn sniff_format(bytes: &[u8]) -> Option<ImageFormat> {
    if is_svg(bytes) {
        return Some(ImageFormat::Svg);
    }
    Some(match image::guess_format(bytes).ok()? {
        image::ImageFormat::Png => ImageFormat::Png,
        image::ImageFormat::Jpeg => ImageFormat::Jpeg,
        image::ImageFormat::WebP => ImageFormat::Webp,
        image::ImageFormat::Gif => ImageFormat::Gif,
        image::ImageFormat::Bmp => ImageFormat::Bmp,
        image::ImageFormat::Tiff => ImageFormat::Tiff,
        image::ImageFormat::Ico => ImageFormat::Ico,
        _ => return None,
    })
}

fn is_svg(bytes: &[u8]) -> bool {
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
/// FR-13 forbids an SVG's external resources. Only fragment references and
/// inline raster images pass; entity declarations, character references
/// and anything else reject the whole image.
fn svg_is_self_contained(svg: &[u8]) -> bool {
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

/// What a load produces.
#[derive(Clone, Debug)]
pub enum Loaded {
    Path(PathBuf),
    Bytes(Arc<Image>),
}

#[derive(Clone, Debug)]
enum Entry {
    /// No such local file, or a scheme that is never loaded.
    Unavailable,
    Pending,
    Ready {
        natural: (f32, f32),
        data: Loaded,
    },
    /// Unreadable: drawn as a placeholder at its estimated height.
    Failed,
}

/// A load for the caller to run off the main thread.
pub struct LoadJob {
    pub url: String,
    pub source: Source,
}

/// A job's url, with the image's natural size and what to draw, if it loaded.
pub type LoadResult = (String, Option<((f32, f32), Loaded)>);

impl LoadJob {
    /// Reads the image's header (and, when remote or inline, its bytes).
    pub fn run(self) -> LoadResult {
        let result = match self.source {
            Source::Local(path) => read_prefix(&path, 64 * 1024).and_then(|header| {
                if is_svg(&header) {
                    // GPUI would read the file itself: an SVG is checked
                    // like an inline or remote one and handed over as bytes.
                    read_prefix(&path, MAX_SVG_BYTES).and_then(bytes_loaded)
                } else {
                    natural_size(&header).map(|size| (size, Loaded::Path(path)))
                }
            }),
            Source::Remote(url) => fetch(&url).and_then(bytes_loaded),
            Source::Data(bytes) => bytes_loaded(bytes),
        };
        let result = result.filter(|((width, height), _)| width * height <= MAX_PIXELS);
        (self.url, result)
    }
}

fn bytes_loaded(bytes: Vec<u8>) -> Option<((f32, f32), Loaded)> {
    let size = natural_size(&bytes)?;
    let format = sniff_format(&bytes)?;
    if format == ImageFormat::Svg && !svg_is_self_contained(&bytes) {
        return None;
    }
    Some((
        size,
        Loaded::Bytes(Arc::new(Image::from_bytes(format, bytes))),
    ))
}

/// The first `limit` bytes of a file.
fn read_prefix(path: &Path, limit: u64) -> Option<Vec<u8>> {
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(limit)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

/// HTTPS only, redirects included; bounded in size and time.
fn fetch(url: &str) -> Option<Vec<u8>> {
    let agent: ureq::Agent = ureq::Agent::config_builder()
        .https_only(true)
        .timeout_global(Some(std::time::Duration::from_secs(20)))
        .build()
        .into();
    let response = agent.get(url).call().ok()?;
    response
        .into_body()
        .with_config()
        .limit(MAX_REMOTE_BYTES)
        .read_to_vec()
        .ok()
}

/// Image sources seen by one editor, their sizes, and the heights already
/// reserved for them per column width.
#[derive(Default)]
pub struct ImageHost {
    note_dir: Option<PathBuf>,
    /// The canonical folder local images must be inside (FR-13): the
    /// workspace root, else the note's folder.
    root: Option<PathBuf>,
    entries: HashMap<String, Entry>,
    /// Height per (destination, display width): what the line reserves
    /// before the image is measured again (Writer's `imageHeightCache`).
    heights: HashMap<(String, u32), f32>,
}

impl ImageHost {
    pub fn new(note_dir: Option<PathBuf>) -> Self {
        Self {
            root: note_dir
                .as_deref()
                .and_then(|dir| std::fs::canonicalize(dir).ok()),
            note_dir,
            ..Self::default()
        }
    }

    /// Confines local images to the workspace at `root` rather than the
    /// note's folder.
    pub fn set_root(&mut self, root: &Path) {
        self.root = std::fs::canonicalize(root).ok();
        self.entries.clear();
    }

    /// An existing file inside the root, symlinks resolved.
    fn loadable(&self, path: &Path) -> bool {
        let (Some(root), Ok(real)) = (&self.root, std::fs::canonicalize(path)) else {
            return false;
        };
        real.starts_with(root) && real.is_file()
    }

    pub fn note_dir(&self) -> Option<&Path> {
        self.note_dir.as_deref()
    }

    /// The box of the image at `url` in a column `width` wide, and a load
    /// to start when the image was never seen. Before it is measured (and
    /// when it cannot be read) an image keeps its estimated height: the last
    /// height cached for this source and width, else one text row.
    pub fn image_box(
        &mut self,
        url: &str,
        width: f32,
        row_height: f32,
    ) -> (ImageBox, Option<LoadJob>) {
        let mut job = None;
        if !self.entries.contains_key(url) {
            let entry = match resolve(self.note_dir.as_deref(), url) {
                None => Entry::Unavailable,
                Some(Source::Local(path)) if !self.loadable(&path) => Entry::Unavailable,
                Some(source) => {
                    job = Some(LoadJob {
                        url: url.to_owned(),
                        source,
                    });
                    Entry::Pending
                }
            };
            self.entries.insert(url.to_owned(), entry);
        }
        let column = width.max(1.0).floor();
        let key = (url.to_owned(), column as u32);
        let image_box = match self.entries.get(url) {
            Some(Entry::Unavailable) | None => ImageBox::Unavailable,
            Some(Entry::Ready { natural, .. }) => {
                let display = natural.0.min(column).floor().max(1.0);
                let height = (natural.1 * display / natural.0).round().max(1.0);
                self.heights.insert(key, height);
                ImageBox::Sized {
                    width: display,
                    height,
                }
            }
            Some(Entry::Pending | Entry::Failed) => ImageBox::Sized {
                width: column,
                height: self.heights.get(&key).copied().unwrap_or(row_height),
            },
        };
        (image_box, job)
    }

    /// A load finished; `true` when the image's size is now known.
    pub fn finish(&mut self, url: String, result: Option<((f32, f32), Loaded)>) -> bool {
        let ready = result.is_some();
        let entry = match result {
            Some((natural, data)) => Entry::Ready { natural, data },
            None => Entry::Failed,
        };
        self.entries.insert(url, entry);
        ready
    }

    /// The element drawn for the image at `url`, `width` by `height`.
    pub fn element(&self, url: &str, width: f32, height: f32, id: String) -> AnyElement {
        let frame = div();
        #[cfg(test)]
        let frame = frame.debug_selector(move || id);
        #[cfg(not(test))]
        let _ = id;
        let frame = frame.w(px(width)).h(px(height)).rounded(px(4.0));
        match self.entries.get(url) {
            Some(Entry::Ready {
                data: Loaded::Path(path),
                ..
            }) => frame
                .child(img(path.clone()).size_full())
                .into_any_element(),
            Some(Entry::Ready {
                data: Loaded::Bytes(image),
                ..
            }) => frame
                .child(img(image.clone()).size_full())
                .into_any_element(),
            _ => frame.bg(theme::code_background()).into_any_element(),
        }
    }
}

/// Why a pasted image was not saved.
#[derive(Debug, thiserror::Error)]
pub enum PasteError {
    #[error("Image not pasted: {0:.1} MB is over the 5 MB limit")]
    TooLarge(f64),
    #[error("Image not pasted: the note is not saved in a folder")]
    NoFolder,
    #[error("Image not pasted: {0}")]
    Io(#[from] std::io::Error),
}

/// Saves a pasted image as `{stem}-assets/YYYYMMDD-HHMMSS-xxxx.ext` next to
/// `note` (local time; Writer used UTC) and returns the Markdown to insert.
pub fn save_pasted_image(
    note: &Path,
    image: &Image,
    now: chrono::DateTime<chrono::Local>,
) -> Result<String, PasteError> {
    if image.bytes.len() > MAX_PASTE_BYTES {
        return Err(PasteError::TooLarge(
            image.bytes.len() as f64 / (1024.0 * 1024.0),
        ));
    }
    let dir = note.parent().ok_or(PasteError::NoFolder)?;
    let stem = note
        .file_stem()
        .ok_or(PasteError::NoFolder)?
        .to_string_lossy()
        .into_owned();
    let assets = format!("{stem}-assets");
    std::fs::create_dir_all(dir.join(&assets))?;
    let ext = match image.format {
        ImageFormat::Jpeg => "jpg",
        ImageFormat::Webp => "webp",
        ImageFormat::Gif => "gif",
        ImageFormat::Svg => "svg",
        ImageFormat::Bmp => "bmp",
        ImageFormat::Tiff => "tiff",
        _ => "png",
    };
    let stamp = now.format("%Y%m%d-%H%M%S");
    let mut suffix = (image.id ^ now.timestamp_nanos_opt().unwrap_or(0) as u64) & 0xffff;
    let name = loop {
        let name = format!("{stamp}-{suffix:04x}.{ext}");
        if !dir.join(&assets).join(&name).exists() {
            break name;
        }
        suffix = (suffix + 1) & 0xffff;
    };
    cerata_core::fs::atomic::atomic_write(&dir.join(&assets).join(&name), &image.bytes)?;
    let relative = format!("{assets}/{name}");
    Ok(format!(
        "![image.{ext}]({})",
        markdown_destination(&relative)
    ))
}

/// Writer's `formatMarkdownDestination`: angle brackets around a
/// destination with spaces or parentheses.
pub fn markdown_destination(path: &str) -> String {
    if path.contains([' ', '\t', '(', ')', '<', '>']) {
        format!("<{}>", path.replace('<', "%3C").replace('>', "%3E"))
    } else {
        path.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destinations_resolve_by_scheme() {
        let dir = Path::new("/notes");
        assert_eq!(
            resolve(Some(dir), "img/a%20b.png"),
            Some(Source::Local(dir.join("img/a b.png")))
        );
        assert_eq!(
            resolve(Some(dir), "https://example.com/a.png"),
            Some(Source::Remote("https://example.com/a.png".into()))
        );
        assert_eq!(resolve(Some(dir), "http://example.com/a.png"), None);
        assert_eq!(resolve(Some(dir), "ftp://example.com/a.png"), None);
        assert_eq!(
            resolve(Some(dir), "data:image/png;base64,iVBORw0KGgo="),
            Some(Source::Data(b"\x89PNG\r\n\x1a\n".to_vec()))
        );
        assert!(matches!(
            resolve(Some(dir), "C:\\pics\\a.png"),
            Some(Source::Local(_))
        ));
        assert_eq!(
            resolve(Some(dir), "file:///C:/pics/a%20b.png"),
            Some(Source::Local(PathBuf::from("C:/pics/a b.png")))
        );
        assert_eq!(
            resolve(Some(dir), "file:///srv/a.png"),
            Some(Source::Local(PathBuf::from("/srv/a.png")))
        );
        // A network share is never touched, however it is spelled.
        for url in [
            "\\\\host\\share\\a.png",
            "//host/share/a.png",
            "%5C%5Chost%5Cshare%5Ca.png",
            "file://host/share/a.png",
            "file:////host/share/a.png",
            "/\\host\\share\\a.png",
            "\\/host/share/a.png",
            "file:///\\host\\share\\a.png",
        ] {
            assert_eq!(resolve(Some(dir), url), None, "{url}");
        }
    }

    #[test]
    fn natural_sizes_come_from_headers() {
        let svg = br#"<svg xmlns="http://www.w3.org/2000/svg" width="640" height="251"></svg>"#;
        assert_eq!(natural_size(svg), Some((640.0, 251.0)));
        let boxed = br#"<svg viewBox="0 0 100 50"></svg>"#;
        assert_eq!(natural_size(boxed), Some((100.0, 50.0)));
        let mut png = Vec::new();
        image::RgbaImage::new(3, 2)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        assert_eq!(natural_size(&png), Some((3.0, 2.0)));
        assert_eq!(natural_size(b"not an image"), None);
    }

    #[test]
    fn svg_attributes_with_any_quote_never_panic() {
        assert_eq!(
            natural_size("<svg width=\u{e9}100\u{e9} height=\"5\">".as_bytes()),
            Some((100.0, 5.0))
        );
        assert_eq!(
            natural_size(b"<svg width='12' height='5'></svg>"),
            Some((12.0, 5.0))
        );
    }

    #[test]
    fn an_svg_reaching_outside_itself_is_not_loaded() {
        let svg = |body: &str| {
            format!(r#"<svg xmlns="http://www.w3.org/2000/svg" width="4" height="4">{body}</svg>"#)
        };
        for body in [
            "",
            r##"<use href="#a"/>"##,
            r#"<image href="data:image/png;base64,iVBORw0KGgo="/>"#,
        ] {
            assert!(bytes_loaded(svg(body).into_bytes()).is_some(), "{body}");
        }
        for body in [
            r#"<image href="\\host\share\a.png"/>"#,
            r#"<image xlink:href = 'C:/secret.png'/>"#,
            r#"<image href="a.png"/>"#,
            r#"<feImage href="file:///etc/x.png"/>"#,
            r#"<image href="data:image/svg+xml;base64,PHN2Zz4="/>"#,
            r#"<image href="&#92;&#92;host\a.png"/>"#,
            r#"<image href=a.png/>"#,
        ] {
            assert!(bytes_loaded(svg(body).into_bytes()).is_none(), "{body}");
        }
        let entity = r#"<!DOCTYPE svg [<!ENTITY h "a.png">]><svg width="4" height="4"><image href="&h;"/></svg>"#;
        assert!(bytes_loaded(entity.as_bytes().to_vec()).is_none());
    }

    #[test]
    fn local_svgs_are_checked_and_images_stay_inside_the_root() {
        let workspace = tempfile::tempdir().unwrap();
        let notes = workspace.path().join("notes");
        std::fs::create_dir(&notes).unwrap();
        let outside = tempfile::tempdir().unwrap();
        let mut png = Vec::new();
        image::RgbaImage::new(3, 2)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        std::fs::write(notes.join("in.png"), &png).unwrap();
        std::fs::write(workspace.path().join("up.png"), &png).unwrap();
        std::fs::write(outside.path().join("out.png"), &png).unwrap();
        std::fs::write(
            notes.join("leak.svg"),
            r#"<svg width="4" height="4"><image href="//host/share/a.png"/></svg>"#,
        )
        .unwrap();
        let mut host = ImageHost::new(Some(notes.clone()));
        assert!(host.image_box("in.png", 700.0, 30.0).1.is_some());
        let out = outside
            .path()
            .join("out.png")
            .to_string_lossy()
            .into_owned();
        assert_eq!(host.image_box(&out, 700.0, 30.0).0, ImageBox::Unavailable);
        assert_eq!(
            host.image_box("../up.png", 700.0, 30.0).0,
            ImageBox::Unavailable,
            "outside the note's folder when no workspace is set"
        );
        host.set_root(workspace.path());
        assert!(host.image_box("../up.png", 700.0, 30.0).1.is_some());
        let (_, job) = host.image_box("leak.svg", 700.0, 30.0);
        let (_, result) = job.expect("inside the root").run();
        assert!(result.is_none(), "its external reference is refused");
    }

    #[test]
    fn a_header_claiming_too_many_pixels_is_not_loaded() {
        let job = LoadJob {
            url: "x".into(),
            source: Source::Data(br#"<svg width="100000" height="100000"></svg>"#.to_vec()),
        };
        assert!(job.run().1.is_none());
    }

    #[test]
    fn heights_are_cached_per_source_and_width_and_kept_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.png"), b"garbage").unwrap();
        let mut host = ImageHost::new(Some(dir.path().to_path_buf()));
        assert_eq!(
            host.image_box("missing.png", 700.0, 30.0).0,
            ImageBox::Unavailable
        );
        let (estimate, job) = host.image_box("bad.png", 700.0, 30.0);
        assert_eq!(
            estimate,
            ImageBox::Sized {
                width: 700.0,
                height: 30.0
            }
        );
        let (url, result) = job.expect("a load starts").run();
        assert!(!host.finish(url, result), "unreadable");
        // Asking again keeps the estimate: no jump.
        let (again, job) = host.image_box("bad.png", 700.0, 30.0);
        assert_eq!(again, estimate);
        assert!(job.is_none());
    }
}
