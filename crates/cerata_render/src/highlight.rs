//! Code highlighting with tree-sitter grammars (US-014), in the color roles
//! of Writer's `generalSyntaxHighlights` (`syntaxHighlighting.ts:214-273`).
//! A fence whose language has no grammar renders in monospace without
//! colors: [`highlight`] returns `None`, never an error.

use std::ops::Range;
use std::sync::OnceLock;

use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// Writer's code color roles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum HighlightKind {
    Link,
    Keyword,
    /// Atoms, booleans, URLs, labels.
    Atom,
    /// Numbers and other literals.
    Literal,
    String,
    /// Regular expressions, escapes, special strings.
    Regexp,
    DefinitionVariable,
    LocalVariable,
    /// Types and namespaces.
    TypeName,
    ClassName,
    /// Built-in variables and macros.
    Macro,
    DefinitionProperty,
    Comment,
    Invalid,
}

/// Tree-sitter capture names, most specific first where they share a
/// prefix, and the role each one takes.
const CAPTURES: &[(&str, HighlightKind)] = &[
    ("comment", HighlightKind::Comment),
    ("keyword", HighlightKind::Keyword),
    ("conditional", HighlightKind::Keyword),
    ("repeat", HighlightKind::Keyword),
    ("include", HighlightKind::Keyword),
    ("exception", HighlightKind::Keyword),
    ("storageclass", HighlightKind::Keyword),
    ("boolean", HighlightKind::Atom),
    ("constant.builtin", HighlightKind::Atom),
    ("constant", HighlightKind::Atom),
    ("label", HighlightKind::Atom),
    ("attribute", HighlightKind::Atom),
    ("tag", HighlightKind::TypeName),
    ("number", HighlightKind::Literal),
    ("float", HighlightKind::Literal),
    ("character", HighlightKind::String),
    ("string.regex", HighlightKind::Regexp),
    ("string.regexp", HighlightKind::Regexp),
    ("string.escape", HighlightKind::Regexp),
    ("string.special", HighlightKind::Regexp),
    ("escape", HighlightKind::Regexp),
    ("string", HighlightKind::String),
    ("text.uri", HighlightKind::Link),
    ("text.reference", HighlightKind::Link),
    ("type.builtin", HighlightKind::TypeName),
    ("type", HighlightKind::TypeName),
    ("namespace", HighlightKind::TypeName),
    ("module", HighlightKind::TypeName),
    ("constructor", HighlightKind::ClassName),
    ("function.macro", HighlightKind::Macro),
    ("variable.builtin", HighlightKind::Macro),
    ("function", HighlightKind::DefinitionVariable),
    ("variable.parameter", HighlightKind::LocalVariable),
    ("parameter", HighlightKind::LocalVariable),
    ("property", HighlightKind::DefinitionProperty),
    ("field", HighlightKind::DefinitionProperty),
    ("diff.plus", HighlightKind::Literal),
    ("diff.minus", HighlightKind::String),
    ("error", HighlightKind::Invalid),
];

/// The capture names handed to every configuration.
fn capture_names() -> Vec<&'static str> {
    CAPTURES.iter().map(|(name, _)| *name).collect()
}

struct Grammar {
    /// Canonical name, then the fence aliases that select it.
    names: &'static [&'static str],
    load: fn() -> Option<HighlightConfiguration>,
}

fn config(
    language: tree_sitter_language::LanguageFn,
    name: &str,
    highlights: &str,
    injections: &str,
    locals: &str,
) -> Option<HighlightConfiguration> {
    let mut config =
        HighlightConfiguration::new(language.into(), name, highlights, injections, locals).ok()?;
    config.configure(&capture_names());
    Some(config)
}

macro_rules! grammar {
    ($names:expr, $lang:expr, $highlights:expr) => {
        Grammar {
            names: $names,
            load: || config($lang, $names[0], $highlights, "", ""),
        }
    };
}

