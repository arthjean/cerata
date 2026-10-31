//! Settings (US-004): three layers (schema defaults, the global config, the
//! workspace config) in a hand-editable `key = value` format.
//!
//! Ported from Writer `config.rs`, with the write ordering of Writer's
//! `src/stores/settings-store.ts` moved into the core. The app holds a
//! [`Settings`] handle per window:
//!
//! ```no_run
//! # use cerata_core::settings::{ConfigValue, Layer, Settings};
//! # fn main() -> Result<(), cerata_core::settings::SettingsError> {
//! let (settings, _warnings) = Settings::open_default()?;
//! let pending = settings.begin_set("editor.zoom", ConfigValue::Number(125.0), Layer::Global)?;
//! // `settings.get("editor.zoom")` already reads 125; write on a background thread:
//! pending.commit()?;
//! # Ok(()) }
//! ```
//!
//! Files: `paths::global_config_dir()/config` and
//! `paths::workspace_config_path(root)`.

mod atomic;
mod format;
mod schema;
mod store;
#[cfg(test)]
mod tests;

use std::fmt;
use std::path::PathBuf;

use serde::Deserialize;

pub use schema::{Normalization, Schema, Scope, SettingDef, ValueType};
pub use store::{PendingWrite, Settings, WriteOutcome, global_config_path};

/// A setting value. Lists are written as one `key = item` line per item.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(untagged)]
pub enum ConfigValue {
    Bool(bool),
    Number(f64),
    String(String),
    List(Vec<String>),
}

impl ConfigValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_f64(&self) -> Option<f64> {
        match self {
            Self::Number(number) => Some(*number),
            _ => None,
        }
    }

    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(flag) => Some(*flag),
            _ => None,
        }
    }

    pub fn as_list(&self) -> Option<&[String]> {
        match self {
            Self::List(items) => Some(items),
            _ => None,
        }
    }
}

/// The config file a write targets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layer {
    Global,
    Workspace,
}

/// A problem found in a config file. The offending line is skipped (or, for
/// [`WarningKind::OutOfRange`], clamped), other lines still load, and the
/// file text itself is never rewritten because of it.
#[derive(Debug, Clone, PartialEq)]
pub struct ConfigWarning {
    pub path: PathBuf,
    /// 1-based line number.
    pub line: usize,
    /// The line's key, or the whole line when it is malformed.
    pub key: String,
    pub kind: WarningKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WarningKind {
    /// The value does not parse as the schema type; the key falls back to
    /// the next layer.
    InvalidValue { raw: String, expected: ValueType },
    /// A range value outside its bounds, clamped.
    OutOfRange { raw: String },
    /// A key the schema does not declare (including Writer-legacy keys).
    UnknownKey,
    /// A global-only key in a workspace config; ignored.
    GlobalOnly,
    /// A scalar key repeated; the last valid line wins.
    Duplicate { first_line: usize },
    /// A non-comment line without `key = value`.
    Malformed,
}

impl fmt::Display for ConfigWarning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self {
            path, line, key, ..
        } = self;
        write!(f, "{}:{line}: ", path.display())?;
        match &self.kind {
            WarningKind::InvalidValue { raw, expected } => write!(
                f,
                "`{key} = {raw}`: expected {expected}, falling back to the default"
            ),
            WarningKind::OutOfRange { raw } => {
                write!(f, "`{key} = {raw}` is out of range and was clamped")
            }
            WarningKind::UnknownKey => write!(f, "unknown setting `{key}`, ignored"),
            WarningKind::GlobalOnly => write!(
                f,
                "`{key}` is a global setting and cannot be set per workspace, ignored"
            ),
            WarningKind::Duplicate { first_line } => write!(
                f,
                "`{key}` is already set on line {first_line}; this line overrides it"
            ),
            WarningKind::Malformed => write!(f, "expected `key = value`, got `{key}`"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SettingsError {
    #[error("unknown setting `{0}`")]
    UnknownKey(String),
    #[error("setting `{key}` is global and cannot be set per workspace")]
    GlobalOnly { key: String },
    #[error("setting `{key}` expects {expected}")]
    TypeMismatch { key: String, expected: ValueType },
    #[error("value for `{key}` {reason}")]
    InvalidValue { key: String, reason: &'static str },
    #[error("no workspace is open")]
    NoWorkspace,
    #[error("the OS reports no config directory")]
    NoConfigDir,
    #[error("cannot access {}: {source}", path.display())]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("embedded settings schema is malformed: {0}")]
    Schema(String),
}
