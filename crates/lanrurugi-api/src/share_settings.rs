//! Sharing settings (specs/017-archive-sharing data-model.md "Settings"). Stored as `share_*`
//! fields of `LRR_CONFIG`, but read and written only through `/api/shares/settings`, never the
//! general `/settings`: read-only tokens can read that one, and these include a proxy URL that may
//! carry credentials.

use std::collections::HashMap;
use std::net::SocketAddr;

use deadpool_redis::redis::AsyncCommands;
use lanrurugi_share_proto::messages::Permissions;
use lanrurugi_storage::keys::CONFIG_KEY;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::AppState;

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Bool,
    /// 1–65535.
    Port,
    /// A whole number of at least this value.
    Int(i64),
    /// Empty, or a whole number of at least this value.
    OptInt(i64),
    Text,
}

/// `(field, kind, default)`.
const FIELDS: &[(&str, Kind, &str)] = &[
    ("share_public_services", Kind::Bool, "1"),
    ("share_nodes", Kind::Text, ""),
    ("share_relay_urls", Kind::Text, ""),
    ("share_pkarr_relays", Kind::Text, ""),
    ("share_node_rendezvous", Kind::Bool, "0"),
    ("share_node_relay", Kind::Bool, "0"),
    ("share_node_relay_tls", Kind::Text, "acme"),
    ("share_node_relay_port", Kind::Port, "11205"),
    ("share_node_relay_cert", Kind::Text, ""),
    ("share_node_relay_key", Kind::Text, ""),
    // Bytes per second per relayed client: 10 MiB/s, empty for no limit.
    ("share_node_relay_rate", Kind::OptInt(1), "10485760"),
    ("share_node_relay_allow", Kind::Text, ""),
    ("share_self_relay_url", Kind::Text, ""),
    ("share_portmap", Kind::Bool, "1"),
    ("share_external_addrs", Kind::Text, ""),
    ("share_udp_port", Kind::Port, "11204"),
    ("share_public_url", Kind::Text, ""),
    (
        "share_default_permissions",
        Kind::Text,
        "import,read,download",
    ),
    ("share_default_download_limit", Kind::OptInt(1), ""),
    ("share_default_ip_allow", Kind::Text, ""),
    ("share_default_ip_first_n", Kind::OptInt(1), ""),
    ("share_default_expiry_days", Kind::OptInt(1), ""),
    ("share_default_embed", Kind::Bool, "0"),
    ("share_default_allow_helpers", Kind::Bool, "0"),
    ("share_upload_limit", Kind::OptInt(1), ""),
    ("share_max_peers", Kind::Int(1), "8"),
    ("share_helper_upload_limit", Kind::OptInt(1), ""),
    ("share_helper_max_peers", Kind::Int(1), "4"),
    ("share_alt_upload_limit", Kind::OptInt(0), ""),
    ("share_alt_max_peers", Kind::OptInt(1), ""),
    ("share_alt_helper_upload_limit", Kind::OptInt(0), ""),
    ("share_alt_helper_max_peers", Kind::OptInt(1), ""),
    ("share_alt_schedule", Kind::Text, ""),
    ("share_alt_manual", Kind::Bool, "0"),
    ("share_import_concurrency", Kind::Int(1), "3"),
];

/// The alternative-speed schedule (FR-042).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AltSchedule {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub windows: Vec<ScheduleWindow>,
    /// ISO weekdays, 1 = Monday. Empty means every day.
    #[serde(default)]
    pub days: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScheduleWindow {
    pub from: String,
    pub to: String,
}

/// Minutes after midnight of an `HH:MM` string.
pub fn parse_hhmm(value: &str) -> Option<u32> {
    let (h, m) = value.split_once(':')?;
    let (h, m): (u32, u32) = (h.parse().ok()?, m.parse().ok()?);
    (h < 24 && m < 60).then_some(h * 60 + m)
}

/// Every sharing setting, defaults filled in.
#[derive(Debug, Clone)]
pub struct ShareSettings {
    values: HashMap<&'static str, String>,
}

impl ShareSettings {
    /// Settings from plain values, defaults filled in; node-only mode has no Redis to read.
    pub fn from_map(stored: &HashMap<String, String>) -> Self {
        Self::from_stored(stored)
    }

