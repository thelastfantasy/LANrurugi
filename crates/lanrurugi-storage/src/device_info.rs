//! [`DeviceInfo`] — a `User-Agent`-derived + IP-geolocated + (where available) client-reported
//! device summary attached to [`crate::activity::ActivityEntry`] and
//! [`crate::refresh_tokens::RefreshTokenRecord`]. Parsing the raw header (via `woothee`) and
//! looking up the geo database happen in `lanrurugi-api`, close to the HTTP request boundary —
//! this crate only owns the resulting shape, since both record types living here need to store it.

use std::fmt::Display;
use std::str::FromStr;

use serde::{Deserialize, Deserializer, Serialize};

/// Deserializes an `Option<T>` (`T: u32`/`f32`/`bool`/...) from either a bare string form
/// (`"2560"`) or an already-typed JSON token (`2560`), rather than requiring one specific source
/// shape.
///
/// Needed for two different round-trips of the exact same [`ClientReportedInfo`] type, each of
/// which hands this a different token kind:
///
/// - **Form decode** (`lanrurugi-api::login::LoginForm`'s `#[serde(flatten)] client_reported`
///   field, out of an `application/x-www-form-urlencoded` body): `serde_urlencoded`'s *top-level*
///   deserializer happily coerces a bare numeric-looking form value into `u32`/`f32`/`bool`, but
///   the `serde::de::value::MapDeserializer` that `#[serde(flatten)]` routes every remaining field
///   through internally does not — every value stays a `str`, and a plain
///   `#[derive(Deserialize)]` `Option<u32>` field fails with "invalid type: string ..., expected
///   u32" for a value as ordinary as `screen_width=2560` — confirmed live (a real login request
///   with these fields set 422'd) and reproduced in isolation against `serde_urlencoded` 0.7
///   directly, not a guess.
/// - **JSON round-trip** (this same struct also lives inside [`crate::activity::ActivityEntry`]/
///   [`crate::refresh_tokens::RefreshTokenRecord`], `serde_json`-serialized into Redis and read
///   back later): here the value was written as a real JSON number (`serde_json::to_string` on an
///   already-parsed `u32` never re-stringifies it), so a deserializer that *only* accepts a string
///   token fails the opposite way — "invalid type: integer `2560`, expected a string" — confirmed
///   live against a real stored `ActivityEntry` after the form-decode fix above was in place.
///
/// Routing through `serde_json::Value` first (rather than `Option<String>`) accepts both: a JSON
/// string is parsed via `T::from_str`, a JSON number/bool is converted directly.
fn from_str_opt<'de, D, T>(deserializer: D) -> Result<Option<T>, D::Error>
where
    D: Deserializer<'de>,
    T: FromStr,
    T::Err: Display,
{
    let opt: Option<serde_json::Value> = Option::deserialize(deserializer)?;
    match opt {
        None | Some(serde_json::Value::Null) => Ok(None),
        Some(serde_json::Value::String(s)) => {
            if s.is_empty() {
                Ok(None)
            } else {
                s.parse().map(Some).map_err(serde::de::Error::custom)
            }
        }
        Some(other) => {
            // A number/bool token: round-trip it through its own `Display` form (e.g. `2560`,
            // `true`) and feed that to the same `T::from_str` the string branch uses, rather than
            // hand-rolling a second conversion path per `T`.
            let as_string = match &other {
                serde_json::Value::Number(n) => n.to_string(),
                serde_json::Value::Bool(b) => b.to_string(),
                _ => {
                    return Err(serde::de::Error::custom(format!(
                        "unexpected value {other:?} for a string/number/bool field"
                    )))
                }
            };
            as_string
                .parse()
                .map(Some)
                .map_err(serde::de::Error::custom)
        }
    }
}

