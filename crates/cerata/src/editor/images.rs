//! Block widget host: image sources, their measured sizes, the elements
//! drawn for them, and pasted images as import sources (`cerata_core::images`).
//!
//! Writer's references: `fold/image.ts:12-121` (height cache per
//! destination), `image-src-resolver.ts:5-24` and `paths.ts:217-225`
//! (resolution), `editor-clipboard.ts:9-33` and `commands/images.rs:13-119`
//! (paste), the CSP of `tauri.conf.json:32-38` (`img-src 'self' data: asset:
//! https:`: never `http:`).

use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyElement, Image, ImageFormat, InteractiveElement as _, IntoElement, ParentElement,
    SharedString, StatefulInteractiveElement as _, Styled, div, img, px,
};

pub use cerata_core::images::natural_size;
use cerata_core::images::{
    ImageKind, ImportSource, MAX_PIXELS, MAX_SVG_BYTES, is_svg, svg_is_self_contained,
};
use cerata_core::workspace::NameIndex;

use super::layout::ImageBox;
use super::theme;

/// A remote image larger than this is not loaded.
const MAX_REMOTE_BYTES: u64 = 25 * 1024 * 1024;

/// Where an image destination points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    Local(PathBuf),
    Remote(String),
    Data(Vec<u8>),
    /// An image embed (`![[pic.png]]`), found in the background, then read
    /// like a local image inside `confine`.
    Embed {
        target: String,
        workspace: Option<PathBuf>,
        note_dir: Option<PathBuf>,
        confine: Option<PathBuf>,
    },
}

