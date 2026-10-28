//! Diagnostics settings (specs/018-error-telemetry-feedback data-model.md), stored as
//! `diagnostics_*` fields of `LRR_CONFIG` but read/written only through `/api/diagnostics/settings`
//! — the tracker DSN is a credential, so it never rides along with the general `/settings` payload.

use std::collections::HashMap;

use deadpool_redis::redis::AsyncCommands;
use lanrurugi_storage::keys::CONFIG_KEY;
use serde_json::Value;

use lanrurugi_storage::diagnostics::Retention;

use crate::AppState;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bool,
    /// A whole number of at least this value.
    Int(i64),
    /// Free text (the tracker DSN), validated separately from its shape.
    Text,
}

/// `(field, kind, default)`.
const FIELDS: &[(&str, Kind, &str)] = &[
    // Master switch, then one per source: capture is on by default because it never leaves the
    // machine, and turning it off is a deliberate act.
    ("diagnostics_enabled", Kind::Bool, "1"),
    ("diagnostics_web", Kind::Bool, "1"),
    ("diagnostics_server", Kind::Bool, "1"),
    ("diagnostics_plugins", Kind::Bool, "1"),
    ("diagnostics_breadcrumbs", Kind::Bool, "1"),
    ("diagnostics_request_bodies", Kind::Bool, "1"),
    // The one switch that can put an archive's name into a report.
    ("diagnostics_include_archive_names", Kind::Bool, "0"),
    // Retention: whichever limit is reached first evicts the oldest.
    ("diagnostics_retention_days", Kind::Int(1), "7"),
    ("diagnostics_max_events", Kind::Int(100), "50000"),
    ("diagnostics_max_bytes", Kind::Int(1048576), "536870912"),
    // Optional forwarding target; empty means "never send".
    ("diagnostics_tracker_dsn", Kind::Text, ""),
    // Unix milliseconds; 0 (or the past) means closed.
    ("diagnostics_window_deadline", Kind::Int(0), "0"),
    ("diagnostics_rate_per_minute", Kind::Int(1), "60"),
    ("diagnostics_rate_per_hour", Kind::Int(1), "1000"),
];

/// Diagnostics settings, defaults filled in.
#[derive(Debug, Clone)]
pub struct DiagnosticsSettings {
    values: HashMap<&'static str, String>,
}

impl DiagnosticsSettings {
    pub fn from_map(stored: &HashMap<String, String>) -> Self {
        let values = FIELDS
            .iter()
            .map(|(name, _, default)| {
                (
                    *name,
                    stored
                        .get(*name)
                        .cloned()
                        .unwrap_or_else(|| default.to_string()),
                )
            })
            .collect();
        Self { values }
    }

    pub fn text(&self, name: &str) -> &str {
        self.values.get(name).map(String::as_str).unwrap_or("")
    }

    pub fn flag(&self, name: &str) -> bool {
        self.text(name) == "1"
    }

    pub fn number(&self, name: &str) -> u64 {
        self.text(name).trim().parse().unwrap_or(0)
    }

    pub fn enabled(&self) -> bool {
        self.flag("diagnostics_enabled")
    }

    pub fn capture_source(&self, source: &str) -> bool {
        self.enabled()
            && match source {
                "web" => self.flag("diagnostics_web"),
                "plugin" => self.flag("diagnostics_plugins"),
                _ => self.flag("diagnostics_server"),
            }
    }

    pub fn retention(&self) -> Retention {
        Retention {
            days: self.number("diagnostics_retention_days"),
            max_events: self.number("diagnostics_max_events"),
            max_bytes: self.number("diagnostics_max_bytes"),
        }
    }

    pub fn tracker_dsn(&self) -> Option<&str> {
        let dsn = self.text("diagnostics_tracker_dsn").trim();
        (!dsn.is_empty()).then_some(dsn)
    }

    pub fn window_deadline(&self) -> i64 {
        self.text("diagnostics_window_deadline")
            .trim()
            .parse()
            .unwrap_or(0)
    }