/// Every field `woothee::parser::WootheeResult` exposes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UserAgentInfo {
    /// "pc" | "smartphone" | "mobilephone" | "tablet" | "crawler" | "appliance" | "misc" |
    /// "UNKNOWN" — `woothee`'s own category string, not a closed enum, so a future `woothee`
    /// dataset update adding a new category never fails to deserialize an already-stored entry.
    pub category: String,
    pub os: String,
    /// `woothee`'s own OS version string (e.g. "17.0" for iOS 17, "10" for Windows 10).
    pub os_version: String,
    pub browser: String,
    /// The browser's own version as parsed from `User-Agent` alone (e.g. "120.0.0.0" for Chrome).
    /// A UA string can lie/omit this — `client_reported.browser_full_version` (Client Hints,
    /// Chromium-only) is more trustworthy when both are present.
    pub browser_version: String,
    /// "browser" | "crawler" | "misc" | "appliance" | "UNKNOWN" — `woothee`'s own coarse
    /// classification, independent of (and coarser than) `category` above.
    pub browser_type: String,
    /// The vendor/engine behind this UA where `woothee` can tell ("Google", "Microsoft", "Apple",
    /// "Mozilla Foundation", ..., or "UNKNOWN").
    pub vendor: String,
}

/// MaxMind GeoLite2-City lookup result for this request's `client_ip` — `None` fields mean the
/// database had no entry for that IP (private/reserved ranges, or a real gap in MaxMind's own
/// coverage), not a lookup failure; the whole struct is absent instead of present-with-all-`None`
/// when the database itself isn't installed at all (`lanrurugi_api::geoip`'s own docs) or the IP
/// failed to parse.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GeoInfo {
    /// ISO 3166-1 alpha-2 ("JP", "US", ...).
    pub country_code: Option<String>,
    pub country_name: Option<String>,
    pub city_name: Option<String>,
    /// GeoLite2's own subdivision (state/province/prefecture) — the most specific one it reports,
    /// e.g. "Tokyo" for a Tokyo IP, "California" for a California one.
    pub subdivision_name: Option<String>,
}

