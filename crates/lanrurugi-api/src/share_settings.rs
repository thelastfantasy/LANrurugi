//! Sharing settings (specs/017-archive-sharing data-model.md "Settings"). Stored as `share_*`
//! fields of `LRR_CONFIG`, but read and written only through `/api/shares/settings`, never the
//! general `/settings`: read-only tokens can read that one, and these include a proxy URL that may
//! carry credentials.

use std::collections::HashMap;

use deadpool_redis::redis::AsyncCommands;
use lanrurugi_storage::keys::CONFIG_KEY;
use serde_json::Value;

use crate::AppState;

pub use lanrurugi_share::settings::{
    canonical_https_url, parse_hhmm, parse_node_string, parse_permissions, split_list, AltSchedule,
    ScheduleWindow, ShareSettings,
};
use lanrurugi_share::settings::{Kind, FIELDS};

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
        // Empty asks for a random free port on every start (and every rotation); a number pins it.
        Kind::Port if text.trim().is_empty() => Ok(String::new()),
        Kind::Port => match text.parse::<u32>() {
            Ok(p) if (1..=65535).contains(&p) => Ok(p.to_string()),
            _ => Err(format!(
                "\"{key}\" must be a port number (1–65535), or empty for a random port."
            )),
        },
        // A rotation sooner than six hours is a schedule a scanner can sit on, and one further
        // out than twelve barely rotates at all; 0 still means off.
        Kind::Int(_) if key == "share_port_rotate" => match text.parse::<i64>() {
            Ok(0) => Ok("0".into()),
            Ok(n) if (360..=720).contains(&n) => Ok(n.to_string()),
            _ => Err(format!(
                "\"{key}\" must be 0 (off) or between 360 and 720 minutes (6 to 12 hours)."
            )),
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
        "share_default_target_ratio" if !text.is_empty() => match text.trim().parse::<f32>() {
            Ok(ratio) if ratio.is_finite() && ratio >= 0.0 => Ok(if ratio == 0.0 {
                String::new()
            } else {
                format!("{ratio:.2}")
            }),
            _ => bad("must be a number such as 1.5"),
        },
        "share_default_ratio_action" => match text.trim() {
            "stop" | "throttle" => Ok(text.trim().to_string()),
            _ => bad("must be \"stop\" or \"throttle\""),
        },
        "share_nodes" => {
            let mut out = Vec::new();
            for entry in split_list(&text) {
                // A node code is the preferred form and carries no address at all; a full
                // `lrrg-node:` string still parses, so an existing configuration keeps working.
                if let Ok(code) = lanrurugi_share_proto::node_code::NodeCode::parse_prefixed(&entry)
                {
                    out.push(code.prefixed());
                    continue;
                }
                let entry = if entry.starts_with("lrrg-node:") {
                    entry
                } else {
                    format!("lrrg-node:{entry}")
                };
                if parse_node_string(&entry).is_none() {
                    return bad(&format!(
                        "{entry:?} is neither a node code nor an lrrg-node:<id>@<host:port> string"
                    ));
                }
                out.push(entry);
            }
            // The same node written two ways (with and without the prefix, any case) is one
            // node; a list that names it twice only ever produces duplicate dials.
            let mut seen = std::collections::HashSet::new();
            out.retain(|entry| seen.insert(entry.to_ascii_lowercase()));
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
        // A name that does not exist yet is accepted: a tunnel often comes up after the server,
        // and pinning the share to it now is the point. The endpoint stays off until it appears
        // (and comes back by itself then), so this can never fall back to the physical link.
        "share_bind_interface" if !text.is_empty() => {
            if !lanrurugi_core::http_client::interface_binding_supported() {
                return bad("interface binding is not supported on this platform");
            }
            let valid =
                text.len() <= 15 && !text.contains(['/', ' ', ':']) && !text.starts_with('-');
            if valid {
                Ok(text)
            } else {
                bad("is not a valid network interface name")
            }
        }
        "share_external_addrs" => {
            for entry in split_list(&text) {
                // Only literal addresses: this is what gets published as a direct address, and a
                // hostname here would be resolved once and published as whatever it pointed at.
                let ok = entry.parse::<std::net::SocketAddr>().is_ok()
                    || entry.parse::<std::net::IpAddr>().is_ok();
                if !ok {
                    return bad(&format!(
                        "{entry:?} is not an address such as 203.0.113.5:11205"
                    ));
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

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_share_proto::messages::Permissions;

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
        // Random by default: a fixed port is opt-in for people who forwarded one.
        assert_eq!(s.number("share_udp_port"), None);
        assert_eq!(s.text("share_udp_port"), "");
        // Rotation is on by default; 0 is what pins the ports.
        assert_eq!(s.number("share_port_rotate"), Some(480));
        assert_eq!(s.number("share_import_concurrency"), Some(3));
        assert_eq!(s.default_permissions(), Permissions::ALL);
        assert_eq!(s.public_url(), None);
    }

    #[test]
    fn validation_accepts_good_values_and_rejects_bad_ones() {
        let ok = |k: &str, v: Value| validate_field(k, &v);
        assert_eq!(ok("share_udp_port", Value::from(443)).unwrap(), "443");
        assert_eq!(
            ok(
                "share_external_addrs",
                Value::from("203.0.113.5:11205\n[2001:db8::1]:11205")
            )
            .unwrap(),
            "203.0.113.5:11205\n[2001:db8::1]:11205"
        );
        assert!(ok("share_external_addrs", Value::from("not an address")).is_err());
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
        // The same node written three ways is still one node: a list that names it twice only
        // produces duplicate dials, and the code form means the spelling can differ.
        assert_eq!(
            ok(
                "share_nodes",
                Value::from("3F2K-9Q4M-7XTD-08RC\nlrrg-node:3f2k9q4m7xtd08rc\n3F2K 9Q4M 7XTD 08RC")
            )
            .unwrap(),
            "lrrg-node:3F2K-9Q4M-7XTD-08RC"
        );
        assert_eq!(
            ok(
                "share_relay_urls",
                Value::from("relay.example.org\nhttps://relay.example.org./")
            )
            .unwrap(),
            "https://relay.example.org/"
        );
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