    /// The JSON the settings page gets: the DSN is replaced by a boolean, never echoed.
    pub fn to_json(&self) -> Value {
        let mut object = serde_json::Map::new();
        for (name, kind, _) in FIELDS {
            if *name == "diagnostics_tracker_dsn" || *name == "diagnostics_window_deadline" {
                continue;
            }
            let raw = self.text(name);
            let value = match kind {
                Kind::Bool => Value::Bool(raw == "1"),
                Kind::Int(_) => raw.parse::<i64>().map(Value::from).unwrap_or(Value::Null),
                // The DSN is skipped above, so this arm is only to keep the match exhaustive.
                Kind::Text => continue,
            };
            object.insert((*name).to_string(), value);
        }
        object.insert(
            "tracker_configured".to_string(),
            Value::Bool(self.tracker_dsn().is_some()),
        );
        Value::Object(object)
    }
}

pub async fn load(state: &AppState) -> DiagnosticsSettings {
    let stored: HashMap<String, String> = match state.redis.config.get().await {
        Ok(mut conn) => {
            let names: Vec<&str> = FIELDS.iter().map(|(n, _, _)| *n).collect();
            let values: Vec<Option<String>> = deadpool_redis::redis::cmd("HMGET")
                .arg(CONFIG_KEY)
                .arg(&names)
                .query_async(&mut conn)
                .await
                .unwrap_or_else(|_| vec![None; names.len()]);
            names
                .into_iter()
                .zip(values)
                .filter_map(|(n, v)| v.map(|v| (n.to_string(), v)))
                .collect()
        }
        Err(_) => HashMap::new(),
    };
    DiagnosticsSettings::from_map(&stored)
}

/// Validates and writes the given fields; unknown keys or invalid values reject the whole update.
pub async fn save(
    state: &AppState,
    body: &serde_json::Map<String, Value>,
) -> Result<DiagnosticsSettings, String> {
    let mut writes = Vec::with_capacity(body.len());
    for (key, value) in body {
        let stored = validate_field(key, value)?;
        // Stored under the canonical field name: `tracker_dsn` is accepted as an alias of
        // `diagnostics_tracker_dsn`, and writing the alias would leave the loader reading "".
        writes.push((field_name(key).to_string(), stored));
    }
    if !writes.is_empty() {
        let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
        let _: () = conn
            .hset_multiple(CONFIG_KEY, &writes)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(load(state).await)
}

/// Accepts the contract's `tracker_dsn` spelling as an alias of the stored field.
fn field_name(key: &str) -> &str {
    match key {
        "tracker_dsn" => "diagnostics_tracker_dsn",
        other => other,
    }
}

fn validate_field(key: &str, value: &Value) -> Result<String, String> {
    let key = field_name(key);
    // The DSN and the window deadline are written by their own routes, not by this general update.
    if key == "diagnostics_window_deadline" {
        return Err("the forwarding window is set through /api/diagnostics/window.".to_string());
    }
    let Some((_, kind, _)) = FIELDS.iter().find(|(name, _, _)| *name == key) else {
        return Err(format!("Unknown diagnostics setting: \"{key}\"."));
    };
    let text = match value {
        Value::String(s) => s.trim().to_string(),
        Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
        Value::Number(n) => n.to_string(),
        Value::Null => String::new(),
        _ => return Err(format!("\"{key}\" must be a string, number or boolean.")),
    };
    match kind {
        Kind::Bool => match text.as_str() {
            "1" | "true" => Ok("1".into()),
            "0" | "false" => Ok("0".into()),
            _ => Err(format!("\"{key}\" must be true or false.")),
        },
        Kind::Int(_) => match text.parse::<i64>() {
            Ok(n) if n >= 0 => Ok(n.to_string()),
            _ => Err(format!("\"{key}\" must be a whole number of at least 1.")),
        },
        Kind::Text => {
            if text.is_empty() {
                return Ok(String::new());
            }
            match url::Url::parse(&text) {
                Ok(url) if matches!(url.scheme(), "http" | "https") && url.host().is_some() => {
                    Ok(text)
                }
                _ => Err(format!(
                    "\"{key}\" must be an http(s) URL, for example http://host:8080/project-id."
                )),
            }
        }
    }
}
