//! Wiki link parsing: `[[target#fragment|alias]]`, with `\|` escaping the
//! pipe inside tables. Ported from `lib/wiki-links.ts:14-75`.

/// A wiki link's inner text, split.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WikiLink {
    /// The target with `\|` unescaped, fragment included.
    pub target: String,
    /// The target path, normalized by [`normalize_target`].
    pub path: String,
    pub fragment: Option<String>,
    pub alias: Option<String>,
    /// What the folded link shows: the alias, else the path, else
    /// `#fragment`.
    pub display: String,
}

fn unescape(text: &str) -> String {
    text.replace("\\|", "|").trim().to_owned()
}

/// Parses the text between `[[` and `]]` (`parseWikiLink`).
pub fn parse(raw: &str) -> WikiLink {
    let raw = raw.trim();
    let (target, alias) = match raw.find('|') {
        None => (raw, None),
        Some(separator) => {
            let escaped = separator > 0 && raw.as_bytes()[separator - 1] == b'\\';
            let target_end = if escaped { separator - 1 } else { separator };
            (&raw[..target_end], Some(unescape(&raw[separator + 1..])))
        }
    };
    let target = unescape(target);
    let (path, fragment) = match target.find('#') {
        None => (target.as_str(), None),
        Some(hash) => (&target[..hash], Some(target[hash + 1..].to_owned())),
    };
    let path = normalize_target(path);
    let alias = alias.filter(|a| !a.is_empty());
    let display = match (&alias, &fragment) {
        (Some(alias), _) => alias.clone(),
        (None, _) if !path.is_empty() => path.clone(),
        (None, Some(fragment)) => format!("#{fragment}"),
        (None, None) => target.clone(),
    };
    WikiLink {
        target: target.clone(),
        path,
        fragment,
        alias,
        display,
    }
}

/// Trims, turns `\` into `/`, drops leading slashes and a `.md` or
/// `.markdown` extension (`normalizeWikiTarget`).
pub fn normalize_target(raw: &str) -> String {
    let target = raw.trim().replace('\\', "/");
    let target = target.trim_start_matches('/');
    let lower = target.to_ascii_lowercase();
    if lower.ends_with(".md") {
        target[..target.len() - 3].to_owned()
    } else if lower.ends_with(".markdown") {
        target[..target.len() - 9].to_owned()
    } else {
        target.to_owned()
    }
}

/// Image formats an Obsidian embed (`![[image.png]]`) shows
/// (`SPECs/obsidian-image-embed-spec.md`).
const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "gif", "webp", "svg"];

/// The image path an embed's inner text names, slashes normalized and
/// leading `/` dropped; `None` for a note, a PDF or anything else. What
/// follows an unescaped `|` (alias, size) is ignored
/// (`parseWikiImageEmbedTarget`).
pub fn image_embed_target(raw: &str) -> Option<String> {
    let raw = raw.trim();
    let target = match raw.find('|') {
        Some(i) if i > 0 && raw.as_bytes()[i - 1] == b'\\' => &raw[..i - 1],
        Some(i) => &raw[..i],
        None => raw,
    };
    let path = unescape(target).replace('\\', "/");
    let path = path.trim_start_matches('/');
    let dot = path.rfind('.')?;
    if dot == 0 || dot == path.len() - 1 {
        return None;
    }
    let ext = path[dot + 1..].to_ascii_lowercase();
    IMAGE_EXTENSIONS
        .contains(&ext.as_str())
        .then(|| path.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `wiki-links.test.ts:16-50`.
    #[test]
    fn normalizes_targets() {
        for (raw, normalized) in [
            ("  Roadmap  ", "Roadmap"),
            ("planning\\Roadmap", "planning/Roadmap"),
            ("Roadmap.md", "Roadmap"),
            ("Roadmap.markdown", "Roadmap"),
            ("Roadmap.MD", "Roadmap"),
            ("Roadmap.Markdown", "Roadmap"),
            ("Roadmap", "Roadmap"),
            ("  planning\\Roadmap.md  ", "planning/Roadmap"),
            ("   ", ""),
            ("\\dir\\Note.MARKDOWN", "dir/Note"),
        ] {
            assert_eq!(normalize_target(raw), normalized, "{raw:?}");
        }
    }

    /// `wiki-links.test.ts:55-91`.
    #[test]
    fn splits_alias_fragment_and_escaped_pipe() {
        let link = parse("No prior experience|I have no prior experience");
        assert_eq!(link.path, "No prior experience");
        assert_eq!(link.fragment, None);
        assert_eq!(link.alias.as_deref(), Some("I have no prior experience"));
        assert_eq!(link.display, "I have no prior experience");

        let link = parse("Format your notes#^376b9d|second option");
        assert_eq!(link.path, "Format your notes");
        assert_eq!(link.fragment.as_deref(), Some("^376b9d"));
        assert_eq!(link.display, "second option");

        let link = parse("#^0f681f|with great power comes great responsibility");
        assert_eq!(link.path, "");
        assert_eq!(link.fragment.as_deref(), Some("^0f681f"));
        assert_eq!(link.display, "with great power comes great responsibility");

        let link = parse("Format your notes\\|Formatting");
        assert_eq!(link.path, "Format your notes");
        assert_eq!(link.alias.as_deref(), Some("Formatting"));
        assert_eq!(link.display, "Formatting");

        let link = parse("Notes/Plan.md#Goals");
        assert_eq!(link.path, "Notes/Plan");
        assert_eq!(link.fragment.as_deref(), Some("Goals"));
        assert_eq!(link.display, "Notes/Plan");

        assert_eq!(parse("#Only a heading").display, "#Only a heading");
        assert_eq!(parse("a|").display, "a");
    }

    /// `wiki-links.test.ts:396-425`.
    #[test]
    fn image_embeds_name_an_image_file() {
        for (raw, target) in [
            ("diagram.png", "diagram.png"),
            ("Photo.JPEG", "Photo.JPEG"),
            ("anim.gif", "anim.gif"),
            ("icon.svg", "icon.svg"),
            ("assets\\pic.png", "assets/pic.png"),
            ("/assets/pic.png", "assets/pic.png"),
            ("pic.png|400", "pic.png"),
            ("pic.png|alt text", "pic.png"),
            ("pic.png\\|400", "pic.png"),
        ] {
            assert_eq!(image_embed_target(raw).as_deref(), Some(target), "{raw:?}");
        }
        for raw in [
            "Some Note",
            "Some Note.md",
            "report.pdf",
            ".png",
            "dot.",
            "",
        ] {
            assert_eq!(image_embed_target(raw), None, "{raw:?}");
        }
    }
}