/// The 34 languages, most common in Markdown notes first.
static GRAMMARS: &[Grammar] = &[
    grammar!(
        &["javascript", "js", "jsx", "mjs", "cjs", "node"],
        tree_sitter_javascript::LANGUAGE,
        tree_sitter_javascript::HIGHLIGHT_QUERY
    ),
    grammar!(
        &["typescript", "ts", "mts", "cts"],
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
        TYPESCRIPT_HIGHLIGHTS.get_or_init(typescript_highlights)
    ),
    grammar!(
        &["tsx"],
        tree_sitter_typescript::LANGUAGE_TSX,
        TYPESCRIPT_HIGHLIGHTS.get_or_init(typescript_highlights)
    ),
    grammar!(
        &["python", "py", "python3", "py3"],
        tree_sitter_python::LANGUAGE,
        tree_sitter_python::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["bash", "sh", "shell", "zsh", "console", "shellscript"],
        tree_sitter_bash::LANGUAGE,
        tree_sitter_bash::HIGHLIGHT_QUERY
    ),
    grammar!(
        &["json", "jsonc", "json5"],
        tree_sitter_json::LANGUAGE,
        tree_sitter_json::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["rust", "rs"],
        tree_sitter_rust::LANGUAGE,
        tree_sitter_rust::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["html", "htm", "xhtml"],
        tree_sitter_html::LANGUAGE,
        tree_sitter_html::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["css"],
        tree_sitter_css::LANGUAGE,
        tree_sitter_css::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["yaml", "yml"],
        tree_sitter_yaml::LANGUAGE,
        tree_sitter_yaml::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["sql", "postgresql", "postgres", "mysql", "sqlite", "plsql"],
        tree_sitter_sequel::LANGUAGE,
        tree_sitter_sequel::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["go", "golang"],
        tree_sitter_go::LANGUAGE,
        tree_sitter_go::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["java"],
        tree_sitter_java::LANGUAGE,
        tree_sitter_java::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["c", "h"],
        tree_sitter_c::LANGUAGE,
        tree_sitter_c::HIGHLIGHT_QUERY
    ),
    grammar!(
        &["cpp", "c++", "cc", "cxx", "hpp", "hh", "hxx"],
        tree_sitter_cpp::LANGUAGE,
        CPP_HIGHLIGHTS.get_or_init(cpp_highlights)
    ),
    grammar!(
        &["csharp", "cs", "c#", "c-sharp"],
        tree_sitter_c_sharp::LANGUAGE,
        tree_sitter_c_sharp::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["php"],
        tree_sitter_php::LANGUAGE_PHP,
        tree_sitter_php::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["ruby", "rb"],
        tree_sitter_ruby::LANGUAGE,
        tree_sitter_ruby::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["swift"],
        tree_sitter_swift::LANGUAGE,
        tree_sitter_swift::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["toml"],
        tree_sitter_toml_ng::LANGUAGE,
        tree_sitter_toml_ng::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["xml", "svg", "plist", "xsl"],
        tree_sitter_xml::LANGUAGE_XML,
        tree_sitter_xml::XML_HIGHLIGHT_QUERY
    ),
    grammar!(
        &["diff", "patch"],
        tree_sitter_diff::LANGUAGE,
        tree_sitter_diff::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["lua"],
        tree_sitter_lua::LANGUAGE,
        tree_sitter_lua::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["scala", "sc"],
        tree_sitter_scala::LANGUAGE,
        tree_sitter_scala::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["haskell", "hs"],
        tree_sitter_haskell::LANGUAGE,
        tree_sitter_haskell::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["r"],
        tree_sitter_r::LANGUAGE,
        tree_sitter_r::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["dart"],
        tree_sitter_dart::LANGUAGE,
        tree_sitter_dart::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["elixir", "ex", "exs"],
        tree_sitter_elixir::LANGUAGE,
        tree_sitter_elixir::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["ocaml", "ml"],
        tree_sitter_ocaml::LANGUAGE_OCAML,
        tree_sitter_ocaml::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["zig"],
        tree_sitter_zig::LANGUAGE,
        tree_sitter_zig::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["make", "makefile", "mk", "mak"],
        tree_sitter_make::LANGUAGE,
        tree_sitter_make::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["nix"],
        tree_sitter_nix::LANGUAGE,
        tree_sitter_nix::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["elm"],
        tree_sitter_elm::LANGUAGE,
        tree_sitter_elm::HIGHLIGHTS_QUERY
    ),
    grammar!(
        &["regex", "regexp"],
        tree_sitter_regex::LANGUAGE,
        tree_sitter_regex::HIGHLIGHTS_QUERY
    ),
];

