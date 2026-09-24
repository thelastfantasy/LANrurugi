//! Resolves the real filename for a downloaded resource — `Content-Disposition` header parsing
//! (RFC 6266 `filename=`/`filename*=`), percent-decoding, and path-traversal sanitization. Split
//! out of `stream.rs` (issue #92's own chaika.moe investigation) once it became clear the same
//! "server hands back a percent-encoded/RFC-6266 filename, decode it the way legacy actually does"
//! shape was going to be needed again outside the download-streaming path specifically — keeping
//! it here as a self-contained, directly-testable unit rather than three private functions buried
//! in `stream.rs`'s own much larger module.

use std::path::Path;

/// Determines the real filename for a downloaded resource, in priority order (contracts/
/// plugin-download-protocol.md's `filename_hint` field docs):
/// 1. The response's own `Content-Disposition` header (`filename=`/`filename*=`), when present
///    and parseable — matches legacy `Model::Upload.pm::download_url`'s own real-file-name
///    behavior (verified against that Perl source, including its `filename=` > `filename*=`
///    priority order — legacy checks `filename=` first via `elsif`, not the other way around,
///    despite RFC 6266 technically recommending `filename*=` take precedence when both are
///    present; matching legacy's own actual behavior here rather than the stricter RFC reading).
/// 2. The plugin-supplied `filename_hint`, when the header is absent or unparseable.
/// 3. A name derived from the URL's own path, as a last resort — percent-decoded, matching
///    legacy's own `uri_unescape($1)` on this exact fallback (`download_url`'s own comment: "Also
///    URL/utf8 decode just in case"). A path segment straight from `url::Url::path_segments()` is
///    still percent-encoded (that's the whole point of a URL-safe representation), so skipping
///    this decode would reproduce the same "literal `%5B...%5D` ends up as the on-disk filename"
///    bug this module's `content_disposition_filename` fix addresses, just for a different one of
///    the three sources this function draws from.
pub(crate) fn resolve_filename(
    response: &reqwest::Response,
    filename_hint: Option<&str>,
    url: &url::Url,
) -> String {
    if let Some(name) = content_disposition_filename(response) {
        return sanitize_filename(&name);
    }
    if let Some(hint) = filename_hint {
        return sanitize_filename(hint);
    }
    let from_path = url
        .path_segments()
        .and_then(|mut segments| segments.next_back())
        .filter(|s| !s.is_empty())
        .map(|s| {
            percent_encoding::percent_decode_str(s)
                .decode_utf8_lossy()
                .into_owned()
        });
    sanitize_filename(from_path.as_deref().unwrap_or("download"))
}

