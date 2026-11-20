//! Reading configs, settings and sources from JSON text, with clear errors.
//!
//! Missing settings take their default; for a config, the defaults follow its trust level. Unknown fields are
//! errors. The result is then validated ([`crate::validate`]).

use std::fmt;

use serde::de::DeserializeOwned;
use serde_json::{Map, Value};

use crate::model::{Config, Kind, SCHEMA, Settings, Source, Trust};
use crate::validate::{self, Invalid};

/// Why a file could not be read.
#[derive(Debug)]
pub enum LoadError {
    /// Not JSON.
    Syntax {
        line: usize,
        column: usize,
        message: String,
    },
    /// JSON, but not the expected shape (unknown or missing field, wrong type).
    Structure { path: String, message: String },
    /// The expected shape, but invalid values.
    Invalid(Invalid),
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Syntax {
                line,
                column,
                message,
            } => {
                write!(
                    f,
                    "  - line {line}, column {column}: not valid JSON: {message}"
                )
            }
            LoadError::Structure { path, message } => write!(f, "  - {path}: {message}"),
            LoadError::Invalid(invalid) => invalid.fmt(f),
        }
    }
}

impl std::error::Error for LoadError {}

/// Reads and validates a sandbox config or a profile.
pub fn config(text: &str, kind: Kind) -> Result<Config, LoadError> {
    let user = object(text)?;
    check_schema(&user)?;
    let trust = match user.get("trust").and_then(Value::as_str) {
        Some("untrusted") => Trust::Untrusted,
        _ => Trust::Work,
    };
    let Value::Object(mut defaults) = to_value(Config::defaults(trust)) else {
        unreachable!()
    };
    defaults.remove("id");
    defaults.remove("name");
    let config: Config = shape(merge(defaults, user))?;
    validate::config(&config, kind).map_err(LoadError::Invalid)?;
    Ok(config)
}

/// Reads and validates tool settings.
pub fn settings(text: &str) -> Result<Settings, LoadError> {
    let user = object(text)?;
    check_schema(&user)?;
    let Value::Object(defaults) = to_value(Settings::default()) else {
        unreachable!()
    };
    let settings: Settings = shape(merge(defaults, user))?;
    validate::settings(&settings).map_err(LoadError::Invalid)?;
    Ok(settings)
}

/// Reads and validates a source.
pub fn source(text: &str) -> Result<Source, LoadError> {
    let source: Source = shape(object(text)?)?;
    validate::source(&source).map_err(LoadError::Invalid)?;
    Ok(source)
}

fn object(text: &str) -> Result<Map<String, Value>, LoadError> {
    match serde_json::from_str(text) {
        Ok(Value::Object(map)) => Ok(map),
        Ok(_) => Err(LoadError::Structure {
            path: "(file)".into(),
            message: "expected a JSON object { … }".into(),
        }),
        Err(e) => Err(LoadError::Syntax {
            line: e.line(),
            column: e.column(),
            message: e
                .to_string()
                .split(" at line ")
                .next()
                .unwrap_or_default()
                .to_owned(),
        }),
    }
}

fn check_schema(user: &Map<String, Value>) -> Result<(), LoadError> {
    let message = match user.get("schema") {
        None => format!("missing: add \"schema\": {SCHEMA}"),
        Some(Value::Number(n)) if n.as_u64() == Some(u64::from(SCHEMA)) => return Ok(()),
        Some(v) => format!("{v} is not supported (this version reads schema {SCHEMA})"),
    };
    Err(LoadError::Structure {
        path: "schema".into(),
        message,
    })
}

fn to_value<T: serde::Serialize>(t: T) -> Value {
    serde_json::to_value(t).expect("model types serialize to JSON")
}

/// `over` on top of `base`: objects merge field by field, anything else replaces.
fn merge(mut base: Map<String, Value>, over: Map<String, Value>) -> Value {
    for (key, value) in over {
        let merged = match (base.remove(&key), value) {
            (Some(Value::Object(b)), Value::Object(o)) => merge(b, o),
            (_, value) => value,
        };
        base.insert(key, merged);
    }
    Value::Object(base)
}

fn shape<T: DeserializeOwned>(value: impl Into<Value>) -> Result<T, LoadError> {
    serde_path_to_error::deserialize(value.into()).map_err(|e| {
        let path = e.path().to_string();
        let path = if path == "." {
            "(file)".to_owned()
        } else {
            path
        };
        LoadError::Structure {
            path,
            message: e.into_inner().to_string(),
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Blocked, Theme};

    fn err(text: &str) -> String {
        config(text, Kind::Sandbox).unwrap_err().to_string()
    }

    #[test]
    fn minimal_untrusted_sandbox_gets_the_untrusted_defaults() {
        let c = config(
            r#"{ "schema": 1, "id": "t", "name": "T", "trust": "untrusted" }"#,
            Kind::Sandbox,
        )
        .unwrap();
        assert!(c.home.disposable && c.security.isolated_uid);
        assert_eq!(c.firewall.blocked, Blocked::Drop);
        // A field set by hand wins over the preset.
        let c = config(
            r#"{ "schema": 1, "id": "t", "name": "T", "trust": "untrusted", "home": { "disposable": false } }"#,
            Kind::Sandbox,
        )
        .unwrap();
        assert!(!c.home.disposable && c.security.isolated_uid);
    }

    #[test]
    fn clear_errors() {
        assert_eq!(
            err(r#"{ "schema": 1, "id": "t", }"#),
            "  - line 1, column 27: not valid JSON: trailing comma"
        );
        assert_eq!(
            err(r#"{ "id": "t" }"#),
            "  - schema: missing: add \"schema\": 1"
        );
        assert_eq!(
            err(r#"{ "schema": 2 }"#),
            "  - schema: 2 is not supported (this version reads schema 1)"
        );
        assert_eq!(
            err(r#"{ "schema": 1, "name": "T" }"#),
            "  - (file): missing field `id`"
        );
        let e = err(r#"{ "schema": 1, "id": "t", "name": "T", "capabilities": { "gpuu": true } }"#);
        assert!(
            e.starts_with("  - capabilities.gpuu: unknown field `gpuu`, expected one of"),
            "{e}"
        );
        assert_eq!(
            err(r#"{ "schema": 1, "id": "t", "name": "T", "capabilities": { "x11": "maybe" } }"#),
            "  - capabilities.x11: unknown variant `maybe`, expected one of `off`, `isolated`, `host`"
        );
        assert_eq!(
            err(
                r#"{ "schema": 1, "id": "t", "name": "T", "services": [ { "id": "s", "name": "S", "exec": ["x"], "autostart": "yes" } ] }"#
            ),
            "  - services[0].autostart: expected true, false or a list of folder names"
        );
        // Several invalid values are all reported.
        let e = err(
            r#"{ "schema": 1, "id": "Bad Id", "name": "T", "identity": { "home_mount": "/etc" } }"#,
        );
        assert_eq!(e.lines().count(), 2, "{e}");
        assert!(
            e.contains("  - id: ") && e.contains("  - identity.home_mount: "),
            "{e}"
        );
    }

    #[test]
    fn settings_defaults() {
        let s = settings(r#"{ "schema": 1, "theme": "dark" }"#).unwrap();
        assert_eq!(s.theme, Theme::Dark);
        assert!(s.tray_at_login);
        assert!(settings(r#"{ "schema": 1, "terminal": [] }"#).is_err());
    }
}
