//! Sharing settings: the typed view and its parsing rules.
//!
//! Storage stays in the API crate (Redis, validation, the admin route); the engine only ever reads
//! this filled-in view.

use std::collections::HashMap;

use lanrurugi_share_proto::messages::Permissions;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub enum Kind {
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
pub const FIELDS: &[(&str, Kind, &str)] = &[
    ("share_public_services", Kind::Bool, "1"),
    ("share_nodes", Kind::Text, ""),
    ("share_relay_urls", Kind::Text, ""),
    ("share_pkarr_relays", Kind::Text, ""),
    ("share_node_rendezvous", Kind::Bool, "0"),
    ("share_node_relay", Kind::Bool, "0"),
    ("share_node_relay_tls", Kind::Text, "proxy"),
    // Fixed by default so a reverse proxy can point at it; empty picks a free one per start.
    ("share_node_relay_port", Kind::Port, "11205"),
    ("share_node_relay_cert", Kind::Text, ""),
    ("share_node_relay_key", Kind::Text, ""),
    // Bytes per second per relayed client: 10 MiB/s, empty for no limit.
    ("share_node_relay_rate", Kind::OptInt(1), "10485760"),
    ("share_node_relay_allow", Kind::Text, ""),
    ("share_self_relay_url", Kind::Text, ""),
    ("share_portmap", Kind::Bool, "1"),
    // Random by default, and rotated below: a fixed share port is an address a scanner can sit on.
    // Pinning it is opt-in for a manual port forward (turn rotation off too). A fixed port that is
    // already taken still falls back to a free one, with a warning.
    ("share_udp_port", Kind::Port, ""),
    // Addresses this host is reachable at from outside, `host:port` each, for a router that
    // forwards ports but has no UPnP/PCP. Empty lets iroh work them out.
    ("share_external_addrs", Kind::Text, ""),
    // Minutes between automatic port rotations, jittered ±25% and clamped to 6–12 hours; 0 pins the
    // ports. On by default: a port that never moves is a fixed target. Rotation never cuts a live
    // transfer (it waits for it), so the default does not cost resumability.
    ("share_port_rotate", Kind::Int(0), "480"),
    // Empty binds every interface; a name (`tun0`) pins the endpoint to it, so sharing can be
    // kept inside a VPN tunnel while the rest of the app still serves the LAN.
    ("share_bind_interface", Kind::Text, ""),
    ("share_public_url", Kind::Text, ""),
    // Where the download endpoints are reached: a share page behind a CDN proxy (Cloudflare, a
    // load balancer) sets this to a direct origin, because a multi-gigabyte response through the
    // proxy times out. Empty keeps every link on the page's own origin.
    // Peers whose `X-Forwarded-For` is believed. Loopback only by default: a private-range peer is
    // just as able to write that header itself, so a LAN client could otherwise pose as any address
    // the share's `ip_allow` / first-N rule admits.
    ("share_trusted_proxies", Kind::Text, "127.0.0.1/32, ::1/128"),
    (
        "share_default_permissions",
        Kind::Text,
        "import,read,download",
    ),
    ("share_default_download_limit", Kind::OptInt(1), ""),
    // Ratio of served import bytes to the share's own size; "0" or empty disables the target.
    ("share_default_target_ratio", Kind::Text, ""),
    // "stop" or "throttle" once the target is reached.
    ("share_default_ratio_action", Kind::Text, "stop"),
    // Bytes per second once throttled; 0 pauses instead of stopping.
    ("share_default_ratio_rate", Kind::OptInt(0), ""),
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

    pub fn from_stored(stored: &HashMap<String, String>) -> Self {
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
