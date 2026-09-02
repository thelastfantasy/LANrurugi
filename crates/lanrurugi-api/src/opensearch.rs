//! `GET /opensearch.xml` — browser-autodiscoverable OpenSearch description (issue #90). Dynamic,
//! not a static file: the search URL template reflects whichever `Host`/`X-Forwarded-Host` the
//! request actually arrived on (so an instance reachable at both a real domain and a bare
//! `ip:port` gets the right one installed either way), and the description text embeds the
//! resolved client IP (`procedure::client_ip`'s own `X-Forwarded-For`-first-hop-or-peer-addr
//! semantics — display-only, same as everywhere else that field is used, never a security
//! boundary) purely so two different visitors' installed searches are visually distinguishable.
//!
//! Deliberately **not** merged into [`crate::router`] (the `/api/*`, `require_api_key`-gated
//! group) — a browser must be able to fetch this during its own autodiscovery/install step,
//! before any login has happened, the same reasoning `login::router()` stays unprotected for.
//! Wired into `lanrurugi-server/src/app.rs` at the bare `/opensearch.xml` path (not under
//! `/api`), matching where `index.html`'s own `<link rel="search">` points.

use axum::extract::{ConnectInfo, State};
use axum::http::{header, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;

use crate::procedure::client_ip;
use crate::state::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/opensearch.xml", get(opensearch_xml))
}

/// Escapes the five XML predefined entities — every dynamic value below (`Host`, forwarded proto,
/// client IP) is attacker-influenceable (a request header, verbatim) and gets threaded into a raw
/// XML string, so every single one must go through this before insertion, no exceptions. Not
/// `<img>`/attribute-context HTML escaping (a wider set of characters); this only ever needs to
/// produce valid XML text/attribute content, and XML defines exactly these five.
fn xml_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            _ => out.push(c),
        }
    }
    out
}

/// Resolves the scheme+host the browser should query — `X-Forwarded-Proto`/`X-Forwarded-Host`
/// first (this app's own documented reverse-proxy deployment shape, same convention
/// `client_ip`'s own `X-Forwarded-For` precedence follows), falling back to the plain `Host`
/// header and a scheme guessed from whether this looks like a loopback/private dev address
/// (never `X-Forwarded-Proto`-derived, since there's nothing to fall back to it *from* once it's
/// already absent). A missing `Host` entirely (malformed request) falls back to `localhost` — the
/// resulting template just wouldn't resolve anywhere useful, but the XML itself stays well-formed
/// rather than panicking or 500ing over a header no real browser omits.
fn resolve_base_url(headers: &HeaderMap) -> String {
    let host = headers
        .get("X-Forwarded-Host")
        .and_then(|v| v.to_str().ok())
        .or_else(|| headers.get(header::HOST).and_then(|v| v.to_str().ok()))
        .unwrap_or("localhost")
        .trim();
    let host = if host.is_empty() { "localhost" } else { host };

    let scheme = headers
        .get("X-Forwarded-Proto")
        .and_then(|v| v.to_str().ok())
        .filter(|s| *s == "http" || *s == "https")
        .unwrap_or("http");

    format!("{scheme}://{host}")
}

async fn opensearch_xml(
    State(_state): State<AppState>,
    ConnectInfo(peer_addr): ConnectInfo<std::net::SocketAddr>,
    headers: HeaderMap,
) -> Response {
    let base_url = resolve_base_url(&headers);
    let ip = client_ip(&headers, peer_addr).unwrap_or_else(|| "unknown".to_string());

    let base_url_escaped = xml_escape(&base_url);
    let ip_escaped = xml_escape(&ip);
    let search_url = format!("{base_url_escaped}/?q={{searchTerms}}");

    // `moz:SearchForm` (Firefox's own extension to the spec, `xmlns:moz` namespace) points back
    // at the bare site root — the closest equivalent to "a human-usable search form", since the
    // Library page's own search bar lives there, not at a dedicated `/search` page.
    let xml = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<OpenSearchDescription xmlns="http://a9.com/-/spec/opensearch/1.1/" xmlns:moz="http://www.mozilla.org/2006/browser/search/">
  <ShortName>LANrurugi</ShortName>
  <Description>Search LANrurugi ({base_url_escaped}) — requested from {ip_escaped}</Description>
  <InputEncoding>UTF-8</InputEncoding>
  <OutputEncoding>UTF-8</OutputEncoding>
  <Url type="text/html" method="get" template="{search_url}"/>
  <moz:SearchForm>{base_url_escaped}/</moz:SearchForm>
</OpenSearchDescription>
"#
    );

    (
        StatusCode::OK,
        [
            (
                header::CONTENT_TYPE,
                "application/opensearchdescription+xml; charset=utf-8",
            ),
            (header::CACHE_CONTROL, "no-store"),
        ],
        xml,
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn xml_escape_covers_all_five_predefined_entities() {
        assert_eq!(
            xml_escape(r#"<a href="x">'&'</a>"#),
            "&lt;a href=&quot;x&quot;&gt;&apos;&amp;&apos;&lt;/a&gt;"
        );
    }

    #[test]
    fn resolve_base_url_prefers_forwarded_headers() {
        let mut h = HeaderMap::new();
        h.insert("X-Forwarded-Host", "lanrurugi.example.com".parse().unwrap());
        h.insert("X-Forwarded-Proto", "https".parse().unwrap());
        h.insert(header::HOST, "127.0.0.1:3000".parse().unwrap());
        assert_eq!(resolve_base_url(&h), "https://lanrurugi.example.com");
    }

    #[test]
    fn resolve_base_url_falls_back_to_plain_host_and_http() {
        let mut h = HeaderMap::new();
        h.insert(header::HOST, "192.168.1.10:3000".parse().unwrap());
        assert_eq!(resolve_base_url(&h), "http://192.168.1.10:3000");
    }

    #[test]
    fn resolve_base_url_falls_back_to_localhost_when_host_missing() {
        let h = HeaderMap::new();
        assert_eq!(resolve_base_url(&h), "http://localhost");
    }

    #[test]
    fn resolve_base_url_ignores_an_unrecognized_forwarded_proto_value() {
        let mut h = HeaderMap::new();
        h.insert("X-Forwarded-Host", "lanrurugi.example.com".parse().unwrap());
        h.insert("X-Forwarded-Proto", "gopher".parse().unwrap());
        assert_eq!(resolve_base_url(&h), "http://lanrurugi.example.com");
    }
}
