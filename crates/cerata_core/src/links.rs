//! Where a Markdown link leads (US-033). Ported from `lib/paths.ts:18-183`
//! (`resolveLinkTarget`), on `/`-separated path strings like Writer's; the
//! caller turns an internal path into a `PathBuf`.
//!
//! Change from Writer: an absolute extensionless link (`/docs/guide`) is
//! tried under the workspace root before the filesystem root.

/// What following a link does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LinkTarget {
    /// `#anchor`: scroll the current note.
    SameDocAnchor(String),
    /// A note of the workspace, opened in the app.
    Internal {
        path: String,
        anchor: Option<String>,
    },
    /// `scheme:...`: handed to the OS when the scheme is allowed.
    ExternalUrl(String),
    /// Any other local path: handed to the OS after a confirmation.
    ExternalPath(String),
}

fn is_windows_absolute(path: &str) -> bool {
    let b = path.as_bytes();
    b.len() >= 3 && b[0].is_ascii_alphabetic() && b[1] == b':' && (b[2] == b'\\' || b[2] == b'/')
}

fn has_scheme(href: &str) -> bool {
    let Some(colon) = href.find(':') else {
        return false;
    };
    let scheme = &href[..colon];
    scheme
        .chars()
        .next()
        .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
}

/// `decodeURIComponent`, keeping the input when it is not valid UTF-8 once
/// decoded or holds a malformed escape.
pub fn decode_link_path(path: &str) -> String {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = path
                .get(i + 1..i + 3)
                .and_then(|h| u8::from_str_radix(h, 16).ok());
            match hex {
                Some(b) => {
                    out.push(b);
                    i += 3;
                    continue;
                }
                None => return path.to_owned(),
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8(out).unwrap_or_else(|_| path.to_owned())
}

/// `normalizePath`: `/` separators, `.` and `..` resolved.
pub fn normalize_path(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    let windows = is_windows_absolute(&normalized);
    let leading = normalized.starts_with('/');
    let mut stack: Vec<&str> = Vec::new();
    for part in normalized.split('/') {
        if part.is_empty() || part == "." {
            continue;
        }
        if part == ".." {
            if stack.last().is_some_and(|last| *last != "..") {
                stack.pop();
                continue;
            }
            if !windows && !leading {
                stack.push(part);
            }
            continue;
        }
        stack.push(part);
    }
    if windows {
        let (drive, rest) = stack.split_first().map_or(("", &[][..]), |(d, r)| (*d, r));
        return if rest.is_empty() {
            format!("{drive}/")
        } else {
            format!("{drive}/{}", rest.join("/"))
        };
    }
    if leading {
        return format!("/{}", stack.join("/"));
    }
    stack.join("/")
}

fn parent_dir(path: &str) -> String {
    let normalized = path.replace('\\', "/");
    match normalized.rfind('/') {
        Some(0) => "/".to_owned(),
        Some(i) => normalized[..i].to_owned(),
        None => String::new(),
    }
}

fn resolve_path(base_dir: &str, target: &str) -> String {
    if is_windows_absolute(target) || target.starts_with('/') {
        return normalize_path(target);
    }
    normalize_path(&format!("{}/{target}", base_dir.trim_end_matches('/')))
}

/// Inside the root; compared case-insensitively on Windows, where the file
/// system is.
pub fn is_inside_root(path: &str, root: &str) -> bool {
    let (mut path, mut root) = (normalize_path(path), normalize_path(root));
    if cfg!(windows) {
        path = path.to_lowercase();
        root = root.to_lowercase();
    }
    let root = root.trim_end_matches('/');
    path == root || path.starts_with(&format!("{root}/"))
}

/// The last component of `path`, `/` or `\` separated (`getFileName`).
pub fn file_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

/// The extension of `path`'s name, without its dot; none for a dotfile
/// (`getFileExtension`).
pub fn file_extension(path: &str) -> &str {
    let name = file_name(path);
    match name.rfind('.') {
        Some(0) | None => "",
        Some(i) => &name[i + 1..],
    }
}

/// `path`'s name without its extension; a dotfile keeps its whole name
/// (`getFileStem`).
pub fn file_stem(path: &str) -> &str {
    let name = file_name(path);
    match name.rfind('.') {
        Some(0) | None => name,
        Some(i) => &name[..i],
    }
}

/// `full` relative to `root` with `/` separators, or `full` itself (with
/// `/` separators) outside it (`getRelativePath`).
pub fn relative_path(full: &str, root: &str) -> String {
    let full = full.replace('\\', "/");
    let root = root.replace('\\', "/");
    let root = root.trim_end_matches('/');
    match full.strip_prefix(&format!("{root}/")) {
        Some(relative) => relative.to_owned(),
        None => full,
    }
}

fn extension(path: &str) -> String {
    file_extension(path).to_ascii_lowercase()
}

fn anchor_of(href: &str) -> Option<String> {
    let anchor = &href[href.find('#')? + 1..];
    (!anchor.is_empty()).then(|| decode_link_path(anchor))
}

/// Resolves `href` written in the note at `current` (`resolveLinkTarget`).
/// `exists` probes candidates for an extensionless link. `None` for an
/// empty link or a bare `#`.
pub fn resolve(
    href: &str,
    current: &str,
    root: Option<&str>,
    exists: &dyn Fn(&str) -> bool,
) -> Option<LinkTarget> {
    let trimmed = cerata_markdown::decorate::normalize_markdown_destination(href);
    if trimmed.is_empty() {
        return None;
    }
    if trimmed.starts_with('#') {
        return anchor_of(&trimmed).map(LinkTarget::SameDocAnchor);
    }
    if has_scheme(&trimmed) && !is_windows_absolute(&trimmed) {
        return Some(LinkTarget::ExternalUrl(trimmed));
    }
    let split = trimmed.find(['#', '?']).unwrap_or(trimmed.len());
    let target = &trimmed[..split];
    if target.is_empty() {
        return None;
    }
    let anchor = anchor_of(&trimmed);
    let decoded = decode_link_path(target);
    let resolved = resolve_path(&parent_dir(current), &decoded);
    let ext = extension(&resolved);
    let inside = |p: &str| root.is_none_or(|r| is_inside_root(p, r));
    if (ext == "md" || ext == "markdown") && inside(&resolved) {
        return Some(LinkTarget::Internal {
            path: resolved,
            anchor,
        });
    }
    if ext.is_empty() {
        let mut bases = Vec::new();
        let posix_absolute = decoded.starts_with('/') && !is_windows_absolute(&decoded);
        if posix_absolute && let Some(root) = root {
            let mapped = normalize_path(&format!("{}{decoded}", root.trim_end_matches('/')));
            bases.push(mapped.trim_end_matches('/').to_owned());
        }
        let own = resolved.trim_end_matches('/').to_owned();
        if !bases.contains(&own) {
            bases.push(own);
        }
        for base in bases {
            for suffix in [
                ".md",
                ".markdown",
                "/index.md",
                "/index.markdown",
                "/README.md",
            ] {
                let candidate = format!("{base}{suffix}");
                if root.is_some_and(|r| !is_inside_root(&candidate, r)) {
                    continue;
                }
                if exists(&candidate) {
                    return Some(LinkTarget::Internal {
                        path: candidate,
                        anchor,
                    });
                }
            }
        }
    }
    Some(LinkTarget::ExternalPath(resolved))
}

/// The note a wiki link names (`resolveWikiLink`, `lib/wiki-links.ts`):
/// a path with a `/` is taken from the root (`.md`, then `.markdown`); a
/// bare name must match the stem of exactly one note, `stems` listing the
/// notes whose name contains it. `#fragment` alone means the current note.
pub fn resolve_wiki(
    raw: &str,
    root: &str,
    current: Option<&str>,
    exists: &dyn Fn(&str) -> bool,
    stems: &dyn Fn(&str) -> Vec<String>,
) -> Option<String> {
    let link = cerata_markdown::wiki::parse(raw);
    let target = link.path;
    if target.is_empty() {
        return link.fragment.and(current.map(str::to_owned));
    }
    if target.contains('/') {
        let base = normalize_path(&format!("{}/{target}", root.trim_end_matches('/')));
        return [".md", ".markdown"]
            .iter()
            .map(|ext| format!("{base}{ext}"))
            .find(|candidate| is_inside_root(candidate, root) && exists(candidate));
    }
    let wanted = target.to_lowercase();
    let matches: Vec<String> = stems(&target)
        .into_iter()
        .filter(|path| file_stem(path).to_lowercase() == wanted)
        .collect();
    match matches.as_slice() {
        [only] => Some(only.clone()),
        _ => None,
    }
}

/// The file an image embed (`![[target]]`) shows (`resolveWikiImage`): a
/// target with a `/` is tried from the root, then from the note's folder;
/// a bare name from the note's folder, then the root, then by name
/// anywhere in the workspace (`find`, Obsidian's shortest-path links).
/// Without a workspace, only the note's folder. Never outside the root.
pub fn resolve_wiki_image(
    target: &str,
    root: Option<&str>,
    note_dir: Option<&str>,
    exists: &dyn Fn(&str) -> bool,
    find: &dyn Fn(&str, &str) -> Option<String>,
) -> Option<String> {
    let root = root.map(|r| r.trim_end_matches(['/', '\\']));
    let mut candidates: Vec<String> = Vec::new();
    let mut push = |base: Option<&str>| {
        if let Some(base) = base {
            let path = normalize_path(&format!("{base}/{target}"));
            if !candidates.contains(&path) {
                candidates.push(path);
            }
        }
    };
    if target.contains('/') {
        push(root);
        push(note_dir);
    } else {
        push(note_dir);
        push(root);
    }
    let inside = |p: &str| root.is_none_or(|r| is_inside_root(p, r));
    if let Some(found) = candidates.into_iter().find(|c| inside(c) && exists(c)) {
        return Some(found);
    }
    match root {
        Some(root) if !target.contains('/') => find(root, target).filter(|p| inside(p)),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> bool {
        false
    }

    fn internal(path: &str, anchor: Option<&str>) -> Option<LinkTarget> {
        Some(LinkTarget::Internal {
            path: path.into(),
            anchor: anchor.map(Into::into),
        })
    }

    /// `paths.test.ts:114-298`.
    #[test]
    fn resolves_like_writer() {
        let r = |href: &str, current: &str, root: &str| resolve(href, current, Some(root), &none);
        assert_eq!(
            r("../ideas/next.md", "/vault/daily/today.md", "/vault"),
            internal("/vault/ideas/next.md", None)
        );
        assert_eq!(
            r("./guide.md?view=1#intro", "/vault/docs/start.md", "/vault"),
            internal("/vault/docs/guide.md", Some("intro"))
        );
        assert_eq!(
            r("./My%20Guide.md", "/vault/docs/start.md", "/vault"),
            internal("/vault/docs/My Guide.md", None)
        );
        assert_eq!(
            r("<./My Guide.md>", "/vault/docs/start.md", "/vault"),
            internal("/vault/docs/My Guide.md", None)
        );
        assert_eq!(
            r("./My\\ Guide.md", "/vault/docs/start.md", "/vault"),
            internal("/vault/docs/My Guide.md", None)
        );
        assert_eq!(
            r("#intro", "/vault/docs/start.md", "/vault"),
            Some(LinkTarget::SameDocAnchor("intro".into()))
        );
        assert_eq!(r("#", "/vault/docs/start.md", "/vault"), None);
        assert_eq!(
            r("#hello%20world", "/vault/docs/start.md", "/vault"),
            Some(LinkTarget::SameDocAnchor("hello world".into()))
        );
        assert_eq!(
            r("./guide.md#setup", "/vault/docs/start.md", "/vault"),
            internal("/vault/docs/guide.md", Some("setup"))
        );
        assert_eq!(
            r("https://example.com/docs", "/vault/docs/start.md", "/vault"),
            Some(LinkTarget::ExternalUrl("https://example.com/docs".into()))
        );
        assert_eq!(
            r("./guide.pdf", "/vault/docs/start.md", "/vault"),
            Some(LinkTarget::ExternalPath("/vault/docs/guide.pdf".into()))
        );
        assert_eq!(
            r("../outside.md", "/vault/docs/start.md", "/vault/docs"),
            Some(LinkTarget::ExternalPath("/vault/outside.md".into()))
        );
        assert_eq!(
            r(
                "C:\\vault\\notes\\other.md",
                "C:\\vault\\notes\\start.md",
                "C:/vault"
            ),
            internal("C:/vault/notes/other.md", None)
        );
        assert_eq!(
            r("/tmp/scratch/note.md", "/vault/docs/start.md", "/vault"),
            Some(LinkTarget::ExternalPath("/tmp/scratch/note.md".into()))
        );
    }

    /// `paths.test.ts` "extensionless link resolution", with the absolute
    /// path tried under the root first.
    #[test]
    fn extensionless_links_probe_markdown_candidates() {
        let existing = [
            "/vault/docs/guide.md",
            "/vault/docs/foo/index.md",
            "/vault/docs/bar.md",
            "/vault/docs/bar/index.md",
            "/vault/docs/readme-only/README.md",
        ];
        let exists = |p: &str| existing.contains(&p);
        let r = |href: &str| resolve(href, "/vault/docs/start.md", Some("/vault"), &exists);
        assert_eq!(r("./guide"), internal("/vault/docs/guide.md", None));
        assert_eq!(r("./bar/"), internal("/vault/docs/bar.md", None));
        assert_eq!(r("./foo"), internal("/vault/docs/foo/index.md", None));
        assert_eq!(r("./foo/"), internal("/vault/docs/foo/index.md", None));
        assert_eq!(
            r("./readme-only"),
            internal("/vault/docs/readme-only/README.md", None)
        );
        assert_eq!(
            r("./missing"),
            Some(LinkTarget::ExternalPath("/vault/docs/missing".into()))
        );
        assert_eq!(
            r("./image.png"),
            Some(LinkTarget::ExternalPath("/vault/docs/image.png".into()))
        );
        assert_eq!(
            r("./guide#heading"),
            internal("/vault/docs/guide.md", Some("heading"))
        );
        assert_eq!(
            resolve(
                "/docs/guide",
                "/vault/daily/today.md",
                Some("/vault"),
                &exists
            ),
            internal("/vault/docs/guide.md", None)
        );
        let outside = |p: &str| p == "/outside/doc.md";
        assert_eq!(
            resolve(
                "../../outside/doc",
                "/vault/docs/start.md",
                Some("/vault"),
                &outside
            ),
            Some(LinkTarget::ExternalPath("/outside/doc".into()))
        );
        assert_eq!(
            resolve("./guide", "/vault/docs/start.md", Some("/vault"), &none),
            Some(LinkTarget::ExternalPath("/vault/docs/guide".into()))
        );
        // Under the root first, the filesystem path second.
        let both = |p: &str| p == "/vault/vault/x.md" || p == "/vault/x.md";
        assert_eq!(
            resolve("/vault/x", "/vault/a.md", Some("/vault"), &both),
            internal("/vault/vault/x.md", None)
        );
    }

    #[test]
    fn normalizes_paths() {
        assert_eq!(normalize_path("/a/./b/../c"), "/a/c");
        assert_eq!(normalize_path("a\\b"), "a/b");
        assert_eq!(normalize_path("../a"), "../a");
        assert_eq!(normalize_path("C:\\x\\..\\y"), "C:/y");
    }

    #[test]
    fn wiki_links_resolve_by_path_or_unique_stem() {
        let exists = |p: &str| p == "/vault/notes/plan.md";
        let stems = |q: &str| -> Vec<String> {
            let all = [
                "/vault/Roadmap.md",
                "/vault/a/Twin.md",
                "/vault/b/twin.md",
                "/vault/Roadmap 2.md",
            ];
            all.iter()
                .filter(|p| p.to_lowercase().contains(&q.to_lowercase()))
                .map(|p| p.to_string())
                .collect()
        };
        let r = |raw: &str| resolve_wiki(raw, "/vault", Some("/vault/here.md"), &exists, &stems);
        assert_eq!(r("notes/plan").as_deref(), Some("/vault/notes/plan.md"));
        assert_eq!(
            r("notes/plan.md|alias").as_deref(),
            Some("/vault/notes/plan.md")
        );
        assert_eq!(r("notes/missing"), None);
        assert_eq!(r("roadmap").as_deref(), Some("/vault/Roadmap.md"));
        assert_eq!(r("Roadmap#Goals").as_deref(), Some("/vault/Roadmap.md"));
        assert_eq!(r("Twin"), None, "two notes share the stem");
        assert_eq!(r("Nowhere"), None);
        assert_eq!(r("#Goals").as_deref(), Some("/vault/here.md"));
        assert_eq!(r("../../etc/passwd"), None);
    }

    /// `paths.test.ts:14-72`.
    #[test]
    fn names_stems_extensions_and_relative_paths() {
        assert_eq!(file_extension("/path/to/file.md"), "md");
        assert_eq!(file_extension("test.txt"), "txt");
        assert_eq!(file_extension("Makefile"), "");
        assert_eq!(file_extension("/path/to/file"), "");
        assert_eq!(file_extension("file.test.ts"), "ts");
        assert_eq!(file_extension(".gitignore"), "");
        assert_eq!(file_extension("/path/.env"), "");
        assert_eq!(file_extension(r"C:\dir.d\file"), "");

        assert_eq!(file_stem("/path/to/file.md"), "file");
        assert_eq!(file_stem("test.txt"), "test");
        assert_eq!(file_stem("Makefile"), "Makefile");
        assert_eq!(file_stem("file.test.ts"), "file.test");
        assert_eq!(file_stem(".gitignore"), ".gitignore");

        assert_eq!(file_name("/path/to/file.md"), "file.md");
        assert_eq!(file_name("file.md"), "file.md");
        assert_eq!(file_name(r"C:\Users\test\file.md"), "file.md");

        assert_eq!(
            relative_path("/home/user/docs/file.md", "/home/user"),
            "docs/file.md"
        );
        assert_eq!(
            relative_path("/other/file.md", "/home/user"),
            "/other/file.md"
        );
        assert_eq!(
            relative_path("/home/user/file.md", "/home/user/"),
            "file.md"
        );
        assert_eq!(relative_path(r"C:\vault\a\b.md", r"C:\vault"), "a/b.md");
    }

    /// `wiki-links.test.ts:108-316`, one case per Writer test.
    #[test]
    fn wiki_links_resolve_like_writer() {
        let notes = [
            "/vault/Roadmap.md",
            "/vault/Notes.md",
            "/vault/SomeOther.md",
            "/vault/Adventurer/No prior experience.md",
            "/vault/Formatting/Format your notes.md",
            "/vault/Formatting/Callout.md",
            "/vault/Other/Callout.md",
        ];
        let stems = |q: &str| -> Vec<String> {
            notes
                .iter()
                .filter(|p| p.to_lowercase().contains(&q.to_lowercase()))
                .map(|p| p.to_string())
                .collect()
        };
        let present = [
            "/vault/planning/Roadmap.md",
            "/vault/Project Notes/My Note.md",
        ];
        let exists = |p: &str| present.contains(&p);
        let r = |raw: &str| {
            resolve_wiki(
                raw,
                "/vault",
                Some("/vault/Vault is just a local folder.md"),
                &exists,
                &stems,
            )
        };
        assert_eq!(r("Roadmap").as_deref(), Some("/vault/Roadmap.md"));
        assert_eq!(r("roadmap").as_deref(), Some("/vault/Roadmap.md"));
        assert_eq!(
            r("planning/Roadmap").as_deref(),
            Some("/vault/planning/Roadmap.md")
        );
        assert_eq!(
            r("Project Notes/My Note|Label With Spaces").as_deref(),
            Some("/vault/Project Notes/My Note.md")
        );
        assert_eq!(r("planning/Missing"), None);
        assert_eq!(r("Notes.md").as_deref(), Some("/vault/Notes.md"));
        assert_eq!(r("Notes.markdown").as_deref(), Some("/vault/Notes.md"));
        assert_eq!(r("  "), None);
        assert_eq!(r("Missing"), None);
        assert_eq!(
            r("No prior experience|I have no prior experience").as_deref(),
            Some("/vault/Adventurer/No prior experience.md")
        );
        assert_eq!(
            r("Format your notes#^376b9d|second option").as_deref(),
            Some("/vault/Formatting/Format your notes.md")
        );
        assert_eq!(
            r("#^0f681f|with great power comes great responsibility").as_deref(),
            Some("/vault/Vault is just a local folder.md")
        );
        assert_eq!(r("Callout"), None, "two notes share the stem");
        let qualified = |p: &str| p == "/vault/Formatting/Callout.md";
        assert_eq!(
            resolve_wiki("Formatting/Callout", "/vault", None, &qualified, &stems).as_deref(),
            Some("/vault/Formatting/Callout.md")
        );
    }

    /// `wiki-links.test.ts:428-510`.
    #[test]
    fn image_embeds_resolve_like_writer() {
        let no_find = |_: &str, _: &str| -> Option<String> { panic!("no name search") };
        let only = |path: &'static str| move |p: &str| p == path;
        let note_dir = Some("/vault/notes");
        assert_eq!(
            resolve_wiki_image(
                "assets/pic.png",
                Some("/vault"),
                note_dir,
                &only("/vault/assets/pic.png"),
                &no_find
            )
            .as_deref(),
            Some("/vault/assets/pic.png")
        );
        assert_eq!(
            resolve_wiki_image(
                "assets/pic.png",
                Some("/vault"),
                note_dir,
                &only("/vault/notes/assets/pic.png"),
                &no_find
            )
            .as_deref(),
            Some("/vault/notes/assets/pic.png")
        );
        assert_eq!(
            resolve_wiki_image(
                "pic.png",
                Some("/vault"),
                note_dir,
                &only("/vault/notes/pic.png"),
                &no_find
            )
            .as_deref(),
            Some("/vault/notes/pic.png")
        );
        let find = |root: &str, name: &str| {
            (root == "/vault" && name == "pic.png")
                .then(|| "/vault/deep/attachments/pic.png".into())
        };
        assert_eq!(
            resolve_wiki_image("pic.png", Some("/vault"), note_dir, &none, &find).as_deref(),
            Some("/vault/deep/attachments/pic.png")
        );
        assert_eq!(
            resolve_wiki_image("assets/pic.png", Some("/vault"), None, &none, &no_find),
            None
        );
        assert_eq!(
            resolve_wiki_image(
                "pic.png",
                None,
                Some("/anywhere"),
                &only("/anywhere/pic.png"),
                &no_find
            )
            .as_deref(),
            Some("/anywhere/pic.png")
        );
        // Never outside the workspace.
        let anything = |_: &str| true;
        assert_eq!(
            resolve_wiki_image(
                "../../etc/x.png",
                Some("/vault"),
                note_dir,
                &anything,
                &no_find
            ),
            None
        );
        let outside = |_: &str, _: &str| Some("/elsewhere/pic.png".to_owned());
        assert_eq!(
            resolve_wiki_image("pic.png", Some("/vault"), note_dir, &none, &outside),
            None
        );
    }
}
