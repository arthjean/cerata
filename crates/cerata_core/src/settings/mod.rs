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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Layer {
    Global,
    Workspace,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigWarning {
    pub path: PathBuf,
    pub line: usize,
    pub key: String,
    pub kind: WarningKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WarningKind {
    InvalidValue { raw: String, expected: ValueType },
    OutOfRange { raw: String },
    UnknownKey,
    GlobalOnly,
    Duplicate { first_line: usize },
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
