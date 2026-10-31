//! On-disk text format: line endings and UTF-8 BOM are detected on read and
//! restored on write, so saving never rewrites a file's conventions. The
//! editor always sees `\n` line endings and no BOM.

use std::borrow::Cow;

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LineEnding {
    #[default]
    Lf,
    CrLf,
}

/// How a file was stored, to write it back the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TextFormat {
    pub line_ending: LineEnding,
    pub bom: bool,
}

/// Decodes file bytes into editor text (UTF-8, `\n` only, no BOM) and the
/// format to restore on save. The first line ending found decides the
/// format of a file that mixes both.
pub fn decode(bytes: &[u8]) -> Result<(String, TextFormat), std::str::Utf8Error> {
    let (bom, body) = match bytes.strip_prefix(UTF8_BOM) {
        Some(rest) => (true, rest),
        None => (false, bytes),
    };
    let text = std::str::from_utf8(body)?;
    let line_ending = match text.find('\n') {
        Some(index) if index > 0 && text.as_bytes()[index - 1] == b'\r' => LineEnding::CrLf,
        _ => LineEnding::Lf,
    };
    let text = if text.contains("\r\n") {
        text.replace("\r\n", "\n")
    } else {
        text.to_owned()
    };
    Ok((text, TextFormat { line_ending, bom }))
}

/// Encodes editor text back into bytes in `format`.
pub fn encode(text: &str, format: TextFormat) -> Vec<u8> {
    let body: Cow<'_, str> = match format.line_ending {
        LineEnding::Lf => Cow::Borrowed(text),
        LineEnding::CrLf => Cow::Owned(text.replace("\r\n", "\n").replace('\n', "\r\n")),
    };
    let mut bytes = Vec::with_capacity(body.len() + UTF8_BOM.len());
    if format.bom {
        bytes.extend_from_slice(UTF8_BOM);
    }
    bytes.extend_from_slice(body.as_bytes());
    bytes
}

/// The `files.*` settings applied to text right before it is written.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaveOptions {
    /// `files.trim-trailing-whitespace`, off by default.
    pub trim_trailing_whitespace: bool,
    /// `files.insert-final-newline`, on by default.
    pub insert_final_newline: bool,
}

impl Default for SaveOptions {
    fn default() -> Self {
        Self {
            trim_trailing_whitespace: false,
            insert_final_newline: true,
        }
    }
}

impl SaveOptions {
    /// Reads `files.trim-trailing-whitespace` and `files.insert-final-newline`
    /// from the merged settings, falling back to the defaults.
    pub fn from_settings(settings: &crate::settings::Settings) -> Self {
        let flag = |key: &str, default: bool| {
            settings
                .get(key)
                .and_then(|value| value.as_bool())
                .unwrap_or(default)
        };
        let defaults = Self::default();
        Self {
            trim_trailing_whitespace: flag(
                "files.trim-trailing-whitespace",
                defaults.trim_trailing_whitespace,
            ),
            insert_final_newline: flag("files.insert-final-newline", defaults.insert_final_newline),
        }
    }
}

/// Applies [`SaveOptions`] to editor text (Writer `save.ts:105`).
pub fn prepare_for_save(text: &str, options: SaveOptions) -> String {
    let mut result = if options.trim_trailing_whitespace {
        text.split('\n')
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
    } else {
        text.to_owned()
    };
    if options.insert_final_newline && !result.ends_with('\n') {
        result.push('\n');
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_and_bom_round_trip() {
        let original = b"\xEF\xBB\xBF# Title\r\n\r\nBody\r\n";
        let (text, format) = decode(original).unwrap();
        assert_eq!(text, "# Title\n\nBody\n");
        assert_eq!(
            format,
            TextFormat {
                line_ending: LineEnding::CrLf,
                bom: true
            }
        );
        assert_eq!(encode(&text, format), original.to_vec());
    }

    #[test]
    fn lf_without_bom_round_trips() {
        let original = b"a\nb";
        let (text, format) = decode(original).unwrap();
        assert_eq!(format, TextFormat::default());
        assert_eq!(encode(&text, format), original.to_vec());
    }

    #[test]
    fn invalid_utf8_is_an_error() {
        assert!(decode(b"\xff\xfe").is_err());
    }

    #[test]
    fn save_options_trim_and_final_newline() {
        let text = "a  \nb\t\n\nc ";
        assert_eq!(
            prepare_for_save(text, SaveOptions::default()),
            "a  \nb\t\n\nc \n"
        );
        let both = SaveOptions {
            trim_trailing_whitespace: true,
            insert_final_newline: true,
        };
        assert_eq!(prepare_for_save(text, both), "a\nb\n\nc\n");
        let neither = SaveOptions {
            trim_trailing_whitespace: false,
            insert_final_newline: false,
        };
        assert_eq!(prepare_for_save(text, neither), text);
    }

    #[test]
    fn save_options_follow_the_files_settings() {
        let config = tempfile::TempDir::new().unwrap();
        let (settings, _) = crate::settings::Settings::open(config.path()).unwrap();
        assert_eq!(
            SaveOptions::from_settings(&settings),
            SaveOptions::default()
        );
        std::fs::write(
            config.path().join("config"),
            "files.trim-trailing-whitespace = true
files.insert-final-newline = false
",
        )
        .unwrap();
        let (settings, _) = crate::settings::Settings::open(config.path()).unwrap();
        assert_eq!(
            SaveOptions::from_settings(&settings),
            SaveOptions {
                trim_trailing_whitespace: true,
                insert_final_newline: false,
            }
        );
    }
}