/// Parses a `Content-Disposition` header's `filename=`/`filename*=` parameter.
///
/// Reads the header's **raw bytes** (`.as_bytes()`), not `HeaderValue::to_str()` — a real,
/// confirmed-live bug this fixes: `to_str()` only succeeds for visible-ASCII byte sequences and
/// returns `Err` (silently swallowed by the old code's `.ok()?`) for anything else, so a server
/// that puts a real UTF-8-encoded non-ASCII filename directly into a plain `filename="..."`
/// parameter — not RFC-compliant (that should be `filename*=UTF-8''...`, percent-encoded), but
/// confirmed against a real download source that sends raw UTF-8 bytes in a plain `filename=`
/// parameter — made this function give up entirely and fall through to the plugin's own
/// `filename_hint`/URL-derived fallback, discarding a perfectly real filename the server did
/// provide.
///
/// `filename*=UTF-8''...` (RFC 6266's own percent-encoded form) IS decoded — a second real,
/// confirmed-live bug: a chaika.moe download whose response used this exact form previously got
/// catalogued with the literal percent-encoded text as its `title`/`name`/on-disk filename
/// (`%5BGreat%20Canyon...` instead of `[Great Canyon...`), and — separately — the archive's `file`
/// field was left empty by a since-fixed failure in the fixup step that follows ingestion,
/// producing a permanently-broken reader (`archives::fetch_page`'s `extension()` call finds no `.`
/// in an empty path and 500s on every request). Undecoded percent-encoding was never actually
/// "good enough" the way an earlier version of this comment claimed.
fn content_disposition_filename(response: &reqwest::Response) -> Option<String> {
    let bytes = response
        .headers()
        .get(reqwest::header::CONTENT_DISPOSITION)?
        .as_bytes();
    // UTF-8 first (the real, confirmed-live case above) — Latin-1 as a last-resort fallback for a
    // server that genuinely sends single-byte-per-character bytes (Latin-1 maps every byte to a
    // Unicode code point of the identical value, so this conversion can never itself fail).
    let header = String::from_utf8(bytes.to_vec())
        .unwrap_or_else(|_| bytes.iter().map(|&b| b as char).collect());
    // Two passes, not one `.split(';')` loop that returns on whichever it meets first — legacy's
    // own `if (filename=...) elsif (filename*=...)` always prefers a plain `filename=` when both
    // are present in the same header, regardless of which is written first (verified against that
    // Perl source). Matching that fixed priority, not RFC 6266's own (stricter) recommendation
    // that `filename*=` should win when both appear, since this is reproducing legacy's actual
    // observed behavior, not implementing the RFC from scratch.
    let parts: Vec<&str> = header.split(';').map(str::trim).collect();
    for part in &parts {
        if let Some(name) = part.strip_prefix("filename=") {
            return Some(name.trim_matches('"').to_string());
        }
    }
    for part in &parts {
        if let Some(name) = part.strip_prefix("filename*=") {
            // `UTF-8''actual-name` — strip the charset/lang prefix, then percent-decode the rest
            // per RFC 6266 §4.2/RFC 5987. `percent_decode_str` returns raw bytes (the encoding
            // could in principle be non-UTF-8 despite the `UTF-8''` tag lying), so this still goes
            // through a lossy UTF-8 conversion rather than assuming success — a malformed/non-UTF-8
            // percent-encoded name degrades to `�` replacement characters instead of failing this
            // whole function and falling back to `filename_hint`/the URL, matching how the plain
            // `filename=` branch above already never rejects on invalid content either.
            let name = name.rsplit("''").next().unwrap_or(name);
            let name = name.trim_matches('"');
            return Some(
                percent_encoding::percent_decode_str(name)
                    .decode_utf8_lossy()
                    .into_owned(),
            );
        }
    }
    None
}

