//! IP → country/city lookup via a local MaxMind GeoLite2-City database — no third-party API call,
//! so a request's IP never leaves this server (unlike an online lookup service, e.g. ip-api.com,
//! which would hand every visitor's real IP to a third party just to render a country name).
//!
//! The `.mmdb` file itself is not bundled (MaxMind's license forbids redistributing it, and it
//! requires a free MaxMind account to obtain) — an operator who wants geo data sets
//! `LANRURUGI_GEOIP_DB_PATH` to point at one, kept fresh by MaxMind's own `geoipupdate` tool (see
//! `docker-entrypoint.dev.sh` / the production `Dockerfile` for how this project wires that up).
//! Absent entirely, [`lookup`] always returns `None` — geo data is additive, never required for
//! the rest of the activity-log feature to work.

use std::net::IpAddr;

use lanrurugi_storage::device_info::GeoInfo;
use maxminddb::geoip2;

const DEFAULT_DB_PATH: &str = "/var/lib/GeoIP/GeoLite2-City.mmdb";

/// `OnceLock<Option<Reader>>` rather than `OnceLock<Reader>` — the database file may genuinely
/// not exist (no `geoipupdate` run yet, or an operator who never opted in at all), which must
/// degrade to "no geo data", not a panic or a repeated failed-open attempt on every request.
/// Opened once, lazily, on first use — not at process startup — so a server with no geo database
/// installed pays zero cost for this feature ever existing.
fn reader() -> &'static Option<maxminddb::Reader<Vec<u8>>> {
    static READER: std::sync::OnceLock<Option<maxminddb::Reader<Vec<u8>>>> =
        std::sync::OnceLock::new();
    READER.get_or_init(|| {
        let path = std::env::var("LANRURUGI_GEOIP_DB_PATH")
            .unwrap_or_else(|_| DEFAULT_DB_PATH.to_string());
        match maxminddb::Reader::open_readfile(&path) {
            Ok(reader) => {
                tracing::info!(path = %path, "geoip: GeoLite2 database loaded");
                Some(reader)
            }
            Err(e) => {
                tracing::info!(
                    path = %path,
                    error = %e,
                    "geoip: no database installed — activity entries will carry no geo data"
                );
                None
            }
        }
    })
}

/// `None` if no database is installed, `ip` doesn't parse (a `client_ip` value is
/// display-diagnostic-only free text — see `procedure::client_ip`'s own docs — so a malformed one
/// is a normal, silent no-op here, not an error), or the database has no coverage for this
/// specific address (private/reserved ranges, or a real gap in MaxMind's own data) — in every
/// case, "no geo info", never a hard failure that could take down the activity write behind it.
pub fn lookup(ip: &str) -> Option<GeoInfo> {
    let reader = reader().as_ref()?;
    let addr: IpAddr = ip.parse().ok()?;
    let result = reader.lookup(addr).ok()?;
    let city = result.decode::<geoip2::City>().ok()??;

    let country_code = city.country.iso_code.map(str::to_string);
    let country_name = city.country.names.english.map(str::to_string);
    let city_name = city.city.names.english.map(str::to_string);
    // The most specific subdivision GeoLite2 reports is last in the list (see `geoip2::City::
    // subdivisions`'s own docs: "ordered from largest to smallest") — e.g. for a Tokyo IP this is
    // "Tokyo" itself, not some larger enclosing region.
    let subdivision_name = city
        .subdivisions
        .last()
        .and_then(|s| s.names.english)
        .map(str::to_string);

    if country_code.is_none()
        && country_name.is_none()
        && city_name.is_none()
        && subdivision_name.is_none()
    {
        return None;
    }

    Some(GeoInfo {
        country_code,
        country_name,
        city_name,
        subdivision_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// No `LANRURUGI_GEOIP_DB_PATH` set in the test environment (and no database installed at the
    /// default path either) — confirms the "database not installed" path degrades to `None`
    /// rather than panicking, without needing a real `.mmdb` fixture file in this repo.
    #[test]
    fn returns_none_without_a_database_installed() {
        if std::env::var("LANRURUGI_GEOIP_DB_PATH").is_ok() {
            eprintln!("skipping: LANRURUGI_GEOIP_DB_PATH is set in this environment");
            return;
        }
        assert!(lookup("8.8.8.8").is_none());
    }

    #[test]
    fn returns_none_for_an_unparseable_ip() {
        assert!(lookup("not-an-ip").is_none());
    }
}
