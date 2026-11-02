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

/// An image embed's inner text, read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageEmbed {
    /// The image path, slashes normalized and leading `/` dropped.
    pub path: String,
    /// The size on its last `|` segment (`image_size::embed_size`).
    pub size: Option<crate::image_size::ImageSize>,
}

/// The image an embed's inner text names, with its size; `None` for a
/// note, a PDF or anything else. An alias after an unescaped `|` is
/// ignored (`parseWikiImageEmbedTarget`).
pub fn image_embed_target(raw: &str) -> Option<ImageEmbed> {
    let size = crate::image_size::embed_size(raw).map(|s| s.size);
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
        .then(|| ImageEmbed {
            path: path.to_owned(),
            size,
        })
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

    /// `wiki-links.test.ts:396-425`; the size is kept (US-063).
    #[test]
    fn image_embeds_name_an_image_file() {
        for (raw, target, width) in [
            ("diagram.png", "diagram.png", None),
            ("Photo.JPEG", "Photo.JPEG", None),
            ("anim.gif", "anim.gif", None),
            ("icon.svg", "icon.svg", None),
            ("assets\\pic.png", "assets/pic.png", None),
            ("/assets/pic.png", "assets/pic.png", None),
            ("pic.png|400", "pic.png", Some(400)),
            ("pic.png|alt text", "pic.png", None),
            ("pic.png\\|400", "pic.png", Some(400)),
            ("pic.png|alt|300x200", "pic.png", Some(300)),
            ("pic.png|400px", "pic.png", None),
        ] {
            let embed = image_embed_target(raw);
            assert_eq!(
                embed.as_ref().map(|e| e.path.as_str()),
                Some(target),
                "{raw:?}"
            );
            assert_eq!(
                embed.and_then(|e| e.size).map(|s| s.width),
                width,
                "{raw:?}"
            );
        }
        assert_eq!(
            image_embed_target("pic.png|300x200").and_then(|e| e.size),
            Some(crate::image_size::ImageSize {
                width: 300,
                height: Some(200)
            }),
            "the height is read"
        );
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