/// Same reasoning as `upload.rs::sanitize_filename` — never used as a path, so a
/// server/plugin-supplied name can't traverse outside the staging/archive directory.
fn sanitize_filename(name: &str) -> String {
    Path::new(name)
        .file_name()
        .and_then(|n| n.to_str())
        .unwrap_or("download")
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_filename_strips_directory_traversal() {
        assert_eq!(
            sanitize_filename("../../etc/passwd/archive.zip"),
            "archive.zip"
        );
    }

    #[test]
    fn sanitize_filename_falls_back_when_empty() {
        assert_eq!(sanitize_filename(""), "download");
    }

    /// Spins up a real local HTTP server (`axum`, already a workspace dependency — no new mock-
    /// server crate needed) so these tests exercise `resolve_filename` against an actual
    /// `reqwest::Response` end to end — `reqwest::Response` has no public constructor from a bare
    /// `http::Response`, so a real round trip is the only way to build one for a test.
    async fn spawn_test_server(router: axum::Router) -> (String, tokio::task::JoinHandle<()>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        (format!("http://{addr}"), handle)
    }

    #[tokio::test]
    async fn resolve_filename_prefers_content_disposition_over_hint() {
        let router = axum::Router::new().route(
            "/archive.zip",
            axum::routing::get(|| async {
                [(
                    reqwest::header::CONTENT_DISPOSITION.as_str(),
                    "attachment; filename=\"real-name.zip\"",
                )]
            }),
        );
        let (base_url, server) = spawn_test_server(router).await;

        let url = format!("{base_url}/archive.zip");
        let response = reqwest::get(&url).await.unwrap();
        let parsed_url = url::Url::parse(&url).unwrap();
        let filename = resolve_filename(&response, Some("hint.zip"), &parsed_url);

        assert_eq!(
            filename, "real-name.zip",
            "Content-Disposition must win over filename_hint"
        );
        server.abort();
    }

    /// Regression test for the `filename*=UTF-8''<percent-encoded>` bug (RFC 6266's own
    /// percent-encoded form, distinct from the plain-UTF-8-bytes-in-`filename=` case below) —
    /// confirmed live via a real chaika.moe download that got catalogued with the literal
    /// `%5BGreat%20Canyon...` text as its title/filename/on-disk name because this branch used to
    /// strip only the `UTF-8''` prefix and leave the rest percent-encoded as-is.
    #[tokio::test]
    async fn resolve_filename_percent_decodes_an_rfc6266_content_disposition_filename() {
        let router = axum::Router::new().route(
            "/archive.zip",
            axum::routing::get(|| async {
                [(
                    reqwest::header::CONTENT_DISPOSITION.as_str(),
                    "attachment; filename*=UTF-8''%5BGreat%20Canyon%5D%20real.zip",
                )]
            }),
        );
        let (base_url, server) = spawn_test_server(router).await;

        let url = format!("{base_url}/archive.zip");
        let response = reqwest::get(&url).await.unwrap();
        let parsed_url = url::Url::parse(&url).unwrap();
        let filename = resolve_filename(&response, Some("hint.zip"), &parsed_url);

        assert_eq!(
            filename, "[Great Canyon] real.zip",
            "the percent-encoded filename*= form must be decoded, not used verbatim"
        );
        server.abort();
    }

    /// Regression test for a non-ASCII UTF-8 `Content-Disposition` filename (some real-world
    /// servers send raw UTF-8 bytes directly in a plain `filename="..."` parameter — not RFC
    /// 6266-compliant, that requires `filename*=UTF-8''<percent-encoded>` — which is what made
    /// `content_disposition_filename` give up and fall through to the `filename_hint`/URL-derived
    /// fallback before this fix).
    #[tokio::test]
    async fn resolve_filename_decodes_a_non_ascii_utf8_content_disposition_filename() {
        let filename_utf8 = "日本語ファイル名.zip";
        let header_value = format!("attachment; filename=\"{filename_utf8}\"");
        let router = axum::Router::new().route(
            "/archive.zip",
            axum::routing::get(move || {
                let header_value = header_value.clone();
                async move {
                    [(
                        reqwest::header::CONTENT_DISPOSITION.as_str().to_string(),
                        header_value,
                    )]
                }
            }),
        );
        let (base_url, server) = spawn_test_server(router).await;

        let url = format!("{base_url}/archive.zip");
        let response = reqwest::get(&url).await.unwrap();
        let parsed_url = url::Url::parse(&url).unwrap();
        let filename = resolve_filename(&response, Some("fallback-hint-name.zip"), &parsed_url);

        assert_eq!(
            filename, filename_utf8,
            "the real UTF-8 filename must be used, not the filename_hint fallback"
        );
        server.abort();
    }

    #[tokio::test]
    async fn resolve_filename_falls_back_to_hint_when_no_content_disposition() {
        let router =
            axum::Router::new().route("/archive.zip", axum::routing::get(|| async { "body" }));
        let (base_url, server) = spawn_test_server(router).await;

        let url = format!("{base_url}/archive.zip");
        let response = reqwest::get(&url).await.unwrap();
        let parsed_url = url::Url::parse(&url).unwrap();
        let filename = resolve_filename(&response, Some("hint.zip"), &parsed_url);

        assert_eq!(filename, "hint.zip");
        server.abort();
    }

    #[tokio::test]
    async fn resolve_filename_percent_decodes_the_url_path_fallback_when_no_hint_either() {
        let router = axum::Router::new().route(
            "/%5BGreat%20Canyon%5D%20real.zip",
            axum::routing::get(|| async { "body" }),
        );
        let (base_url, server) = spawn_test_server(router).await;

        let url = format!("{base_url}/%5BGreat%20Canyon%5D%20real.zip");
        let response = reqwest::get(&url).await.unwrap();
        let parsed_url = url::Url::parse(&url).unwrap();
        let filename = resolve_filename(&response, None, &parsed_url);

        assert_eq!(
            filename, "[Great Canyon] real.zip",
            "the URL-derived fallback filename must also be percent-decoded"
        );
        server.abort();
    }
}
