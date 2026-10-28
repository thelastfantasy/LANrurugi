//! Diagnostics settings (specs/018-error-telemetry-feedback data-model.md), stored as
//! `diagnostics_*` fields of `LRR_CONFIG` but read/written only through `/api/diagnostics/settings`
//! — the tracker DSN is a credential, so it never rides along with the general `/settings` payload.

use std::collections::HashMap;
use std::sync::OnceLock;

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
    // On by default (product decision, 2026-10-28): a report that names the file that failed is
    // far easier to act on. Credentials are removed either way.
    ("diagnostics_include_private_data", Kind::Bool, "1"),
    // Retention: whichever limit is reached first evicts the oldest.
    ("diagnostics_retention_days", Kind::Int(1), "7"),
    ("diagnostics_max_events", Kind::Int(100), "50000"),
    ("diagnostics_max_bytes", Kind::Int(1048576), "536870912"),
    // Optional forwarding target; empty means "never send".
    ("diagnostics_tracker_dsn", Kind::Text, ""),
    // JSON object `{"web": "<dsn>", "server": "<dsn>", "plugin": "<dsn>"}`: a per-source
    // override of the default above, so splitting projects later needs no rebuild.
    ("diagnostics_tracker_dsn_by_source", Kind::Text, ""),
    // Unix milliseconds; 0 (or the past) means closed.
    ("diagnostics_window_deadline", Kind::Int(0), "0"),
    ("diagnostics_rate_per_minute", Kind::Int(1), "60"),
    ("diagnostics_rate_per_hour", Kind::Int(1), "1000"),
];

