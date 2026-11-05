use std::path::Path;

pub const MARKDOWN_EXTENSIONS: [&str; 2] = ["md", "markdown"];

pub fn is_markdown_path(path: &Path) -> bool {
    path.extension()
        .and_then(|ext| ext.to_str())
        .is_some_and(|ext| {
            MARKDOWN_EXTENSIONS
                .iter()
                .any(|known| ext.eq_ignore_ascii_case(known))
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_both_extensions_in_any_case() {
        for name in [
            "a.md",
            "a.MD",
            "a.Md",
            "a.markdown",
            "a.MarkDown",
            "dir/b.MARKDOWN",
        ] {
            assert!(is_markdown_path(Path::new(name)), "{name}");
        }
        for name in ["a.mdx", "a.txt", "md", ".md", "a.md.bak", "a"] {
            assert!(!is_markdown_path(Path::new(name)), "{name}");
        }
    }
}
