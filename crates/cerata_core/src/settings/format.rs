use std::collections::HashMap;
use std::path::Path;

use super::schema::{Schema, SettingDef, ValueType};
use super::{ConfigValue, ConfigWarning, Layer, SettingsError, WarningKind};

pub(crate) struct ParsedLayer {
    pub(crate) values: HashMap<String, ConfigValue>,
    pub(crate) warnings: Vec<ConfigWarning>,
}

fn line_key(line: &str) -> Option<&str> {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.starts_with('#') {
        return None;
    }
    let (key, _) = trimmed.split_once('=')?;
    let key = key.trim();
    (!key.is_empty()).then_some(key)
}

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
