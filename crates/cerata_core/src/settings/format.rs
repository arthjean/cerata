//! The Ghostty-style `key = value` config format: schema-typed parsing with
//! line-numbered warnings, and single-key text edits that keep comments,
//! unknown lines and order.
//!
//! Ported from Writer `config.rs` (`parse_value`, `parse_config_with_defaults`,
//! `serialize_config`, `remove_key_from_config`). Changes from Writer: values
//! are parsed by schema type instead of guessed, invalid values are dropped
//! with a warning instead of stored as strings, and a write edits one key in
//! the file text instead of re-serializing a `HashMap`, so a new key lands in
//! schema order rather than in hash order.

use std::collections::HashMap;
use std::path::Path;

use super::schema::{Schema, SettingDef, ValueType};
use super::{ConfigValue, ConfigWarning, Layer, SettingsError, WarningKind};

/// Values and warnings read from one config file.
pub(crate) struct ParsedLayer {
    pub(crate) values: HashMap<String, ConfigValue>,
    pub(crate) warnings: Vec<ConfigWarning>,
}

/// Key of a `key = value` line, or `None` for blank, comment and malformed
/// lines.
fn line_key(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (key, _) = trimmed.split_once('=')?;
    let key = key.trim();
    (!key.is_empty()).then_some(key)
}

/// Parses `raw` against the schema. Lines are 1-based in warnings.
///
/// - Blank lines and `#` comments are skipped.
/// - A list key accumulates its repeated lines; an empty value marks the
///   list as present but empty.
/// - A repeated scalar key keeps its last valid value and warns.
/// - Unknown keys, malformed lines, invalid values and, in a workspace file,
///   global-only keys are skipped with a warning, so the key falls back to
///   the next layer. Out-of-range numbers are clamped with a warning.
pub(crate) fn parse(raw: &str, path: &Path, schema: &Schema, layer: Layer) -> ParsedLayer {
    let mut values = HashMap::new();
    let mut lists: HashMap<String, Vec<String>> = HashMap::new();
    let mut first_lines: HashMap<String, usize> = HashMap::new();
    let mut warnings = Vec::new();

    for (index, line) in raw.lines().enumerate() {
        let number = index + 1;
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let mut warn = |key: &str, kind: WarningKind| {
            warnings.push(ConfigWarning {
                path: path.to_path_buf(),
                line: number,
                key: key.to_string(),
                kind,
            });
        };
        let Some((key, value)) = trimmed
            .split_once('=')
            .map(|(key, value)| (key.trim(), value.trim()))
            .filter(|(key, _)| !key.is_empty())
        else {
            warn(trimmed, WarningKind::Malformed);
            continue;
        };
        let Some(definition) = schema.get(key) else {
            warn(key, WarningKind::UnknownKey);
            continue;
        };
        if layer == Layer::Workspace && definition.is_global_only() {
            warn(key, WarningKind::GlobalOnly);
            continue;
        }
        if definition.value_type == ValueType::List {
            let items = lists.entry(key.to_string()).or_default();
            if !value.is_empty() {
                items.push(value.to_string());
            }
            continue;
        }
        match parse_scalar(definition, value) {
            Some((parsed, clamped)) => {
                if let Some(&first_line) = first_lines.get(key) {
                    warn(key, WarningKind::Duplicate { first_line });
                } else {
                    first_lines.insert(key.to_string(), number);
                }
                if clamped {
                    warn(
                        key,
                        WarningKind::OutOfRange {
                            raw: value.to_string(),
                        },
                    );
                }
                values.insert(key.to_string(), parsed);
            }
            None => warn(
                key,
                WarningKind::InvalidValue {
                    raw: value.to_string(),
                    expected: definition.value_type,
                },
            ),
        }
    }

    values.extend(
        lists
            .into_iter()
            .map(|(key, items)| (key, ConfigValue::List(items))),
    );
    ParsedLayer { values, warnings }
}

/// Parses a non-list value. The flag is true when a range value was clamped.
fn parse_scalar(definition: &SettingDef, raw: &str) -> Option<(ConfigValue, bool)> {
    match definition.value_type {
        ValueType::Boolean => {
            if raw.eq_ignore_ascii_case("true") {
                Some((ConfigValue::Bool(true), false))
            } else if raw.eq_ignore_ascii_case("false") {
                Some((ConfigValue::Bool(false), false))
            } else {
                None
            }
        }
        ValueType::Number => parse_number(raw).map(|number| (ConfigValue::Number(number), false)),
        ValueType::Range => parse_number(raw).map(|number| {
            let clamped = definition.clamp(number);
            (ConfigValue::Number(clamped), clamped != number)
        }),
        ValueType::Enum => definition
            .options
            .as_ref()
            .is_some_and(|options| options.iter().any(|option| option == raw))
            .then(|| (ConfigValue::String(raw.to_string()), false)),
        ValueType::String | ValueType::Color | ValueType::Font => {
            Some((ConfigValue::String(raw.to_string()), false))
        }
        ValueType::List => None,
    }
}