/// Client-reported browser/environment facts — never derivable server-side (no header carries
/// them), so this is only present when the client actually submitted them (`lanrurugi-api`'s
/// login/refresh handlers accept these as optional form fields; `guest_visitor` requests, which
/// carry no request body of this shape at all, never populate this). Every field is itself
/// optional since a given browser/privacy-mode may withhold any one of them (e.g.
/// `navigator.deviceMemory` is Chromium-only, unsupported entirely on Firefox/Safari).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ClientReportedInfo {
    /// `screen.width`/`screen.height` — the full physical screen, not the browser window.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub screen_width: Option<u32>,
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub screen_height: Option<u32>,
    /// `screen.availWidth`/`screen.availHeight` — screen size minus OS-reserved space (taskbar,
    /// dock, notch cutout), the actual usable area.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub screen_avail_width: Option<u32>,
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub screen_avail_height: Option<u32>,
    /// `window.outerWidth`/`window.outerHeight` — the browser chrome's own window size (including
    /// its own toolbars/tab strip), distinct from both the screen and the page viewport.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub window_outer_width: Option<u32>,
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub window_outer_height: Option<u32>,
    /// `window.innerWidth`/`window.innerHeight` — the actual page viewport a CSS media query sees.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub window_inner_width: Option<u32>,
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub window_inner_height: Option<u32>,
    /// `window.devicePixelRatio` — the real DPI-scaling factor (1 = standard, 2 = "Retina"/HiDPI).
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub device_pixel_ratio: Option<f32>,
    /// `screen.colorDepth`, bits.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub color_depth: Option<u32>,
    /// `screen.pixelDepth`, bits — usually equal to `color_depth`, kept separately since the spec
    /// allows them to differ.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub pixel_depth: Option<u32>,
    /// `screen.orientation.type` (e.g. "landscape-primary", "portrait-primary").
    #[serde(default)]
    pub screen_orientation: Option<String>,
    /// `navigator.language` — the browser UI's own primary locale (BCP 47, e.g. "ja-JP").
    #[serde(default)]
    pub language: Option<String>,
    /// `navigator.languages` — the full `Accept-Language`-equivalent preference list, joined with
    /// ", " into one string rather than kept as a `Vec` (nothing ever queries this by individual
    /// entry; it's display-only, same posture as `ActivityCausedBy::description`).
    #[serde(default)]
    pub languages: Option<String>,
    /// `Intl.DateTimeFormat().resolvedOptions().timeZone` (IANA name, e.g. "Asia/Tokyo").
    #[serde(default)]
    pub timezone: Option<String>,
    /// The timezone's current UTC offset in minutes (`-(new Date()).getTimezoneOffset()`) —
    /// alongside the IANA name above since the same offset maps to several zones and the name
    /// alone doesn't say DST is currently in effect.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub timezone_offset_minutes: Option<i32>,
    /// `navigator.platform` — deprecated by spec but still populated by every real browser today;
    /// kept since it's free signal ("Win32", "MacIntel", "Linux x86_64", "iPhone", ...).
    #[serde(default)]
    pub platform: Option<String>,
    /// `navigator.userAgentData.platform` (User-Agent Client Hints, Chromium-only) — a more
    /// current replacement for `platform` above on browsers that support it ("Windows", "macOS",
    /// "Linux", "Android", "Chrome OS").
    #[serde(default)]
    pub uach_platform: Option<String>,
    /// `navigator.userAgentData.platformVersion` (Client Hints, Chromium-only, requires
    /// `getHighEntropyValues`) — the actual OS version number, which the bare `User-Agent` string
    /// increasingly omits/freezes for privacy (e.g. Chrome no longer reveals the real Windows
    /// build in its UA string at all).
    #[serde(default)]
    pub uach_platform_version: Option<String>,
    /// `navigator.userAgentData.brands` (Client Hints) — each `{brand, version}` pair the browser
    /// self-reports, joined as "Brand version, Brand version, ...". More trustworthy than parsing
    /// the `User-Agent` string alone for the browser's *major* version, though still just the
    /// major version, not full (see `uach_full_version_list` for that).
    #[serde(default)]
    pub uach_brands: Option<String>,
    /// `navigator.userAgentData.getHighEntropyValues(["fullVersionList"])` (Client Hints) — same
    /// shape as `uach_brands` but with each brand's *full* version string (e.g. "120.0.6099.129"),
    /// the most precise browser-version signal available on a Chromium browser; unavailable
    /// entirely on Firefox/Safari (no Client Hints support), where `UserAgentInfo::browser_version`
    /// (parsed from the plain `User-Agent` string) is the only version signal at all.
    #[serde(default)]
    pub uach_full_version_list: Option<String>,
    /// `navigator.userAgentData.mobile` (Client Hints) — `true`/`false` self-report, distinct from
    /// `woothee`'s own UA-string-based `category` guess.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub uach_mobile: Option<bool>,
    /// `navigator.hardwareConcurrency` — logical CPU core count.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub hardware_concurrency: Option<u32>,
    /// `navigator.deviceMemory` (GiB, Chromium-only — `None` on Firefox/Safari, not a real 0).
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub device_memory_gib: Option<f32>,
    /// `'ontouchstart' in window || navigator.maxTouchPoints > 0`.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub touch_support: Option<bool>,
    /// `navigator.maxTouchPoints` — how many simultaneous touch points the device reports, beyond
    /// the plain yes/no `touch_support` above.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub max_touch_points: Option<u32>,
    /// `navigator.connection.effectiveType` (Chromium-only Network Information API — "4g" / "3g" /
    /// "2g" / "slow-2g").
    #[serde(default)]
    pub connection_type: Option<String>,
    /// `navigator.connection.downlink` (Chromium-only Network Information API) — estimated
    /// effective bandwidth in Mbps.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub connection_downlink_mbps: Option<f32>,
    /// `navigator.connection.rtt` (Chromium-only Network Information API) — estimated round-trip
    /// latency in milliseconds.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub connection_rtt_ms: Option<u32>,
    /// `navigator.connection.saveData` (Chromium-only Network Information API) — whether the user
    /// has the browser's own data-saver mode on.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub connection_save_data: Option<bool>,
    /// `navigator.cookieEnabled`.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub cookie_enabled: Option<bool>,
    /// `navigator.pdfViewerEnabled` (Chromium-only).
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub pdf_viewer_enabled: Option<bool>,
    /// `matchMedia("(prefers-color-scheme: dark)").matches` — the OS/browser-level dark-mode
    /// preference, independent of whatever theme this app itself renders in.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub prefers_dark_color_scheme: Option<bool>,
    /// `matchMedia("(prefers-reduced-motion: reduce)").matches`.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub prefers_reduced_motion: Option<bool>,
    /// Heuristic-only private/incognito-browsing guess — no browser exposes a real "am I in
    /// private mode" flag (deliberately, as part of that mode's own privacy design), so this is
    /// inferred from `navigator.storage.estimate()` reporting an abnormally small quota (Chrome's
    /// incognito mode caps it far below a normal session's) — see `collectClientReportedInfo`'s
    /// own docs for exactly which signal and threshold. `None` (not `false`) when the API itself
    /// is unavailable (Firefox/Safari don't implement this the same way, so absence here is not
    /// evidence of a normal session) — only ever `Some(true)`/`Some(false)` on Chromium browsers
    /// that support `navigator.storage.estimate()`. Never treated as a real security signal
    /// anywhere in this codebase — display/diagnostic only, same posture as `client_ip`.
    #[serde(default)]
    #[serde(deserialize_with = "from_str_opt")]
    pub probably_incognito: Option<bool>,
}