/// The key an image embed's entry is kept under: never a destination
/// (`[[` cannot start one).
pub fn embed_key(target: &str) -> String {
    format!("\u{1}wiki:{target}")
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
    // Checked before the join: on Unix `\\host\share` is a relative name,
    // and the same note opened on Windows would reach the share.
    if names_network_path(&path) {
        return None;
    }
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
        (Some('/' | '\\'), Some('/' | '\\' | '?'))
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

/// The image format of `bytes`, SVG included: the import pipeline's
/// detection (`cerata_core::images::sniff`), as GPUI names it.
pub fn sniff_format(bytes: &[u8]) -> Option<ImageFormat> {
    cerata_core::images::sniff(bytes).map(gpui_format)
}

fn gpui_format(kind: ImageKind) -> ImageFormat {
    match kind {
        ImageKind::Png => ImageFormat::Png,
        ImageKind::Jpeg => ImageFormat::Jpeg,
        ImageKind::Gif => ImageFormat::Gif,
        ImageKind::Webp => ImageFormat::Webp,
        ImageKind::Bmp => ImageFormat::Bmp,
        ImageKind::Tiff => ImageFormat::Tiff,
        ImageKind::Ico => ImageFormat::Ico,
        ImageKind::Svg => ImageFormat::Svg,
    }
}

/// What a load produces.
#[derive(Clone, Debug)]
pub enum Loaded {
    Path(PathBuf),
    /// Decoded from memory: a remote or inline image, or a local SVG
    /// checked before drawing (`file`).
    Bytes {
        image: Arc<Image>,
        file: Option<PathBuf>,
    },
}

/// What an image's actions reach (US-069): its local file, or the bytes
/// of a remote or inline image.
#[derive(Clone, Debug)]
pub enum Reach {
    File(PathBuf),
    Remote { url: String, image: Arc<Image> },
    Inline(Arc<Image>),
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
    /// The library's file names, which find an embed without a walk.
    pub names: Option<NameIndex>,
}

/// A job's url, with the image's natural size and what to draw, if it loaded.
pub type LoadResult = (String, Option<((f32, f32), Loaded)>);

/// A local image's size and what to draw.
fn local_loaded(path: PathBuf) -> Option<((f32, f32), Loaded)> {
    read_prefix(&path, 64 * 1024).and_then(|header| {
        if is_svg(&header) {
            // GPUI would read the file itself: an SVG is checked like an
            // inline or remote one and handed over as bytes.
            let (size, loaded) = read_prefix(&path, MAX_SVG_BYTES).and_then(bytes_loaded)?;
            let Loaded::Bytes { image, .. } = loaded else {
                return None;
            };
            let file = Some(path);
            Some((size, Loaded::Bytes { image, file }))
        } else {
            natural_size(&header).map(|size| (size, Loaded::Path(path)))
        }
    })
}

/// The file an image embed shows, found as Writer finds it
/// (`cerata_core::links::resolve_wiki_image`) and kept inside `confine`.
/// A lookup by name uses the library's name table once it lists every
/// file (US-087); before that, `walk` searches the folder.
fn embed_path(
    target: &str,
    workspace: Option<&Path>,
    note_dir: Option<&Path>,
    confine: Option<&Path>,
    names: Option<&NameIndex>,
    walk: &dyn Fn(&Path, &str) -> Option<PathBuf>,
) -> Option<PathBuf> {
    let slash = |p: &Path| p.to_string_lossy().replace('\\', "/");
    let (root, dir) = (workspace.map(slash), note_dir.map(slash));
    let exists = |p: &str| Path::new(p).is_file();
    let find = |root: &str, name: &str| {
        let found = match names.and_then(|names| names.find(name)) {
            Some(found) => found,
            None => walk(Path::new(root), name),
        };
        found.map(|p| slash(&p))
    };
    let found = cerata_core::links::resolve_wiki_image(
        target,
        root.as_deref(),
        dir.as_deref(),
        &exists,
        &find,
    )?;
    let real = std::fs::canonicalize(&found).ok()?;
    confine
        .is_some_and(|root| real.starts_with(root) && real.is_file())
        .then_some(real)
}

impl LoadJob {
    /// Reads the image's header (and, when remote or inline, its bytes).
    pub fn run(self) -> LoadResult {
        let result = match self.source {
            Source::Local(path) => local_loaded(path),
            Source::Embed {
                target,
                workspace,
                note_dir,
                confine,
            } => embed_path(
                &target,
                workspace.as_deref(),
                note_dir.as_deref(),
                confine.as_deref(),
                self.names.as_ref(),
                &cerata_core::index::find_file_by_name,
            )
            .and_then(local_loaded),
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
    let image = Arc::new(Image::from_bytes(format, bytes));
    Some((size, Loaded::Bytes { image, file: None }))
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
    /// The workspace root, where image embeds are also looked for.
    workspace: Option<PathBuf>,
    /// The workspace's file names, for embeds.
    names: Option<NameIndex>,
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
        self.workspace = Some(root.to_path_buf());
        self.entries.clear();
    }

    /// Embeds are found through the library's name table (US-087).
    pub fn set_names(&mut self, names: NameIndex) {
        self.names = Some(names);
    }

    /// An existing file inside the root, symlinks resolved.
    fn loadable(&self, path: &Path) -> bool {
        let (Some(root), Ok(real)) = (&self.root, std::fs::canonicalize(path)) else {
            return false;
        };
        real.starts_with(root) && real.is_file()
    }

    /// The box of the image at `url` in a column `width` wide, and a load
    /// to start when the image was never seen. Before it is measured (and
    /// when it cannot be read) an image keeps its estimated height: the last
    /// height cached for this source and width, else one text row.
    ///
    /// Without `requested`, an image shows at its natural width, bounded by
    /// the column; with it (`![a|300](x.png)`, US-064), at that width, which
    /// may enlarge it up to the column. A `<img width>` passes its bounded
    /// width as `width` and nothing as `requested`.
    pub fn image_box(
        &mut self,
        url: &str,
        requested: Option<f32>,
        width: f32,
        row_height: f32,
    ) -> (ImageBox, Option<LoadJob>) {
        let source = |host: &Self| match resolve(host.note_dir.as_deref(), url) {
            Some(Source::Local(path)) if !host.loadable(&path) => None,
            source => source,
        };
        self.sized(url, source, requested, width, row_height)
    }

    /// [`ImageHost::image_box`] for an image embed (`![[target]]`), found
    /// in the background; until then it keeps its estimated height, and a
    /// target found nowhere shows its source.
    pub fn embed_box(
        &mut self,
        target: &str,
        requested: Option<f32>,
        width: f32,
        row_height: f32,
    ) -> (ImageBox, Option<LoadJob>) {
        let source = |host: &Self| {
            Some(Source::Embed {
                target: target.to_owned(),
                workspace: host.workspace.clone(),
                note_dir: host.note_dir.clone(),
                confine: host.root.clone(),
            })
        };
        self.sized(&embed_key(target), source, requested, width, row_height)
    }

    /// The natural size of the image at `url` (an embed by its key), once
    /// measured.
    pub fn natural(&self, url: &str) -> Option<(f32, f32)> {
        match self.entries.get(url) {
            Some(Entry::Ready { natural, .. }) => Some(*natural),
            _ => None,
        }
    }

    /// What the actions of the image at `url` reach once it was measured
    /// (US-069): `None` while it is pending or unavailable.
    pub fn reach(&self, url: &str) -> Option<Reach> {
        let Some(Entry::Ready { data, .. }) = self.entries.get(url) else {
            return None;
        };
        Some(match data {
            Loaded::Path(path)
            | Loaded::Bytes {
                file: Some(path), ..
            } => Reach::File(path.clone()),
            Loaded::Bytes { image, file: None }
                if url.to_ascii_lowercase().starts_with("https://") =>
            {
                Reach::Remote {
                    url: url.to_owned(),
                    image: image.clone(),
                }
            }
            Loaded::Bytes { image, file: None } => Reach::Inline(image.clone()),
        })
    }

    /// The image at `url` was measured: it can be resized (US-067).
    pub fn is_ready(&self, url: &str) -> bool {
        self.natural(url).is_some()
    }

    fn sized(
        &mut self,
        url: &str,
        source: impl FnOnce(&Self) -> Option<Source>,
        requested: Option<f32>,
        width: f32,
        row_height: f32,
    ) -> (ImageBox, Option<LoadJob>) {
        let mut job = None;
        if !self.entries.contains_key(url) {
            let entry = match source(self) {
                None => Entry::Unavailable,
                Some(source) => {
                    job = Some(LoadJob {
                        url: url.to_owned(),
                        source,
                        names: self.names.clone(),
                    });
                    Entry::Pending
                }
            };
            self.entries.insert(url.to_owned(), entry);
        }
        let column = width.max(1.0).floor();
        let requested = requested.map(|w| w.min(column).floor().max(1.0));
        // A requested width is the displayed one, known before the image
        // is measured: it keys the cached height, which is then exact.
        let key = (url.to_owned(), requested.unwrap_or(column) as u32);
        let image_box = match self.entries.get(url) {
            Some(Entry::Unavailable) | None => ImageBox::Unavailable,
            Some(Entry::Ready { natural, .. }) => {
                let display = requested.unwrap_or_else(|| natural.0.min(column).floor().max(1.0));
                let height = (natural.1 * display / natural.0).round().max(1.0);
                self.heights.insert(key, height);
                ImageBox::Sized {
                    width: display,
                    height,
                }
            }
            Some(Entry::Pending | Entry::Failed) => ImageBox::Sized {
                width: requested.unwrap_or(column),
                height: self.heights.get(&key).copied().unwrap_or(row_height),
            },
        };
        (image_box, job)
    }

    /// A load finished; `true` when the image's size is now known.
    pub fn finish(&mut self, url: String, result: Option<((f32, f32), Loaded)>) -> bool {
        let ready = result.is_some();
        let embed = url.starts_with(&embed_key(""));
        let entry = match result {
            Some((natural, data)) => Entry::Ready { natural, data },
            // An embed found nowhere shows its source (`ImageEmbedWidget`).
            None if embed => Entry::Unavailable,
            None => Entry::Failed,
        };
        self.entries.insert(url, entry);
        ready
    }

    /// The element drawn for the image at `url`, `width` by `height`.
    pub fn element(&self, url: &str, width: f32, height: f32, id: String) -> AnyElement {
        let name = accessible_name(url);
        let frame = div().id(SharedString::from(id.clone()));
        #[cfg(test)]
        let frame = frame.debug_selector(move || id);
        #[cfg(not(test))]
        let _ = id;
        let frame = frame
            .role(gpui::Role::Image)
            .aria_label(name)
            .w(px(width))
            .h(px(height))
            .rounded(px(4.0));
        match self.entries.get(url) {
            Some(Entry::Ready {
                data: Loaded::Path(path),
                ..
            }) => frame
                .child(img(path.clone()).size_full())
                .into_any_element(),
            Some(Entry::Ready {
                data: Loaded::Bytes { image, .. },
                ..
            }) => frame
                .child(img(image.clone()).size_full())
                .into_any_element(),
            _ => frame.bg(theme::code_background()).into_any_element(),
        }
    }
}

/// A pasted image as an import source, named
/// `YYYYMMDD-HHMMSS-xxxx` in local time (Writer used UTC).
pub fn pasted_source(image: &Image, now: chrono::DateTime<chrono::Local>) -> ImportSource {
    let declared = match image.format {
        ImageFormat::Jpeg => ImageKind::Jpeg,
        ImageFormat::Webp => ImageKind::Webp,
        ImageFormat::Gif => ImageKind::Gif,
        ImageFormat::Svg => ImageKind::Svg,
        ImageFormat::Bmp => ImageKind::Bmp,
        ImageFormat::Tiff => ImageKind::Tiff,
        ImageFormat::Ico => ImageKind::Ico,
        _ => ImageKind::Png,
    };
    let suffix = (image.id ^ now.timestamp_nanos_opt().unwrap_or(0) as u64) & 0xffff;
    ImportSource::Bytes {
        bytes: image.bytes.clone(),
        declared,
        stem: format!("{}-{suffix:04x}", now.format("%Y%m%d-%H%M%S")),
    }
}

/// An image's accessible name: its file name, as a browser names an
/// `<img>` with no alt.
pub(super) fn accessible_name(url: &str) -> String {
    let path = url.strip_prefix(embed_key("").as_str()).unwrap_or(url);
    path.rsplit(['/', '\\']).next().unwrap_or(path).to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn images_are_named_by_their_file() {
        assert_eq!(accessible_name(&embed_key("assets/pic.png")), "pic.png");
        assert_eq!(accessible_name("https://example.com/a/b.jpg"), "b.jpg");
        assert_eq!(accessible_name("C:\\notes\\shot.png"), "shot.png");
        assert_eq!(accessible_name("plain.gif"), "plain.gif");
    }

    /// `\\?\` and `\??\` name device paths: never touched.
    #[test]
    fn device_paths_are_network_paths() {
        assert!(names_network_path(Path::new(r"\\?\UNC\host\share\x.png")));
        assert!(names_network_path(Path::new(r"\??\UNC\host\share\x.png")));
        assert!(!names_network_path(Path::new(r"C:\pics\x.png")));
    }

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
        assert!(host.image_box("in.png", None, 700.0, 30.0).1.is_some());
        let out = outside
            .path()
            .join("out.png")
            .to_string_lossy()
            .into_owned();
        assert_eq!(
            host.image_box(&out, None, 700.0, 30.0).0,
            ImageBox::Unavailable
        );
        assert_eq!(
            host.image_box("../up.png", None, 700.0, 30.0).0,
            ImageBox::Unavailable,
            "outside the note's folder when no workspace is set"
        );
        host.set_root(workspace.path());
        assert!(host.image_box("../up.png", None, 700.0, 30.0).1.is_some());
        let (_, job) = host.image_box("leak.svg", None, 700.0, 30.0);
        let (_, result) = job.expect("inside the root").run();
        assert!(result.is_none(), "its external reference is refused");
    }

    #[test]
    fn a_header_claiming_too_many_pixels_is_not_loaded() {
        let job = LoadJob {
            url: "x".into(),
            source: Source::Data(br#"<svg width="100000" height="100000"></svg>"#.to_vec()),
            names: None,
        };
        assert!(job.run().1.is_none());
    }

    /// A library with images in nested folders, its live index complete.
    /// Returns the folder as opened, its canonical form (where found files
    /// are reported) and the name table.
    fn embed_library() -> (tempfile::TempDir, PathBuf, PathBuf, NameIndex) {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().to_path_buf();
        let mut png = Vec::new();
        image::RgbaImage::new(2, 2)
            .write_to(&mut Cursor::new(&mut png), image::ImageFormat::Png)
            .unwrap();
        for i in 0..140 {
            let folder = root
                .join(format!("area-{}", i % 7))
                .join(format!("t{}", i % 5));
            std::fs::create_dir_all(&folder).unwrap();
            std::fs::write(folder.join(format!("Pic{i}.png")), &png).unwrap();
        }
        std::fs::create_dir_all(root.join("deep").join("er")).unwrap();
        std::fs::write(root.join("deep").join("er").join("same.png"), &png).unwrap();
        std::fs::write(root.join("deep").join("same.png"), &png).unwrap();
        std::fs::write(root.join("note.md"), "# Note").unwrap();
        let token = cerata_core::epoch::EpochCounter::new().token();
        let index = cerata_core::index::index_workspace_with_global(&root, None, &token).unwrap();
        let names = NameIndex::from_index(cerata_core::index::LiveIndex::from_index(&index));
        let real = std::fs::canonicalize(&root).unwrap();
        (dir, root, real, names)
    }

    /// US-087: 140 embeds resolve from the name table, without one walk.
    #[test]
    fn embeds_resolve_by_name_without_walking_the_library() {
        let (_dir, root, real, names) = embed_library();
        let walks = std::cell::Cell::new(0);
        let walk = |root: &Path, name: &str| {
            walks.set(walks.get() + 1);
            cerata_core::index::find_file_by_name(root, name)
        };
        for i in 0..140 {
            let found = embed_path(
                &format!("pic{i}.png"),
                Some(&root),
                Some(&root),
                Some(&real),
                Some(&names),
                &walk,
            );
            let expected = real
                .join(format!("area-{}", i % 7))
                .join(format!("t{}", i % 5))
                .join(format!("Pic{i}.png"));
            assert_eq!(found.as_deref(), Some(expected.as_path()), "pic{i}.png");
        }
        assert_eq!(walks.get(), 0, "an embed walked the library");
        // Duplicates: the fewest components win, as Obsidian decides.
        let same = embed_path(
            "same.png",
            Some(&root),
            Some(&root),
            Some(&real),
            Some(&names),
            &walk,
        );
        assert_eq!(same, Some(real.join("deep").join("same.png")));
        assert_eq!(
            embed_path(
                "missing.png",
                Some(&root),
                Some(&root),
                Some(&real),
                Some(&names),
                &walk
            ),
            None
        );
        assert_eq!(walks.get(), 0);
    }

    /// US-087: before the index lists every file, the walk still finds it.
    #[test]
    fn an_embed_walks_the_library_until_the_index_is_ready() {
        let (_dir, root, real, _) = embed_library();
        let empty = NameIndex::from_index(cerata_core::index::LiveIndex::new(&root));
        let walks = std::cell::Cell::new(0);
        let walk = |root: &Path, name: &str| {
            walks.set(walks.get() + 1);
            cerata_core::index::find_file_by_name(root, name)
        };
        let found = embed_path(
            "pic3.png",
            Some(&root),
            Some(&root),
            Some(&real),
            Some(&empty),
            &walk,
        );
        assert_eq!(found, Some(real.join("area-3").join("t3").join("Pic3.png")));
        assert_eq!(walks.get(), 1);
    }

    /// US-087: an image renamed outside Cerata resolves by its new name
    /// only, once the watcher or a reconciliation saw it.
    #[test]
    fn a_renamed_image_resolves_by_its_new_name_only() {
        let (_dir, root, real, names) = embed_library();
        let old = root.join("area-0").join("t0").join("Pic0.png");
        let new = root.join("area-0").join("t0").join("Renamed.png");
        std::fs::rename(&old, &new).unwrap();
        names.update(|index| {
            index.remove_subtree(&old);
            index.add_other(&new);
        });
        let walk = |_: &Path, _: &str| -> Option<PathBuf> { panic!("no walk once ready") };
        let find = |name: &str| {
            embed_path(
                name,
                Some(&root),
                Some(&root),
                Some(&real),
                Some(&names),
                &walk,
            )
        };
        assert_eq!(find("pic0.png"), None);
        assert_eq!(
            find("renamed.png"),
            Some(real.join("area-0").join("t0").join("Renamed.png"))
        );
        let _ = new;
    }

    #[test]
    fn heights_are_cached_per_source_and_width_and_kept_on_failure() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.png"), b"garbage").unwrap();
        let mut host = ImageHost::new(Some(dir.path().to_path_buf()));
        assert_eq!(
            host.image_box("missing.png", None, 700.0, 30.0).0,
            ImageBox::Unavailable
        );
        let (estimate, job) = host.image_box("bad.png", None, 700.0, 30.0);
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
        let (again, job) = host.image_box("bad.png", None, 700.0, 30.0);
        assert_eq!(again, estimate);
        assert!(job.is_none());
    }
}