fn parse_number(raw: &str) -> Option<f64> {
    raw.parse::<f64>().ok().filter(|number| number.is_finite())
}

/// Checks a value written by the app against its schema entry and returns
/// its canonical form: strings and list items trimmed (the line format cannot
/// carry surrounding whitespace), empty list items dropped, range numbers
/// clamped to their bounds.
pub(crate) fn validate(
    definition: &SettingDef,
    value: ConfigValue,
) -> Result<ConfigValue, SettingsError> {
    let key = &definition.key;
    let invalid = |reason: &'static str| SettingsError::InvalidValue {
        key: key.clone(),
        reason,
    };
    let single_line = |text: &str| -> Result<String, SettingsError> {
        if text.contains(['\n', '\r']) {
            return Err(invalid("contains a line break"));
        }
        // Every string is trimmed, which also covers `normalize: trim`.
        Ok(text.trim().to_string())
    };
    match (definition.value_type, value) {
        (ValueType::Boolean, value @ ConfigValue::Bool(_)) => Ok(value),
        (ValueType::Number | ValueType::Range, ConfigValue::Number(number)) => {
            if !number.is_finite() {
                return Err(invalid("is not a finite number"));
            }
            if definition.value_type == ValueType::Range {
                Ok(ConfigValue::Number(definition.clamp(number)))
            } else {
                Ok(ConfigValue::Number(number))
            }
        }
        (ValueType::Enum, ConfigValue::String(text)) => {
            let text = single_line(&text)?;
            let known = definition
                .options
                .as_ref()
                .is_some_and(|options| options.contains(&text));
            if known {
                Ok(ConfigValue::String(text))
            } else {
                Err(invalid("is not one of the listed options"))
            }
        }
        (ValueType::String | ValueType::Color | ValueType::Font, ConfigValue::String(text)) => {
            Ok(ConfigValue::String(single_line(&text)?))
        }
        (ValueType::List, ConfigValue::List(items)) => {
            let mut canonical = Vec::with_capacity(items.len());
            for item in items {
                let item = single_line(&item)?;
                if !item.is_empty() {
                    canonical.push(item);
                }
            }
            Ok(ConfigValue::List(canonical))
        }
        (expected, _) => Err(SettingsError::TypeMismatch {
            key: key.clone(),
            expected,
        }),
    }
}

/// The lines that represent `key = value`.
fn value_lines(key: &str, value: &ConfigValue) -> Vec<String> {
    let line = |text: &str| {
        if text.is_empty() {
            format!("{key} =")
        } else {
            format!("{key} = {text}")
        }
    };
    match value {
        ConfigValue::List(items) if items.is_empty() => vec![line("")],
        ConfigValue::List(items) => items.iter().map(|item| line(item)).collect(),
        ConfigValue::Bool(flag) => vec![line(&flag.to_string())],
        ConfigValue::Number(number) => vec![line(&number.to_string())],
        ConfigValue::String(text) => vec![line(text)],
    }
}

/// Rewrites `raw` with `key` set to `value`, or removed when `value` is
/// `None`. Every other line is kept byte for byte, as is the file's line
/// ending (`\r\n` when the file already uses it).
///
/// An existing key is rewritten at its first line and its later duplicates
/// are dropped. A new key is inserted in schema order: right after the last
/// line of the closest preceding schema key in the file; failing that, before
/// the comment block above the first following schema key; failing that, at
/// the end.
pub(crate) fn edit(raw: &str, key: &str, value: Option<&ConfigValue>, schema: &Schema) -> String {
    let newline = if raw.contains("\r\n") { "\r\n" } else { "\n" };
    let mut lines: Vec<String> = Vec::new();
    let mut written = false;
    for line in raw.lines() {
        if line_key(line) == Some(key) {
            if let (Some(value), false) = (value, written) {
                lines.extend(value_lines(key, value));
                written = true;
            }
            continue;
        }
        lines.push(line.to_string());
    }
    if let (Some(value), false) = (value, written) {
        let at = insertion_index(&lines, key, schema);
        lines.splice(at..at, value_lines(key, value));
    }
    let mut out = lines.join(newline);
    if !out.is_empty() {
        out.push_str(newline);
    }
    out
}

fn insertion_index(lines: &[String], key: &str, schema: &Schema) -> usize {
    let Some(target) = schema.position(key) else {
        return lines.len();
    };
    let positions: Vec<(usize, usize)> = lines
        .iter()
        .enumerate()
        .filter_map(|(index, line)| {
            let position = schema.position(line_key(line)?)?;
            Some((index, position))
        })
        .collect();

    let preceding = positions
        .iter()
        .filter(|(_, position)| *position < target)
        .map(|(_, position)| *position)
        .max();
    if let Some(closest) = preceding {
        return positions
            .iter()
            .filter(|(_, position)| *position == closest)
            .map(|(index, _)| index + 1)
            .max()
            .unwrap_or(lines.len());
    }

    let Some(&(mut at, _)) = positions.iter().find(|(_, position)| *position > target) else {
        return lines.len();
    };
    while at > 0 && lines[at - 1].trim_start().starts_with('#') {
        at -= 1;
    }
    at
}