impl ClientReportedInfo {
    /// `true` if every field is `None` — a client-reported form that carried literally nothing
    /// (an old cached frontend build, or a hand-crafted request) should store as
    /// `client_reported: None` on the enclosing `DeviceInfo`, not `Some(Self::default())`.
    pub fn is_empty(&self) -> bool {
        let Self {
            screen_width,
            screen_height,
            screen_avail_width,
            screen_avail_height,
            window_outer_width,
            window_outer_height,
            window_inner_width,
            window_inner_height,
            device_pixel_ratio,
            color_depth,
            pixel_depth,
            screen_orientation,
            language,
            languages,
            timezone,
            timezone_offset_minutes,
            platform,
            uach_platform,
            uach_platform_version,
            uach_brands,
            uach_full_version_list,
            uach_mobile,
            hardware_concurrency,
            device_memory_gib,
            touch_support,
            max_touch_points,
            connection_type,
            connection_downlink_mbps,
            connection_rtt_ms,
            connection_save_data,
            cookie_enabled,
            pdf_viewer_enabled,
            prefers_dark_color_scheme,
            prefers_reduced_motion,
            probably_incognito,
        } = self;
        screen_width.is_none()
            && screen_height.is_none()
            && screen_avail_width.is_none()
            && screen_avail_height.is_none()
            && window_outer_width.is_none()
            && window_outer_height.is_none()
            && window_inner_width.is_none()
            && window_inner_height.is_none()
            && device_pixel_ratio.is_none()
            && color_depth.is_none()
            && pixel_depth.is_none()
            && screen_orientation.is_none()
            && language.is_none()
            && languages.is_none()
            && timezone.is_none()
            && timezone_offset_minutes.is_none()
            && platform.is_none()
            && uach_platform.is_none()
            && uach_platform_version.is_none()
            && uach_brands.is_none()
            && uach_full_version_list.is_none()
            && uach_mobile.is_none()
            && hardware_concurrency.is_none()
            && device_memory_gib.is_none()
            && touch_support.is_none()
            && max_touch_points.is_none()
            && connection_type.is_none()
            && connection_downlink_mbps.is_none()
            && connection_rtt_ms.is_none()
            && connection_save_data.is_none()
            && cookie_enabled.is_none()
            && pdf_viewer_enabled.is_none()
            && prefers_dark_color_scheme.is_none()
            && prefers_reduced_motion.is_none()
            && probably_incognito.is_none()
    }
}

/// Everything this request's own environment revealed — the union [`ActivityEntry::device_info`]
/// and [`crate::refresh_tokens::RefreshTokenRecord::device_info`] actually store. Each of the
/// three parts is independently optional: `user_agent` is `None` only if the header itself was
/// absent/unparseable; `geo` is `None` if the geo database isn't installed or the IP has no
/// coverage; `client_reported` is `None` for any request shape that doesn't carry it (currently:
/// every `guest_visitor` request, and any login/refresh call from a pre-upgrade frontend build).
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DeviceInfo {
    #[serde(default)]
    pub user_agent: Option<UserAgentInfo>,
    #[serde(default)]
    pub geo: Option<GeoInfo>,
    #[serde(default)]
    pub client_reported: Option<ClientReportedInfo>,
}

impl DeviceInfo {
    /// `true` if every part is empty — lets a call site skip storing/serializing a `DeviceInfo`
    /// that would carry zero information (e.g. UA absent, no geo database installed, no client
    /// report submitted), same as omitting the field entirely.
    pub fn is_empty(&self) -> bool {
        self.user_agent.is_none() && self.geo.is_none() && self.client_reported.is_none()
    }