    fn from_stored(stored: &HashMap<String, String>) -> Self {
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

    pub fn number(&self, name: &str) -> Option<u64> {
        self.text(name).trim().parse().ok()
    }

    /// Non-empty lines (or comma-separated entries) of a list field.
    pub fn list(&self, name: &str) -> Vec<String> {
        split_list(self.text(name))
    }

    pub fn public_url(&self) -> Option<String> {
        let url = self.text("share_public_url").trim().trim_end_matches('/');
        (!url.is_empty()).then(|| url.to_string())
    }

    pub fn default_permissions(&self) -> Permissions {
        parse_permissions(self.text("share_default_permissions")).unwrap_or(Permissions::ALL)
    }

    pub fn alt_schedule(&self) -> AltSchedule {
        serde_json::from_str(self.text("share_alt_schedule")).unwrap_or_default()
    }

    pub fn external_addrs(&self) -> Vec<SocketAddr> {
        self.list("share_external_addrs")
            .iter()
            .filter_map(|a| a.parse().ok())
            .collect()
    }

    /// The JSON shape `GET /api/shares/settings` returns: typed values.
    pub fn to_json(&self) -> serde_json::Map<String, Value> {
        FIELDS
            .iter()
            .map(|(name, kind, _)| {
                let raw = self.text(name);
                let value = match kind {
                    Kind::Bool => Value::Bool(raw == "1"),
                    Kind::Port | Kind::Int(_) | Kind::OptInt(_) => {
                        raw.parse::<i64>().map(Value::from).unwrap_or(Value::Null)
                    }
                    Kind::Text => Value::String(raw.to_string()),
                };
                (name.to_string(), value)
            })
            .collect()
    }
}

pub async fn load(state: &AppState) -> ShareSettings {
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
    ShareSettings::from_stored(&stored)
}

/// Validates and writes the given fields. Unknown fields and invalid values reject the whole
/// update, so a half-applied configuration never exists.
pub async fn save(
    state: &AppState,
    body: &serde_json::Map<String, Value>,
) -> Result<ShareSettings, String> {
    let mut writes = Vec::with_capacity(body.len());
    for (key, value) in body {
        let stored = validate_field(key, value)?;
        writes.push((key.clone(), stored));
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

fn validate_field(key: &str, value: &Value) -> Result<String, String> {
    let Some((_, kind, _)) = FIELDS.iter().find(|(name, _, _)| *name == key) else {
        return Err(format!("Unknown sharing setting: \"{key}\"."));
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
        Kind::Port => match text.parse::<u32>() {
            Ok(p) if (1..=65535).contains(&p) => Ok(p.to_string()),
            _ => Err(format!("\"{key}\" must be a port number (1–65535).")),
        },
        Kind::Int(min) => match text.parse::<i64>() {
            Ok(n) if n >= *min && (key != "share_import_concurrency" || n <= 16) => {
                Ok(n.to_string())
            }
            _ => Err(format!(
                "\"{key}\" must be a whole number of at least {min}."
            )),
        },
        Kind::OptInt(min) => {
            if text.is_empty() {
                return Ok(String::new());
            }
            match text.parse::<i64>() {
                Ok(n) if n >= *min => Ok(n.to_string()),
                _ => Err(format!(
                    "\"{key}\" must be empty or a whole number of at least {min}."
                )),
            }
        }
        Kind::Text => validate_text(key, text),
    }
}

fn validate_text(key: &str, text: String) -> Result<String, String> {
    let bad = |what: &str| Err(format!("\"{key}\": {what}"));
    match key {
        "share_public_url" if !text.is_empty() => match url::Url::parse(&text) {
            Ok(u) if u.scheme() == "https" || u.scheme() == "http" => {
                Ok(text.trim_end_matches('/').to_string())
            }
            _ => bad("must be an http:// or https:// address"),
        },
        "share_self_relay_url" if !text.is_empty() => match canonical_https_url(&text) {
            Some(url) => Ok(url.trim_end_matches('/').to_string()),
            None => bad("must be an https:// address"),
        },
        "share_relay_urls" | "share_pkarr_relays" => {
            let mut out = Vec::new();
            for entry in split_list(&text) {
                match canonical_https_url(&entry) {
                    Some(url) if !out.contains(&url) => out.push(url),
                    Some(_) => {}
                    None => return bad(&format!("{entry:?} is not an https:// address")),
                }
            }
            Ok(out.join("\n"))
        }
        "share_nodes" => {
            let mut out = Vec::new();
            for entry in split_list(&text) {
                // The prefix is optional when typing; stored strings always carry it.
                let entry = if entry.starts_with("lrrg-node:") {
                    entry
                } else {
                    format!("lrrg-node:{entry}")
                };
                if parse_node_string(&entry).is_none() {
                    return bad(&format!(
                        "{entry:?} is not an lrrg-node:<id>@<host:port> string"
                    ));
                }
                out.push(entry);
            }
            Ok(out.join("\n"))
        }
        "share_node_relay_allow" => {
            for entry in split_list(&text) {
                if crate::share_net::nodes::parse_allow_entry(&entry).is_none() {
                    return bad(&format!("{entry:?} is not an endpoint id or node string"));
                }
            }
            Ok(text)
        }
        "share_node_relay_tls" => match text.as_str() {
            "acme" | "files" | "proxy" => Ok(text),
            _ => bad("must be acme, files or proxy"),
        },
        "share_external_addrs" => {
            for entry in split_list(&text) {
                if entry.parse::<SocketAddr>().is_err() {
                    return bad(&format!("{entry:?} is not an ip:port address"));
                }
            }
            Ok(text)
        }
        "share_default_permissions" => match parse_permissions(&text) {
            Some(p) if !p.is_empty() => Ok(text),
            _ => bad("select at least one of import, read, download"),
        },
        "share_default_ip_allow" => {
            for entry in split_list(&text) {
                if entry.parse::<std::net::IpAddr>().is_err()
                    && entry.parse::<ipnet::IpNet>().is_err()
                {
                    return bad(&format!("{entry:?} is not an IP address or range"));
                }
            }
            Ok(text)
        }
        "share_alt_schedule" => {
            if text.is_empty() {
                return Ok(text);
            }
            let schedule: AltSchedule =
                serde_json::from_str(&text).map_err(|e| format!("\"{key}\": {e}"))?;
            if schedule
                .windows
                .iter()
                .any(|w| parse_hhmm(&w.from).is_none() || parse_hhmm(&w.to).is_none())
            {
                return bad("times must be HH:MM");
            }
            if schedule.days.iter().any(|d| !(1..=7).contains(d)) {
                return bad("days must be 1 (Monday) to 7 (Sunday)");
            }
            Ok(serde_json::to_string(&schedule).expect("schedule serialises"))
        }
        _ => Ok(text),
    }
}

/// An https address in one canonical form: `https://` added when missing, the host lowercased
/// and without a trailing dot, so `relay.example.org`, `https://relay.example.org./` and
/// `HTTPS://Relay.Example.org` all name one relay. A path (a pkarr relay's `/pkarr`) is kept.
/// `None` for any other scheme, or no host.
pub fn canonical_https_url(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    let mut url = url::Url::parse(&with_scheme).ok()?;
    if url.scheme() != "https" {
        return None;
    }
    let host = url.host_str()?.trim_end_matches('.').to_ascii_lowercase();
    if host.is_empty() {
        return None;
    }
    url.set_host(Some(&host)).ok()?;
    url.set_fragment(None);
    Some(url.to_string())
}

pub fn split_list(text: &str) -> Vec<String> {
    text.split([',', '\n'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

pub fn parse_permissions(text: &str) -> Option<Permissions> {
    let mut p = Permissions::default();
    for part in split_list(text) {
        match part.as_str() {
            "import" => p.import = true,
            "read" => p.read = true,
            "download" => p.download = true,
            _ => return None,
        }
    }
    Some(p)
}

/// `lrrg-node:<endpoint_id>@<host:port>` → `(endpoint_id, host:port)`.
pub fn parse_node_string(entry: &str) -> Option<(String, String)> {
    let rest = entry.trim().strip_prefix("lrrg-node:")?;
    let (id, addr) = rest.split_once('@')?;
    (!id.is_empty() && id.chars().all(|c| c.is_ascii_alphanumeric()) && addr.contains(':'))
        .then(|| (id.to_string(), addr.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn relay_addresses_in_any_common_form_are_one_relay() {
        for input in [
            "aps1-1.relay.n0.iroh.link",
            "https://aps1-1.relay.n0.iroh.link",
            "https://aps1-1.relay.n0.iroh.link/",
            "https://aps1-1.relay.n0.iroh.link./",
            " HTTPS://APS1-1.Relay.N0.Iroh.Link ",
        ] {
            assert_eq!(
                canonical_https_url(input).as_deref(),
                Some("https://aps1-1.relay.n0.iroh.link/"),
                "{input}"
            );
        }
        assert_eq!(
            canonical_https_url("dns.iroh.link/pkarr").as_deref(),
            Some("https://dns.iroh.link/pkarr")
        );
        assert_eq!(
            canonical_https_url("relay.example.org:8443").as_deref(),
            Some("https://relay.example.org:8443/")
        );
        assert_eq!(canonical_https_url("http://relay.example.org"), None);
        assert_eq!(canonical_https_url(""), None);
    }

    fn settings(pairs: &[(&str, &str)]) -> ShareSettings {
        ShareSettings::from_stored(
            &pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        )
    }

    #[test]
    fn defaults_fill_unset_fields() {
        let s = settings(&[]);
        assert!(s.flag("share_public_services"));
        assert_eq!(s.number("share_udp_port"), Some(11204));
        assert_eq!(s.number("share_import_concurrency"), Some(3));
        assert_eq!(s.default_permissions(), Permissions::ALL);
        assert_eq!(s.public_url(), None);
    }

    #[test]
    fn validation_accepts_good_values_and_rejects_bad_ones() {
        let ok = |k: &str, v: Value| validate_field(k, &v);
        assert_eq!(ok("share_udp_port", Value::from(443)).unwrap(), "443");
        assert!(ok("share_udp_port", Value::from(0)).is_err());
        assert!(ok("share_import_concurrency", Value::from(17)).is_err());
        assert_eq!(ok("share_upload_limit", Value::from("")).unwrap(), "");
        assert_eq!(ok("share_alt_upload_limit", Value::from(0)).unwrap(), "0");
        assert!(ok("share_upload_limit", Value::from(0)).is_err());
        assert_eq!(
            ok("share_public_url", Value::from("https://lib.example.org/")).unwrap(),
            "https://lib.example.org"
        );
        assert!(ok("share_public_url", Value::from("ftp://x")).is_err());
        assert!(ok("share_self_relay_url", Value::from("http://x.example")).is_err());
        assert!(ok(
            "share_nodes",
            Value::from("lrrg-node:abc@203.0.113.1:443\n")
        )
        .is_ok());
        assert_eq!(
            ok("share_nodes", Value::from("abc@1.2.3.4:5")).unwrap(),
            "lrrg-node:abc@1.2.3.4:5"
        );
        assert!(ok("share_nodes", Value::from("abc")).is_err());
        assert_eq!(
            ok(
                "share_relay_urls",
                Value::from("relay.example.org\nhttps://relay.example.org./")
            )
            .unwrap(),
            "https://relay.example.org/"
        );
        assert!(ok(
            "share_external_addrs",
            Value::from("203.0.113.7:443, [2001:db8::1]:443")
        )
        .is_ok());
        assert!(ok("share_external_addrs", Value::from("203.0.113.7")).is_err());
        assert!(ok("share_default_permissions", Value::from("read,download")).is_ok());
        assert!(ok("share_default_permissions", Value::from("")).is_err());
        assert!(ok("share_default_permissions", Value::from("write")).is_err());
        assert!(ok(
            "share_alt_schedule",
            Value::from(
                r#"{"enabled":true,"windows":[{"from":"01:00","to":"08:00"}],"days":[1,7]}"#
            )
        )
        .is_ok());
        assert!(ok(
            "share_alt_schedule",
            Value::from(r#"{"windows":[{"from":"25:00","to":"08:00"}]}"#)
        )
        .is_err());
        assert!(ok("share_nonsense", Value::from(1)).is_err());
    }

    #[test]
    fn node_strings_parse() {
        assert_eq!(
            parse_node_string("lrrg-node:abc123@node.example.org:443"),
            Some(("abc123".into(), "node.example.org:443".into()))
        );
        assert_eq!(parse_node_string("lrrg-node:@x:1"), None);
    }

    #[test]
    fn hhmm_parses() {
        assert_eq!(parse_hhmm("01:30"), Some(90));
        assert_eq!(parse_hhmm("24:00"), None);
    }
}
