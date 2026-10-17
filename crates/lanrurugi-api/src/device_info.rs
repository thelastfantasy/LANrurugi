//! Assembles a request's full [`lanrurugi_storage::device_info::DeviceInfo`] — `User-Agent`
//! parsing (`woothee`), IP geolocation (`crate::geoip`), and whatever the client itself reported
//! — kept in this crate (not `lanrurugi-storage`, which only owns the resulting shape) since all
//! three sources are squarely an API-layer / HTTP-request-boundary concern.

use lanrurugi_storage::device_info::{ClientReportedInfo, DeviceInfo, UserAgentInfo};
use woothee::parser::Parser;

/// `Parser::new()` rebuilds `woothee`'s static dataset on every call — cheap (its own benchmark:
/// ~0ns, effectively a `lazy_static` lookup, see `woothee`'s README) but still pointless to redo
/// per-request when one shared instance works identically. `Parser` itself carries no per-call
/// state.
fn parser() -> &'static Parser {
    static PARSER: std::sync::OnceLock<Parser> = std::sync::OnceLock::new();
    PARSER.get_or_init(Parser::new)
}

/// `None` if `user_agent` is absent/empty, or `woothee` couldn't classify it at all (a `Parser`
/// crawler/bot match still returns `Some` — "crawler" is itself a meaningful category, not a
/// parse failure).
pub fn parse_user_agent(user_agent: Option<&str>) -> Option<UserAgentInfo> {
    let ua = user_agent?;
    if ua.trim().is_empty() {
        return None;
    }
    let result = parser().parse(ua)?;
    Some(UserAgentInfo {
        category: result.category.to_string(),
        os: result.os.to_string(),
        os_version: result.os_version.to_string(),
        browser: result.name.to_string(),
        browser_version: result.version.to_string(),
        browser_type: result.browser_type.to_string(),
        vendor: result.vendor.to_string(),
    })
}

/// Builds the full [`DeviceInfo`] for one request — `user_agent`/`client_ip` come from this
/// request's own headers/`AuthContext`; `client_reported` is whatever the client itself submitted
/// alongside the request (currently: `login`/`refresh`'s optional form fields — see
/// `crate::login::ClientReportedFields` — `None` for any request shape that doesn't carry one,
/// e.g. every `guest_visitor` request). Returns `None` (rather than `Some(DeviceInfo::default())`)
/// when every part came back empty, so a caller can `Option`-chain this the same way the old
/// UA-only `parse` function worked.
pub fn build(
    user_agent: Option<&str>,
    client_ip: Option<&str>,
    client_reported: Option<ClientReportedInfo>,
) -> Option<DeviceInfo> {
    let info = DeviceInfo {
        user_agent: parse_user_agent(user_agent),
        geo: client_ip.and_then(crate::geoip::lookup),
        client_reported,
    };
    if info.is_empty() {
        None
    } else {
        Some(info)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_a_mobile_safari_user_agent() {
        let ua = "Mozilla/5.0 (iPhone; CPU iPhone OS 17_0 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.0 Mobile/15E148 Safari/604.1";
        let info = parse_user_agent(Some(ua)).expect("should parse a real iPhone Safari UA");
        assert_eq!(info.category, "smartphone");
    }

    #[test]
    fn recognizes_a_desktop_chrome_user_agent() {
        let ua = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";
        let info = parse_user_agent(Some(ua)).expect("should parse a real desktop Chrome UA");
        assert_eq!(info.category, "pc");
    }

    #[test]
    fn none_for_missing_or_empty_user_agent() {
        assert!(parse_user_agent(None).is_none());
        assert!(parse_user_agent(Some("")).is_none());
        assert!(parse_user_agent(Some("   ")).is_none());
    }

    #[test]
    fn build_is_none_when_every_part_is_empty() {
        assert!(build(None, None, None).is_none());
    }

    #[test]
    fn build_is_some_when_only_client_reported_is_present() {
        let reported = ClientReportedInfo {
            timezone: Some("Asia/Tokyo".to_string()),
            ..Default::default()
        };
        let info = build(None, None, Some(reported)).expect("client_reported alone is enough");
        assert!(info.user_agent.is_none());
        assert!(info.geo.is_none());
        assert_eq!(
            info.client_reported.unwrap().timezone.as_deref(),
            Some("Asia/Tokyo")
        );
    }
}