/// The project's own tracker. When an instance is pointed here (built in, or pasted from the
/// documentation) the panel says so explicitly: it is the maintainer's service, not a third party.
pub const OFFICIAL_TRACKER_HOST: &str = "https://sentry.taosaka.com";

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

    /// The scrubbing profile the ingest applies.
    pub fn scrub_profile(&self) -> crate::diagnostics::scrub::Profile {
        crate::diagnostics::scrub::Profile::from_flag(self.flag("diagnostics_include_private_data"))
    }

    pub fn retention(&self) -> Retention {
        Retention {
            days: self.number("diagnostics_retention_days"),
            max_events: self.number("diagnostics_max_events"),
            max_bytes: self.number("diagnostics_max_bytes"),
        }
    }

    /// The tracker to forward to. A DSN compiled into the binary is the fallback for images the
    /// maintainer publishes; an instance that configured its own DSN always wins, and neither set
    /// means "never forward".
    pub fn tracker_dsn(&self) -> Option<&str> {
        let dsn = self.text("diagnostics_tracker_dsn").trim();
        if !dsn.is_empty() {
            return Some(dsn);
        }
        // A runtime variable wins over the compiled-in one so a dev container can point at a
        // different project without a rebuild; the compiled-in value is what a published image
        // carries. Read once: this is consulted on every settings load.
        static RUNTIME: OnceLock<Option<String>> = OnceLock::new();
        let runtime = RUNTIME.get_or_init(|| {
            std::env::var("LANRURUGI_TRACKER_DSN")
                .ok()
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
        });
        if let Some(dsn) = runtime.as_deref() {
            return Some(dsn);
        }
        const BUILT_IN: &str = match option_env!("LANRURUGI_TRACKER_DSN") {
            Some(dsn) => dsn,
            None => "",
        };
        (!BUILT_IN.trim().is_empty()).then_some(BUILT_IN.trim())
    }

    /// Whether a tracker came from the build rather than from this instance's own settings.
    pub fn tracker_is_built_in(&self) -> bool {
        self.text("diagnostics_tracker_dsn").trim().is_empty() && self.tracker_dsn().is_some()
    }

    /// The per-source overrides, as `source -> DSN`. Invalid JSON is treated as "no overrides":
    /// `save` rejects it, so this only matters for a value written by an older build.
    pub fn tracker_routes(&self) -> std::collections::BTreeMap<String, String> {
        let raw = self.text("diagnostics_tracker_dsn_by_source").trim();
        if raw.is_empty() {
            return Default::default();
        }
        serde_json::from_str(raw).unwrap_or_default()
    }

    /// The tracker one event's source should go to: its own override, else the default. `None`
    /// means that source has no tracker and nothing may be sent for it.
    pub fn tracker_dsn_for(&self, source: &str) -> Option<String> {
        if let Some(dsn) = self.tracker_routes().get(source) {
            if !dsn.trim().is_empty() {
                return Some(dsn.trim().to_string());
            }
        }
        self.tracker_dsn().map(str::to_string)
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
        object.insert(
            "tracker_built_in".to_string(),
            Value::Bool(self.tracker_is_built_in()),
        );
        object.insert(
            "tracker_official".to_string(),
            Value::Bool(
                self.tracker_dsn()
                    .and_then(crate::diagnostics::forward::TrackerDsn::parse)
                    .map(|dsn| dsn.host == OFFICIAL_TRACKER_HOST)
                    .unwrap_or(false),
            ),
        );
        // The numeric project id, so the UI can link straight to that project's issue list — the
        // ingest response only returns an event id, and reading issues back would need a second
        // credential we deliberately do not keep.
        object.insert(
            "tracker_project".to_string(),
            self.tracker_dsn()
                .and_then(crate::diagnostics::forward::TrackerDsn::parse)
                .map(|dsn| Value::String(dsn.project))
                .unwrap_or(Value::Null),
        );
        object.insert(
            "tracker_routes".to_string(),
            Value::Object(
                self.tracker_routes()
                    .iter()
                    .filter_map(|(source, dsn)| {
                        let dsn = crate::diagnostics::forward::TrackerDsn::parse(dsn)?;
                        Some((source.clone(), Value::String(dsn.host)))
                    })
                    .collect(),
            ),
        );
        object.insert(
            "tracker_host".to_string(),
            match self
                .tracker_dsn()
                .and_then(crate::diagnostics::forward::TrackerDsn::parse)
            {
                Some(dsn) => Value::String(dsn.host),
                None => Value::Null,
            },
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
        Kind::Text if key == "diagnostics_tracker_dsn_by_source" => {
            if text.is_empty() {
                return Ok(String::new());
            }
            let routes: std::collections::BTreeMap<String, String> = serde_json::from_str(&text)
                .map_err(|_| {
                    "\"diagnostics_tracker_dsn_by_source\" must be a JSON object of source → DSN."
                        .to_string()
                })?;
            for (source, dsn) in &routes {
                if !matches!(source.as_str(), "web" | "server" | "plugin") {
                    return Err(format!(
                        "\"{source}\" is not a diagnostics source (web, server or plugin)."
                    ));
                }
                if url::Url::parse(dsn.trim()).is_err() {
                    return Err(format!("the DSN for \"{source}\" is not a URL."));
                }
            }
            Ok(serde_json::to_string(&routes).unwrap_or_default())
        }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn settings(pairs: &[(&str, &str)]) -> DiagnosticsSettings {
        let stored: HashMap<String, String> = pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
            .collect();
        DiagnosticsSettings::from_map(&stored)
    }

    #[test]
    fn a_source_override_wins_over_the_default() {
        let settings = settings(&[
            (
                "diagnostics_tracker_dsn",
                "https://default@sentry.example/1",
            ),
            (
                "diagnostics_tracker_dsn_by_source",
                r#"{"web":"https://web@sentry.example/2"}"#,
            ),
        ]);
        assert_eq!(
            settings.tracker_dsn_for("web").as_deref(),
            Some("https://web@sentry.example/2")
        );
        assert_eq!(
            settings.tracker_dsn_for("server").as_deref(),
            Some("https://default@sentry.example/1")
        );
    }

    #[test]
    fn no_tracker_means_no_route_for_any_source() {
        let settings = settings(&[]);
        assert!(settings.tracker_dsn_for("web").is_none());
        assert!(settings.tracker_dsn_for("server").is_none());
    }

    #[test]
    fn a_malformed_table_falls_back_to_the_default() {
        let settings = settings(&[
            (
                "diagnostics_tracker_dsn",
                "https://default@sentry.example/1",
            ),
            ("diagnostics_tracker_dsn_by_source", "not json"),
        ]);
        assert_eq!(
            settings.tracker_dsn_for("web").as_deref(),
            Some("https://default@sentry.example/1")
        );
    }
}