/// TypeScript's query extends JavaScript's.
static TYPESCRIPT_HIGHLIGHTS: OnceLock<String> = OnceLock::new();
fn typescript_highlights() -> String {
    format!(
        "{}\n{}",
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_typescript::HIGHLIGHTS_QUERY
    )
}

/// C++'s query extends C's.
static CPP_HIGHLIGHTS: OnceLock<String> = OnceLock::new();
fn cpp_highlights() -> String {
    format!(
        "{}\n{}",
        tree_sitter_c::HIGHLIGHT_QUERY,
        tree_sitter_cpp::HIGHLIGHT_QUERY
    )
}

/// Compiled configurations, built on first use.
fn configuration(index: usize) -> Option<&'static HighlightConfiguration> {
    static CONFIGS: OnceLock<Vec<OnceLock<Option<HighlightConfiguration>>>> = OnceLock::new();
    let configs = CONFIGS.get_or_init(|| GRAMMARS.iter().map(|_| OnceLock::new()).collect());
    configs
        .get(index)?
        .get_or_init(|| (GRAMMARS[index].load)())
        .as_ref()
}

/// Every language with a grammar, by canonical name.
pub fn languages() -> impl Iterator<Item = &'static str> {
    GRAMMARS.iter().map(|g| g.names[0])
}

/// The grammar a fence's info string selects (`rust`, `RS`, `js title=x`).
fn grammar_index(info: &str) -> Option<usize> {
    let name = info
        .split_whitespace()
        .next()?
        .trim_matches(['{', '}', '.'])
        .to_ascii_lowercase();
    GRAMMARS
        .iter()
        .position(|g| g.names.contains(&name.as_str()))
}

/// The canonical language a fence's info string selects, if it has a grammar.
pub fn language_for(info: &str) -> Option<&'static str> {
    grammar_index(info).map(|i| GRAMMARS[i].names[0])
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HighlightSpan {
    pub range: Range<usize>,
    pub kind: HighlightKind,
}

/// Colored spans of `code` (offsets into `code`), or `None` when `info`
/// names no known language or the grammar fails on this input.
pub fn highlight(info: &str, code: &str) -> Option<Vec<HighlightSpan>> {
    let config = configuration(grammar_index(info)?)?;
    let mut highlighter = Highlighter::new();
    let events = highlighter
        .highlight(config, code.as_bytes(), None, None, |_| None)
        .ok()?;
    let mut spans = Vec::new();
    let mut stack: Vec<HighlightKind> = Vec::new();
    for event in events {
        match event.ok()? {
            HighlightEvent::HighlightStart(h) => {
                if let Some((_, kind)) = CAPTURES.get(h.0) {
                    stack.push(*kind);
                }
            }
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                if let Some(kind) = stack.last() {
                    match spans.last_mut() {
                        Some(HighlightSpan { range, kind: last })
                            if *last == *kind && range.end == start =>
                        {
                            range.end = end
                        }
                        _ => spans.push(HighlightSpan {
                            range: start..end,
                            kind: *kind,
                        }),
                    }
                }
            }
        }
    }
    Some(spans)
}

/// Colored spans of every fenced code block of the document, in document
/// offsets: what the renderer paints over the code lines.
pub fn highlight_code_blocks(state: &cerata_markdown::EditorState) -> Vec<HighlightSpan> {
    let mut out = Vec::new();
    for block in cerata_markdown::code::code_blocks(state.tree(), state.text()) {
        if block.kind != cerata_markdown::code::CodeBlockKind::Fenced {
            continue;
        }
        let code = state.text().slice(block.code_range.clone());
        if let Some(spans) = highlight(&block.language, code) {
            let base = block.code_range.start;
            out.extend(spans.into_iter().map(|s| HighlightSpan {
                range: base + s.range.start..base + s.range.end,
                kind: s.kind,
            }));
        }
    }
    out
}

#[cfg(test)]
mod tests;
