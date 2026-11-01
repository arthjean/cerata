//! The embedded settings schema: 43 keys with their types, defaults, bounds
//! and scopes.
//!
//! Ported from Writer `config.rs` (`SettingDef`, `settings_schema`) and
//! `apps/desktop/shared/settings.schema.json`, copied to
//! `crates/cerata_core/assets/settings.schema.json`.

use std::collections::HashMap;
use std::fmt;
use std::sync::OnceLock;

use serde::Deserialize;

use super::{ConfigValue, SettingsError};

const SCHEMA_JSON: &str = include_str!("../../assets/settings.schema.json");

/// Value type of a setting, as declared by the schema's `type` field.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ValueType {
    String,
    Number,
    Boolean,
    /// A string restricted to [`SettingDef::options`].
    Enum,
    /// Repeated `key = item` lines.
    List,
    Color,
    /// A number clamped to [`SettingDef::min`] and [`SettingDef::max`].
    Range,
    /// A CSS-style font family stack.
    Font,
}

impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::String => "a string",
            Self::Number | Self::Range => "a number",
            Self::Boolean => "true or false",
            Self::Enum => "one of the listed options",
            Self::List => "a list",
            Self::Color => "a color",
            Self::Font => "a font stack",
        })
    }
}

/// Normalization applied to a string value before it is stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Normalization {
    Trim,
}

/// Where a setting may be overridden. Absent means any layer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Scope {
    /// App-level: a workspace config cannot override it.
    Global,
}

/// One schema entry. Writer's web-only `cssVar` and `cssFormat` fields are
/// not read.
#[derive(Debug, Clone, Deserialize)]
pub struct SettingDef {
    pub key: String,
    pub label: String,
    pub description: String,
    pub category: String,
    #[serde(rename = "type")]
    pub value_type: ValueType,
    #[serde(default)]
    pub options: Option<Vec<String>>,
    #[serde(default)]
    pub min: Option<f64>,
    #[serde(default)]
    pub max: Option<f64>,
    #[serde(default)]
    pub step: Option<f64>,
    #[serde(default)]
    pub placeholder: Option<String>,
    #[serde(default)]
    pub normalize: Option<Normalization>,
    #[serde(default)]
    pub scope: Option<Scope>,
    pub default: ConfigValue,
}

impl SettingDef {
    /// True when the key is app-level and a workspace cannot override it.
    pub fn is_global_only(&self) -> bool {
        self.scope == Some(Scope::Global)
    }

    /// Clamps `value` into `[min, max]` when the schema declares bounds.
    pub(crate) fn clamp(&self, value: f64) -> f64 {
        let value = self.min.map_or(value, |min| value.max(min));
        self.max.map_or(value, |max| value.min(max))
    }
}

/// A font default for this OS: outside macOS the San Francisco families
/// and Apple's system aliases are dropped, so the stack starts with a font
/// the system has.
pub fn platform_font_stack(stack: &str, macos: bool) -> String {
    if macos {
        return stack.to_owned();
    }
    stack
        .split(',')
        .map(str::trim)
        .filter(|family| {
            let name = family.trim_matches(|c| c == '"' || c == '\'');
            !(name.starts_with("SF ")
                || name.starts_with("SFMono")
                || name.starts_with("-apple-system"))
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// The parsed schema, in file order.
#[derive(Debug)]
pub struct Schema {
    definitions: Vec<SettingDef>,
    positions: HashMap<String, usize>,
}

#[derive(Deserialize)]
struct SchemaFile {
    settings: Vec<SettingDef>,
}

impl Schema {
    /// The schema embedded in the binary, parsed once per process.
    pub fn embedded() -> Result<&'static Schema, SettingsError> {
        static SCHEMA: OnceLock<Result<Schema, String>> = OnceLock::new();
        SCHEMA
            .get_or_init(|| Self::parse(SCHEMA_JSON).map_err(|error| error.to_string()))
            .as_ref()
            .map_err(|message| SettingsError::Schema(message.clone()))
    }

    fn parse(json: &str) -> Result<Self, serde_json::Error> {
        let mut file: SchemaFile = serde_json::from_str(json)?;
        for definition in &mut file.settings {
            if let (ValueType::Font, ConfigValue::String(stack)) =
                (definition.value_type, &definition.default)
            {
                definition.default =
                    ConfigValue::String(platform_font_stack(stack, cfg!(target_os = "macos")));
            }
        }
        let positions = file
            .settings
            .iter()
            .enumerate()
            .map(|(index, definition)| (definition.key.clone(), index))
            .collect();
        Ok(Self {
            definitions: file.settings,
            positions,
        })
    }

    /// Every definition, in schema order.
    pub fn definitions(&self) -> &[SettingDef] {
        &self.definitions
    }

    /// The definition of `key`, if the schema declares it.
    pub fn get(&self, key: &str) -> Option<&SettingDef> {
        self.position(key).map(|index| &self.definitions[index])
    }

    /// Index of `key` in schema order.
    pub(crate) fn position(&self, key: &str) -> Option<usize> {
        self.positions.get(key).copied()
    }
}

#[cfg(test)]
mod platform_tests {
    use super::*;

    #[test]
    fn font_defaults_have_no_sf_family_outside_macos() {
        let stack =
            r#""SF Pro", -apple-system-body, ui-sans-serif, -apple-system, system-ui, "Segoe UI""#;
        assert_eq!(
            platform_font_stack(stack, false),
            "ui-sans-serif, system-ui, \"Segoe UI\""
        );
        assert_eq!(platform_font_stack(stack, true), stack);
        assert_eq!(
            platform_font_stack(
                r#""SF Mono", ui-monospace, SFMono-Regular, Consolas"#,
                false
            ),
            "ui-monospace, Consolas"
        );
    }

    #[test]
    fn the_embedded_font_defaults_follow_the_os() {
        let schema = Schema::embedded().unwrap();
        for key in ["fonts.ui", "fonts.editor", "fonts.mono"] {
            let Some(ConfigValue::String(stack)) = schema.get(key).map(|d| d.default.clone())
            else {
                panic!("{key} has no string default");
            };
            assert_eq!(
                stack.contains("SF "),
                cfg!(target_os = "macos"),
                "{key}: {stack}"
            );
        }
    }
}
