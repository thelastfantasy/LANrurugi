//! Guards for the second host name a deployment may serve: the one given in the general
//! `direct_download_url` setting, where browsers and download managers fetch archives so that
//! Cloudflare's proxy is not in the path.
//!
//! Two separate things live here. `restrict_to_downloads` is the second lock behind the reverse
//! proxy's own configuration: on that host name only the download routes answer, everything else
//! is a 404, so a mistake in nginx (or a future `location /api/`) cannot expose the admin API on
//! the address that deliberately bypasses the CDN. `share_media_cors` makes the recipient page's
//! cross-origin reads work when pages and media sit on different hosts: those URLs are authorised
//! by their own token in the query, never by a cookie, so `*` is safe and no credentials are
//! allowed.

use axum::extract::{Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::AppState;

/// What a download-only host name may serve.
const DOWNLOAD_PATHS: [&str; 2] = ["/api/share/r/", "/api/dl/"];

/// The response headers a cross-origin reader needs to see: the size and the "this was resized"
/// hints the share reader reads off a HEAD before it decides how to display a page.
const EXPOSED: &str = "content-length, content-range, accept-ranges, \
                       x-lrr-resized, x-lrr-original-size, x-lrr-original-dimensions, x-lrr-served-size";

pub async fn restrict_to_downloads(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(base) = crate::settings::direct_download_url(&state).await else {
        return next.run(request).await;
    };
    let Ok(url) = url::Url::parse(&base) else {
        return next.run(request).await;
    };
    let Some(host) = url.host_str() else {
        return next.run(request).await;
    };
    // The host name can arrive three ways, and a request that spells it none of them is not one
    // this middleware can classify. `uri().authority()` covers HTTP/2, where `:authority` replaces
    // the `Host` header; `X-Forwarded-Host` covers a proxy that rewrites `Host` to its upstream
    // (nginx's `$proxy_host`). Missing all three is not treated as "this is the download host":
    // failing closed here would 404 every request on a deployment whose proxy drops the header.
    let requested = request
        .headers()
        .get(header::HOST)
        .and_then(|value| value.to_str().ok())
        .map(str::to_string)
        .or_else(|| {
            request
                .uri()
                .authority()
                .map(|authority| authority.to_string())
        })
        .or_else(|| {
            request
                .headers()
                .get("x-forwarded-host")
                .and_then(|value| value.to_str().ok())
                .map(str::to_string)
        })
        .unwrap_or_default();
    if requested.is_empty() || !host_matches(&requested, host, url.port()) {
        return next.run(request).await;
    }
    let path = request.uri().path();
    if request.method() == Method::OPTIONS || is_download_path(path) {
        return next.run(request).await;
    }
    crate::common::not_found(
        "download_host_only",
        "This host name only serves downloads.",
    )
}

/// Whether a raw request path belongs to a route the download host may serve. Matching on the raw
/// (still percent-encoded) path is deliberate: a request the router would decode into an admin
/// route does not carry the prefix here, so it is refused rather than decoded into access.
fn is_download_path(path: &str) -> bool {
    DOWNLOAD_PATHS.iter().any(|prefix| path.starts_with(prefix))
}

/// `Host` may or may not carry the port; the configured host may or may not be the default for
/// its scheme. Accept both spellings rather than rejecting a correct request over punctuation.
fn host_matches(requested: &str, host: &str, port: Option<u16>) -> bool {
    let (name, given_port) = match requested.rsplit_once(':') {
        Some((name, port)) if port.chars().all(|c| c.is_ascii_digit()) => {
            (name, port.parse::<u16>().ok())
        }
        _ => (requested, None),
    };
    // A trailing dot is the same name to DNS (`dl.example.org.`), and a request spelling it that way
    // must still be held to the download-only policy — treating it as a different host would let the
    // admin API through on the very address that deliberately bypasses the CDN.
    name.trim_end_matches('.')
        .eq_ignore_ascii_case(host.trim_end_matches('.'))
        && given_port.unwrap_or(443) == port.unwrap_or(443)
}

pub async fn share_media_cors(
    State(_state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if !request.uri().path().starts_with("/api/share/r/") {
        return next.run(request).await;
    }
    let preflight = request.method() == Method::OPTIONS;
    let mut response = if preflight {
        StatusCode::NO_CONTENT.into_response()
    } else {
        next.run(request).await
    };
    let headers = response.headers_mut();
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static("*"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static("GET, HEAD, OPTIONS"),
    );
    headers.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static("range, x-lrr-image-support"),
    );
    headers.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static(EXPOSED),
    );
    headers.insert(header::VARY, HeaderValue::from_static("origin"));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_two_download_prefixes_pass() {
        for allowed in [
            "/api/dl/",
            "/api/dl/archives/abc",
            "/api/share/r/token",
            "/api/share/r/token/page/1",
        ] {
            assert!(is_download_path(allowed), "{allowed} should be served");
        }
        for refused in [
            "/",
            "/api/archives/abc",
            "/api/settings",
            "/api/dl",
            "/api/dlx/archives",
            "/api/share/",
            "//api/dl/archives/abc",
            "/api/%64l/archives/abc",
        ] {
            assert!(!is_download_path(refused), "{refused} must be refused");
        }
    }

    #[test]
    fn the_configured_host_matches_with_or_without_its_port() {
        assert!(host_matches("dl.example.org", "dl.example.org", None));
        assert!(host_matches("DL.Example.ORG", "dl.example.org", None));
        assert!(host_matches("dl.example.org:443", "dl.example.org", None));
        assert!(host_matches(
            "dl.example.org:8443",
            "dl.example.org",
            Some(8443)
        ));
        assert!(!host_matches(
            "dl.example.org",
            "dl.example.org",
            Some(8443)
        ));
    }

    #[test]
    fn a_lookalike_host_name_is_not_the_download_host() {
        assert!(!host_matches(
            "dl.example.org.evil.test",
            "dl.example.org",
            None
        ));
        assert!(!host_matches("evil.test", "dl.example.org", None));
        assert!(host_matches("dl.example.org.", "dl.example.org", None));
    }
}
