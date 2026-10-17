//! Fetches a URL for real, recording the complete redirect trail it actually followed —
//! `generate.rs`'s own `fetch_page` tool and `trial_run.rs`'s per-link fetches both need this
//! (FR-011, `specs/006-ai-plugin-wizard/research.md` §4).
//!
//! A fresh `reqwest::Client` is built per call, not shared/reused — `redirect::Policy::custom`'s
//! closure needs interior-mutable state to accumulate the trail across its own repeated
//! invocations during one request, and that state must be scoped to exactly one call, never
//! shared across concurrent fetches (a shared `Client` would let two in-flight fetches corrupt
//! each other's trail).

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use reqwest::{redirect, StatusCode, Url};

/// Redirect hops a single access is allowed to automatically follow before being judged a
/// failure (FR-011) — matches `reqwest`'s own built-in default cap (`redirect::Policy`'s module
/// docs: "a maximum of 10 redirects... in a chain"), so this doesn't tighten or loosen the
/// existing implicit behavior other outbound calls in this codebase already rely on, just makes
/// the cap explicit and gives it a named, reportable outcome instead of a generic reqwest error.
const REDIRECT_CAP: usize = 10;

/// research.md §6 — bounds one page fetch's own worst-case latency; separate from `tool_chat`'s
/// own (longer) per-call timeout, since a plain page GET and an LLM completion have genuinely
/// different expected durations.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// A real desktop-Chrome UA string — `reqwest`'s own default (`reqwest/<version>`) is trivially
/// fingerprinted and blocked outright by Cloudflare and similar bot-mitigation on many real target
/// sites, which this tool exists specifically to scrape. Not a guarantee of passing every
/// challenge (a JS-execution challenge can't be solved by a plain HTTP client at all regardless of
/// the UA header), but removes the cheapest, most common rejection reason.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";

/// Recognizes Cloudflare's own interstitial "challenge" response by its well-known page markup —
/// a real, previously-undetected gap: this tool had no way to distinguish "the target's real
/// content" from "a Cloudflare challenge page that happens to have a 200/403/503 status", so a
/// plugin generation call reading this as real page structure would silently derive selectors
/// against Cloudflare's own interstitial markup instead of the target site's. Purely a detection
/// heuristic (this HTTP client can't execute JS to actually pass a challenge) — its only purpose
/// is turning a previously invisible failure mode into a loud, logged, AI-visible one instead of a
/// misleading "status: ok" that silently fed garbage into the generation loop.
fn looks_like_cloudflare_challenge(status: StatusCode, body: &str) -> bool {
    let is_challenge_status =
        matches!(status.as_u16(), 403 | 503) || status == StatusCode::FORBIDDEN;
    let has_challenge_markup = body.contains("Just a moment")
        || body.contains("cf-browser-verification")
        || body.contains("Checking your browser")
        || body.contains("cf_chl_opt")
        || body.contains("Cloudflare Ray ID");
    is_challenge_status && has_challenge_markup || (has_challenge_markup && body.len() < 8000)
}

#[derive(Debug)]
pub(super) struct FetchResult {
    /// The original URL, every intermediate hop, and the final landing URL, in order — FR-011
    /// requires the full trail, not just the final destination.
    pub redirect_trail: Vec<Url>,
    pub final_url: Url,
    pub status: StatusCode,
    pub body: String,
    /// See [`looks_like_cloudflare_challenge`] — `true` when this response looks like Cloudflare's
    /// own interstitial rather than the target site's real content.
    pub cloudflare_challenge: bool,
}

#[derive(Debug)]
pub(super) enum FetchError {
    /// More than [`REDIRECT_CAP`] hops occurred before a final (non-redirect) response was
    /// reached — carries the trail accumulated so far, per FR-011's "supply the fact that the
    /// cap was exceeded... as-is" requirement.
    RedirectCapExceeded { trail: Vec<Url> },
    /// Any other failure to complete the request (invalid URL, connection failure, timeout,
    /// non-UTF8 body, etc.) — a human-readable message, not further classified; per FR-010,
    /// classifying *why* a fetch failed in any AI-relevant way is AI's job, not this function's.
    Request(String),
}