    /// Human-readable default label for the login-device list, e.g.
    /// `"Linux x86_64 Chrome 152 + zh-CN"` or `"Windows 10 Firefox 149 + ja"`.
    ///
    /// Deliberately best-effort and lossy: it is only the *initial* name. The active-session API
    /// lets an administrator rename any family, and a stored custom name always wins over this.
    /// The format groups the stable identity (OS/platform, browser) separately from the language
    /// so two different login sessions from the same machine but different browser-language
    /// settings are still distinguishable.
    pub fn display_name(&self) -> String {
        let Some(ua) = self.user_agent.as_ref() else {
            return "Unknown device".to_string();
        };
        let client = self.client_reported.as_ref();

        let mut os = if ua.os.is_empty() || ua.os == "UNKNOWN" {
            client
                .and_then(|c| c.platform.clone().or_else(|| c.uach_platform.clone()))
                .unwrap_or_else(|| "Unknown OS".to_string())
        } else {
            ua.os.clone()
        };
        // The UA string commonly reports Linux with an UNKNOWN version while the client-reported
        // platform carries the useful architecture, e.g. "Linux x86_64". Windows' UA version is
        // "NT 10.0", which just duplicates "Windows 10" if appended verbatim, so omit those
        // noisy-but-redundant forms rather than rendering "Windows 10 NT 10.0".
        if os.eq_ignore_ascii_case("Linux") {
            if let Some(platform) =
                client.and_then(|c| c.platform.clone().or_else(|| c.uach_platform.clone()))
            {
                os = platform;
            }
        }
        let os_version = ua.os_version.trim();
        let os = if os_version.is_empty()
            || os_version == "UNKNOWN"
            || os_version.starts_with("NT ")
            || os.contains(os_version)
        {
            os
        } else {
            format!("{os} {os_version}")
        };

        let browser_version = ua.browser_version.trim();
        let browser = if ua.browser.is_empty() || ua.browser == "UNKNOWN" {
            "Unknown browser".to_string()
        } else if browser_version.is_empty() || browser_version == "UNKNOWN" {
            ua.browser.clone()
        } else {
            format!(
                "{} {}",
                ua.browser,
                browser_version.split('.').next().unwrap_or(browser_version)
            )
        };

        let language = client
            .and_then(|c| {
                c.language
                    .clone()
                    .filter(|value| !value.trim().is_empty())
                    .or_else(|| {
                        c.languages
                            .as_deref()
                            .and_then(|languages| languages.split(',').next())
                            .map(str::trim)
                            .filter(|value| !value.is_empty())
                            .map(str::to_string)
                    })
            })
            .unwrap_or_else(|| "unknown language".to_string());

        format!("{os} {browser} + {language}")
    }
}

#[cfg(test)]
mod display_name_tests {
    use super::*;

    fn ua(os: &str, os_version: &str, browser: &str, browser_version: &str) -> UserAgentInfo {
        UserAgentInfo {
            category: "pc".to_string(),
            os: os.to_string(),
            os_version: os_version.to_string(),
            browser: browser.to_string(),
            browser_version: browser_version.to_string(),
            browser_type: "browser".to_string(),
            vendor: "Google".to_string(),
        }
    }

    #[test]
    fn linux_name_uses_platform_and_language() {
        let device = DeviceInfo {
            user_agent: Some(ua("Linux", "UNKNOWN", "Chrome", "152.0.7977.82")),
            geo: None,
            client_reported: Some(ClientReportedInfo {
                platform: Some("Linux x86_64".to_string()),
                language: Some("zh-CN".to_string()),
                ..Default::default()
            }),
        };
        assert_eq!(device.display_name(), "Linux x86_64 Chrome 152 + zh-CN");
    }

    #[test]
    fn windows_name_omits_redundant_nt_version() {
        let device = DeviceInfo {
            user_agent: Some(ua("Windows 10", "NT 10.0", "Firefox", "149.0")),
            geo: None,
            client_reported: Some(ClientReportedInfo {
                language: Some("ja".to_string()),
                ..Default::default()
            }),
        };
        assert_eq!(device.display_name(), "Windows 10 Firefox 149 + ja");
    }

    #[test]
    fn missing_user_agent_falls_back_to_unknown_device() {
        assert_eq!(DeviceInfo::default().display_name(), "Unknown device");
    }
}
