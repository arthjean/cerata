use super::*;
use cerata_markdown::EditorState;

#[test]
fn at_least_thirty_languages_compile_and_color_code() {
    let samples: &[(&str, &str)] = &[
        ("javascript", "const x = 1; // c"),
        ("typescript", "let x: number = 1;"),
        ("tsx", "const a = <div>{1}</div>;"),
        ("python", "def f():\n    return 'x'"),
        ("bash", "echo \"hi\" # c"),
        ("json", "{\"a\": 1}"),
        ("rust", "fn main() { let x = 1; }"),
        ("html", "<p class=\"a\">x</p>"),
        ("css", "a { color: red; }"),
        ("yaml", "a: 1 # c"),
        ("sql", "SELECT 1 FROM t;"),
        ("go", "func main() { return }"),
        ("java", "class A { int x = 1; }"),
        ("c", "int main(void) { return 0; }"),
        ("cpp", "int main() { return 0; }"),
        ("csharp", "class A { int x = 1; }"),
        ("php", "<?php echo 'x'; ?>"),
        ("ruby", "def f; 'x'; end"),
        ("swift", "let x = 1"),
        ("toml", "a = 1 # c"),
        ("xml", "<a b=\"c\">d</a>"),
        ("diff", "+added\n-removed"),
        ("lua", "local x = 'a' -- c"),
        ("scala", "object A { val x = 1 }"),
        ("haskell", "main = putStrLn \"x\""),
        ("r", "x <- 'a' # c"),
        ("dart", "void main() { var x = 'a'; }"),
        ("elixir", "def f, do: :ok"),
        ("ocaml", "let x = \"a\""),
        ("zig", "const x: u8 = 1;"),
        ("make", "all:\n\techo x # c"),
        ("nix", "{ a = \"b\"; }"),
        ("elm", "main = text \"x\""),
        ("regex", "a+[b-c]"),
    ];
    assert!(samples.len() >= 30);
    assert_eq!(languages().count(), samples.len());
    for (language, code) in samples {
        let spans = highlight(language, code).unwrap_or_else(|| panic!("{language}: no grammar"));
        assert!(!spans.is_empty(), "{language}: no colored span in {code:?}");
        for span in &spans {
            assert!(
                span.range.end <= code.len() && span.range.start < span.range.end,
                "{language}"
            );
        }
    }
}

#[test]
fn fence_info_strings_select_grammars_by_alias() {
    assert_eq!(language_for("js"), Some("javascript"));
    assert_eq!(language_for("RS"), Some("rust"));
    assert_eq!(language_for("py title=x"), Some("python"));
    assert_eq!(language_for("{.yml}"), Some("yaml"));
    assert_eq!(language_for(""), None);
}

#[test]
fn an_unknown_language_stays_plain_without_error() {
    assert_eq!(highlight("foo", "anything at all"), None);
    let state = EditorState::new("```foo\nlet x = 1;\n```\n");
    assert!(highlight_code_blocks(&state).is_empty());
    // The block is still drawn as code.
    let lines = state
        .decorations()
        .into_iter()
        .filter(|d| matches!(d, cerata_markdown::Decoration::Line { style, .. } if style.is_monospace()))
        .count();
    assert_eq!(lines, 3);
}

#[test]
fn highlights_fences_in_document_offsets() {
    let doc = "text\n\n```rust\nfn main() {}\n```\n";
    let state = EditorState::new(doc);
    let spans = highlight_code_blocks(&state);
    let keyword = spans
        .iter()
        .find(|s| s.kind == HighlightKind::Keyword)
        .unwrap();
    assert_eq!(&doc[keyword.range.clone()], "fn");
}