/// `extra_headers` — AI-specified headers for this one fetch (e.g. `Authorization: Key ...`),
/// already substituted with real credential values by the caller (`generate.rs`'s tool loop) —
/// see that module's own docs on why the model only ever handles a `{{name}}` placeholder, never
/// the real value. Invalid header names/values (malformed UTF-8-as-header-bytes, a name containing
/// characters `HeaderName` rejects) are silently skipped rather than failing the whole fetch — an
/// AI-composed header is inherently less trustworthy input than this codebase's own hardcoded
/// headers elsewhere, and a real target site is worth still trying to reach even if one header
/// among several was malformed.
pub(super) async fn fetch_with_redirect_trail(
    url: &str,
    extra_headers: &HashMap<String, String>,
) -> Result<FetchResult, FetchError> {
    let original_url =
        Url::parse(url).map_err(|e| FetchError::Request(format!("无效的 URL: {e}")))?;

    // Populated by the redirect policy closure below as it's invoked once per hop; read back
    // once the request completes. Scoped to this one call (see module docs) — never shared
    // across concurrent fetches.
    let hops: Arc<Mutex<Vec<Url>>> = Arc::new(Mutex::new(Vec::new()));
    let hops_for_policy = Arc::clone(&hops);
    let cap_exceeded: Arc<Mutex<bool>> = Arc::new(Mutex::new(false));
    let cap_exceeded_for_policy = Arc::clone(&cap_exceeded);

    let policy = redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= REDIRECT_CAP {
            *cap_exceeded_for_policy
                .lock()
                .unwrap_or_else(|e| e.into_inner()) = true;
            return attempt.stop();
        }
        hops_for_policy
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(attempt.url().clone());
        attempt.follow()
    });

    let client = reqwest::Client::builder()
        .redirect(policy)
        .timeout(FETCH_TIMEOUT)
        .user_agent(BROWSER_USER_AGENT)
        .build()
        .map_err(|e| FetchError::Request(format!("构建 HTTP 客户端失败: {e}")))?;

    let mut request = client.get(original_url.clone());
    for (name, value) in extra_headers {
        let (Ok(header_name), Ok(header_value)) = (
            reqwest::header::HeaderName::from_bytes(name.as_bytes()),
            reqwest::header::HeaderValue::from_str(value),
        ) else {
            continue;
        };
        request = request.header(header_name, header_value);
    }

    let resp = request
        .send()
        .await
        .map_err(|e| FetchError::Request(format!("请求失败: {e}")))?;

    let final_url = resp.url().clone();
    let status = resp.status();

    let mut trail = vec![original_url];
    trail.extend(
        hops.lock()
            .unwrap_or_else(|e| e.into_inner())
            .iter()
            .cloned(),
    );

    if *cap_exceeded.lock().unwrap_or_else(|e| e.into_inner()) {
        return Err(FetchError::RedirectCapExceeded { trail });
    }

    let body = resp
        .text()
        .await
        .map_err(|e| FetchError::Request(format!("读取响应内容失败: {e}")))?;

    let cloudflare_challenge = looks_like_cloudflare_challenge(status, &body);
    if cloudflare_challenge {
        tracing::warn!(url = %final_url, %status, "plugin wizard: fetch_page hit what looks like a Cloudflare challenge page");
    }

    Ok(FetchResult {
        redirect_trail: trail,
        final_url,
        status,
        body,
        cloudflare_challenge,
    })
}

#[cfg(test)]
mod tests {
    use axum::extract::Path;
    use axum::response::Redirect;
    use axum::routing::get;
    use axum::Router;

    use super::*;

    /// Binds a minimal local test server to an OS-assigned port and returns its base URL —
    /// T046: real network round-trips against `127.0.0.1`, not a mocked transport, since
    /// `fetch_with_redirect_trail` builds its own `reqwest::Client` with no injection point.
    async fn spawn_test_server(router: Router) -> String {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        format!("http://{addr}")
    }

    #[tokio::test]
    async fn direct_hit_has_a_one_element_trail_and_no_redirects() {
        let router = Router::new().route("/page", get(|| async { "hello" }));
        let base = spawn_test_server(router).await;

        let result = fetch_with_redirect_trail(&format!("{base}/page"), &HashMap::new())
            .await
            .unwrap();
        assert_eq!(result.redirect_trail.len(), 1);
        assert_eq!(result.status, StatusCode::OK);
        assert_eq!(result.body, "hello");
        assert_eq!(result.final_url.path(), "/page");
    }

    #[tokio::test]
    async fn a_normal_redirect_chain_is_followed_and_fully_recorded() {
        let router = Router::new()
            .route("/start", get(|| async { Redirect::to("/middle") }))
            .route("/middle", get(|| async { Redirect::to("/end") }))
            .route("/end", get(|| async { "landed" }));
        let base = spawn_test_server(router).await;

        let result = fetch_with_redirect_trail(&format!("{base}/start"), &HashMap::new())
            .await
            .unwrap();
        // original + middle + end = 3 entries in the trail.
        assert_eq!(result.redirect_trail.len(), 3);
        assert_eq!(result.redirect_trail[0].path(), "/start");
        assert_eq!(result.redirect_trail[1].path(), "/middle");
        assert_eq!(result.redirect_trail[2].path(), "/end");
        assert_eq!(result.final_url.path(), "/end");
        assert_eq!(result.body, "landed");
    }

    #[tokio::test]
    async fn exceeding_the_redirect_cap_reports_the_trail_so_far() {
        // Every hop redirects to the next-numbered one, forever — guaranteed to exceed
        // REDIRECT_CAP regardless of its exact value.
        let router = Router::new().route(
            "/hop/{n}",
            get(|Path(n): Path<usize>| async move { Redirect::to(&format!("/hop/{}", n + 1)) }),
        );
        let base = spawn_test_server(router).await;

        let err = fetch_with_redirect_trail(&format!("{base}/hop/0"), &HashMap::new())
            .await
            .unwrap_err();
        match err {
            FetchError::RedirectCapExceeded { trail } => {
                // original + (REDIRECT_CAP - 1) recorded hops: the policy checks
                // `attempt.previous().len() >= REDIRECT_CAP` *before* pushing the current hop, so
                // the trip point is reached one push earlier than the cap value itself.
                assert_eq!(trail.len(), REDIRECT_CAP);
            }
            FetchError::Request(msg) => panic!("expected RedirectCapExceeded, got Request({msg})"),
        }
    }

    #[tokio::test]
    async fn an_unparseable_url_is_a_request_error_not_a_panic() {
        let err = fetch_with_redirect_trail("not a url", &HashMap::new())
            .await
            .unwrap_err();
        assert!(matches!(err, FetchError::Request(_)));
    }
}
