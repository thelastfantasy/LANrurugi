//! Fetches a URL for real on the model's behalf, under the outbound fetch policy
//! (`specs/014-ai-harness-hardening/contracts/outbound-fetch-policy.md`), recording the complete
//! redirect trail it actually followed (FR-011 of `specs/006-ai-plugin-wizard`).
//!
//! Redirects are followed by hand rather than by `reqwest`'s policy: every hop must be evaluated
//! (keep credentials, drop them, or stop), and `reqwest` only strips its own short list of standard
//! sensitive headers on a cross-host hop while also stripping them within the same site. Each
//! hop's host is resolved first, every resolved address is checked, and the connection is pinned
//! to exactly those addresses.
//!
//! Residual risk: this module is the only gate. A future second fetcher that bypasses
//! [`fetch_with_policy`] reintroduces every risk the policy closes.

use std::collections::{HashMap, HashSet};
use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use reqwest::{StatusCode, Url};

/// Redirect hops a single access may follow before being judged a failure — `reqwest`'s own
/// default cap, made explicit and reportable.
const REDIRECT_CAP: usize = 10;

/// Bounds one page fetch's own worst-case latency; separate from the model call's deadline.
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// The most of one response body ever read into memory. Beyond it the read stops and the source's
/// size is reported as a lower bound (FR-016's cap applies to the read itself, T028).
pub(super) const MAX_READ_BYTES: usize = 2 * 1024 * 1024;

/// A real desktop-Chrome UA string — `reqwest`'s own default is trivially fingerprinted and
/// blocked by Cloudflare-style bot mitigation on many target sites.
const BROWSER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 \
    (KHTML, like Gecko) Chrome/128.0.0.0 Safari/537.36";

/// Recognizes Cloudflare's interstitial "challenge" response by its well-known markup, so a
/// challenge page is reported as such instead of being read as the target's real structure.
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

/// Lower-cased host of a user-supplied link or bare domain (`https://www.a.example/x` and
/// `a.example` both work). `None` when nothing host-like is present.
pub(super) fn host_of(input: &str) -> Option<String> {
    let input = input.trim();
    if input.is_empty() {
        return None;
    }
    let with_scheme = if input.contains("://") {
        input.to_string()
    } else {
        format!("https://{input}")
    };
    Url::parse(&with_scheme)
        .ok()?
        .host_str()
        .map(|h| h.trim_end_matches('.').to_ascii_lowercase())
}

/// `host` is `member` or a proper subdomain of it, compared at DNS-label boundaries — so
/// `img.example.com` matches `example.com` but `evil-example.com` never does.
pub(super) fn host_matches(host: &str, member: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    let member = member.trim_end_matches('.').to_ascii_lowercase();
    host == member
        || host
            .strip_suffix(&member)
            .is_some_and(|prefix| prefix.ends_with('.'))
}

/// Loopback, private, link-local, unique-local, unspecified, multicast, broadcast and other
/// non-global ranges (FR-004).
pub(super) fn is_forbidden_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_loopback()
                || v4.is_private()
                || v4.is_link_local()
                || v4.is_unspecified()
                || v4.is_multicast()
                || v4.is_broadcast()
                || v4.is_documentation()
                || o[0] == 0
                // Carrier-grade NAT 100.64.0.0/10.
                || (o[0] == 100 && (o[1] & 0xC0) == 64)
                // 198.18.0.0/15 benchmarking.
                || (o[0] == 198 && (o[1] & 0xFE) == 18)
        }
        IpAddr::V6(v6) => {
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_forbidden_address(IpAddr::V4(v4));
            }
            let first = v6.segments()[0];
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fc00::/7 unique-local.
                || (first & 0xFE00) == 0xFC00
                // fe80::/10 link-local.
                || (first & 0xFFC0) == 0xFE80
        }
    }
}

/// Every `{{name}}` placeholder referenced by a header value.
pub(super) fn placeholder_names(value: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut rest = value;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        let Some(end) = after.find("}}") else {
            break;
        };
        names.push(after[..end].trim().to_string());
        rest = &after[end + 2..];
    }
    names
}

/// Why a request was refused before any traffic (data-model.md §3).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Refusal {
    SchemeNotAllowed,
    HostNotPermitted { host: String },
    CredentialHostNotPermitted { host: String },
    PrivateAddress { host: String },
    UnresolvableHost { host: String },
    UnknownCredentialReference { name: String },
    RedirectLeftPermittedSet { host: String },
}

impl Refusal {
    pub(super) fn reason(&self) -> &'static str {
        match self {
            Self::SchemeNotAllowed => "scheme_not_allowed",
            Self::HostNotPermitted { .. } => "host_not_permitted",
            Self::CredentialHostNotPermitted { .. } => "credential_host_not_permitted",
            Self::PrivateAddress { .. } => "private_address",
            Self::UnresolvableHost { .. } => "unresolvable_host",
            Self::UnknownCredentialReference { .. } => "unknown_credential_reference",
            Self::RedirectLeftPermittedSet { .. } => "redirect_left_permitted_set",
        }
    }
}

/// The two host sets of one wizard action, derived only from what the user typed (FR-001/FR-002).
#[derive(Debug, Clone, Default)]
pub(super) struct FetchPolicy {
    /// Target domain plus every user-supplied link host: what may be fetched at all.
    fetch_hosts: Vec<String>,
    /// Target domain only: what may receive a substituted credential.
    credential_hosts: Vec<String>,
    /// Field names with a real value available for `{{name}}` substitution.
    credential_fields: HashSet<String>,
    #[cfg(test)]
    test_overrides: TestOverrides,
}

#[cfg(test)]
#[derive(Debug, Clone, Default)]
struct TestOverrides {
    /// Host → address, standing in for DNS so stub servers can play distinct sites.
    resolve: HashMap<String, IpAddr>,
    /// Lets tests reach their `127.0.0.1` stubs.
    allow_private: bool,
}

impl FetchPolicy {
    pub(super) fn new<'a>(
        target_domain: Option<&str>,
        links: impl IntoIterator<Item = &'a str>,
        credential_fields: impl IntoIterator<Item = String>,
    ) -> Self {
        let target = target_domain.and_then(host_of);
        let mut fetch_hosts: Vec<String> = target.iter().cloned().collect();
        for link in links {
            if let Some(host) = host_of(link) {
                if !fetch_hosts.contains(&host) {
                    fetch_hosts.push(host);
                }
            }
        }
        Self {
            fetch_hosts,
            credential_hosts: target.into_iter().collect(),
            credential_fields: credential_fields.into_iter().collect(),
            #[cfg(test)]
            test_overrides: TestOverrides::default(),
        }
    }

    pub(super) fn fetch_hosts(&self) -> &[String] {
        &self.fetch_hosts
    }

    pub(super) fn allows_fetch(&self, host: &str) -> bool {
        self.fetch_hosts.iter().any(|m| host_matches(host, m))
    }

    fn allows_credentials(&self, host: &str) -> bool {
        self.credential_hosts.iter().any(|m| host_matches(host, m))
    }

    fn address_allowed(&self, ip: IpAddr) -> bool {
        #[cfg(test)]
        if self.test_overrides.allow_private {
            return true;
        }
        !is_forbidden_address(ip)
    }

    /// Resolves `host` and returns the addresses to pin, or the refusal.
    async fn resolve_checked(&self, host: &str, port: u16) -> Result<Vec<SocketAddr>, Refusal> {
        let addrs: Vec<SocketAddr> = if let Ok(ip) = host.trim_matches(['[', ']']).parse::<IpAddr>()
        {
            vec![SocketAddr::new(ip, port)]
        } else {
            #[cfg(test)]
            if let Some(ip) = self.test_overrides.resolve.get(host) {
                return Ok(vec![SocketAddr::new(*ip, port)]);
            }
            tokio::net::lookup_host((host, port))
                .await
                .map(|it| it.collect())
                .unwrap_or_default()
        };
        if addrs.is_empty() {
            return Err(Refusal::UnresolvableHost {
                host: host.to_string(),
            });
        }
        if addrs.iter().any(|a| !self.address_allowed(a.ip())) {
            return Err(Refusal::PrivateAddress {
                host: host.to_string(),
            });
        }
        Ok(addrs)
    }

    #[cfg(test)]
    pub(super) fn for_test(
        target: &str,
        links: &[&str],
        credentials: &[&str],
        resolve: &[(&str, IpAddr)],
    ) -> Self {
        let mut policy = Self::new(
            Some(target),
            links.iter().copied(),
            credentials.iter().map(|s| s.to_string()),
        );
        policy.test_overrides = TestOverrides {
            resolve: resolve.iter().map(|(h, ip)| (h.to_string(), *ip)).collect(),
            allow_private: true,
        };
        policy
    }
}

#[derive(Debug)]
pub(super) struct FetchResult {
    /// The original URL, every intermediate hop, and the final landing URL, in order.
    pub redirect_trail: Vec<Url>,
    pub final_url: Url,
    pub status: StatusCode,
    /// The body as read, at most [`MAX_READ_BYTES`].
    pub body: String,
    /// Bytes the source sent, counting only up to one byte past the read cap — an upper bound
    /// on what was withheld when `body_truncated`.
    pub total_bytes: usize,
    pub body_truncated: bool,
    pub cloudflare_challenge: bool,
    /// The hop (index into `redirect_trail`) from which credential headers were withheld.
    pub withheld_credentials_from_hop: Option<usize>,
}

#[derive(Debug)]
pub(super) enum FetchError {
    /// More than [`REDIRECT_CAP`] hops occurred before a final response was reached — carries
    /// the trail accumulated so far.
    RedirectCapExceeded { trail: Vec<Url> },
    /// The policy refused the request (or a hop of it) before any traffic to that destination.
    Refused { refusal: Refusal, trail: Vec<Url> },
    /// Any other failure to complete the request — a human-readable message, not further
    /// classified (classifying why a fetch failed is the model's job, not this function's).
    Request(String),
}

/// `raw_headers` are the model-composed headers *before* substitution; credentials are
/// substituted here, per hop, only after that hop passed every check (evaluation strictly
/// precedes substitution and traffic).
pub(super) async fn fetch_with_policy(
    url: &str,
    raw_headers: &HashMap<String, String>,
    credential_values: &HashMap<String, String>,
    policy: &FetchPolicy,
) -> Result<FetchResult, FetchError> {
    let original_url =
        Url::parse(url).map_err(|_| refused(Refusal::SchemeNotAllowed, Vec::new()))?;

    // Step 6 (credential references) does not depend on the hop, so it is settled once.
    let mut credentialed = false;
    for value in raw_headers.values() {
        for name in placeholder_names(value) {
            if !policy.credential_fields.contains(&name) || !credential_values.contains_key(&name) {
                return Err(refused(
                    Refusal::UnknownCredentialReference { name },
                    vec![original_url],
                ));
            }
            credentialed = true;
        }
    }

    let mut trail = vec![original_url.clone()];
    let mut current = original_url;
    let mut withheld_from: Option<usize> = None;

    loop {
        let hop = trail.len() - 1;
        if !matches!(current.scheme(), "http" | "https") {
            return Err(refused(Refusal::SchemeNotAllowed, trail));
        }
        let Some(host) = current.host_str().map(|h| h.to_ascii_lowercase()) else {
            return Err(refused(Refusal::SchemeNotAllowed, trail));
        };
        if !policy.allows_fetch(&host) {
            let refusal = if hop == 0 {
                Refusal::HostNotPermitted { host }
            } else {
                Refusal::RedirectLeftPermittedSet { host }
            };
            return Err(refused(refusal, trail));
        }
        // A credentialed request must start on the target site; a later hop off it drops the
        // credentials for the rest of the chain.
        let keep_credentials =
            credentialed && withheld_from.is_none() && policy.allows_credentials(&host);
        if credentialed && hop == 0 && !keep_credentials {
            return Err(refused(Refusal::CredentialHostNotPermitted { host }, trail));
        }
        if credentialed && !keep_credentials && withheld_from.is_none() {
            withheld_from = Some(hop);
        }

        let port = current.port_or_known_default().unwrap_or(443);
        let addrs = policy
            .resolve_checked(&host, port)
            .await
            .map_err(|r| refused(r, trail.clone()))?;

        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .timeout(FETCH_TIMEOUT)
            .user_agent(BROWSER_USER_AGENT)
            .resolve_to_addrs(&host, &addrs)
            .build()
            .map_err(|e| FetchError::Request(format!("构建 HTTP 客户端失败: {e}")))?;

        let mut request = client.get(current.clone());
        for (name, value) in raw_headers {
            let value = if placeholder_names(value).is_empty() {
                value.clone()
            } else if keep_credentials {
                substitute(value, credential_values)
            } else {
                continue;
            };
            // Invalid header names/values are skipped rather than failing the whole fetch.
            let (Ok(header_name), Ok(header_value)) = (
                reqwest::header::HeaderName::from_bytes(name.as_bytes()),
                reqwest::header::HeaderValue::from_str(&value),
            ) else {
                continue;
            };
            request = request.header(header_name, header_value);
        }

        let resp = request
            .send()
            .await
            .map_err(|e| FetchError::Request(format!("请求失败: {e}")))?;
        let status = resp.status();

        if status.is_redirection() {
            if let Some(next) = resp
                .headers()
                .get(reqwest::header::LOCATION)
                .and_then(|v| v.to_str().ok())
                .and_then(|loc| current.join(loc).ok())
            {
                if trail.len() >= REDIRECT_CAP {
                    return Err(FetchError::RedirectCapExceeded { trail });
                }
                trail.push(next.clone());
                current = next;
                continue;
            }
        }

        let (body, total_bytes, body_truncated) = read_capped(resp).await?;
        let cloudflare_challenge = looks_like_cloudflare_challenge(status, &body);
        if cloudflare_challenge {
            tracing::warn!(url = %current, %status, "plugin wizard: fetch_page hit what looks like a Cloudflare challenge page");
        }
        return Ok(FetchResult {
            final_url: current,
            redirect_trail: trail,
            status,
            body,
            total_bytes,
            body_truncated,
            cloudflare_challenge,
            withheld_credentials_from_hop: withheld_from,
        });
    }
}

fn refused(refusal: Refusal, trail: Vec<Url>) -> FetchError {
    FetchError::Refused { refusal, trail }
}

/// Replaces every `{{name}}` with its real value. Only called for a hop allowed to carry them.
fn substitute(value: &str, credential_values: &HashMap<String, String>) -> String {
    let mut out = value.to_string();
    for (name, real) in credential_values {
        out = out.replace(&format!("{{{{{name}}}}}"), real);
    }
    out
}

/// Reads at most [`MAX_READ_BYTES`]; returns `(text, bytes_seen, truncated)`.
async fn read_capped(mut resp: reqwest::Response) -> Result<(String, usize, bool), FetchError> {
    let mut buf: Vec<u8> = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| FetchError::Request(format!("读取响应内容失败: {e}")))?
    {
        let room = MAX_READ_BYTES - buf.len();
        if chunk.len() > room {
            buf.extend_from_slice(&chunk[..room]);
            truncated = true;
            break;
        }
        buf.extend_from_slice(&chunk);
    }
    let seen = buf.len() + usize::from(truncated);
    Ok((String::from_utf8_lossy(&buf).into_owned(), seen, truncated))
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};

    use axum::extract::Path;
    use axum::http::HeaderMap;
    use axum::response::Redirect;
    use axum::routing::get;
    use axum::Router;

    use super::*;

    const LOCAL: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

    /// Binds a minimal local test server to an OS-assigned port and returns its port — real
    /// network round-trips against `127.0.0.1`, not a mocked transport.
    async fn spawn(router: Router) -> u16 {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        port
    }

    /// A stub that counts requests and records the `Authorization` header of each.
    #[derive(Clone, Default)]
    struct Recorder {
        hits: Arc<AtomicUsize>,
        auth: Arc<Mutex<Vec<Option<String>>>>,
    }

    impl Recorder {
        fn router(&self, path: &'static str, reply: &'static str) -> Router {
            let me = self.clone();
            Router::new().route(
                path,
                get(move |headers: HeaderMap| {
                    let me = me.clone();
                    async move {
                        me.hits.fetch_add(1, Ordering::SeqCst);
                        me.auth.lock().unwrap().push(
                            headers
                                .get("authorization")
                                .and_then(|v| v.to_str().ok())
                                .map(str::to_string),
                        );
                        reply
                    }
                }),
            )
        }
        fn hits(&self) -> usize {
            self.hits.load(Ordering::SeqCst)
        }
    }

    fn open_policy() -> FetchPolicy {
        FetchPolicy::for_test("127.0.0.1", &[], &[], &[])
    }

    fn no_headers() -> HashMap<String, String> {
        HashMap::new()
    }

    fn token_header() -> HashMap<String, String> {
        HashMap::from([("Authorization".to_string(), "Key {{token}}".to_string())])
    }

    fn token_value() -> HashMap<String, String> {
        HashMap::from([("token".to_string(), "SECRET-1".to_string())])
    }

    /// site-a (target), site-c (a supplied link); site-b is never supplied.
    fn three_sites() -> FetchPolicy {
        FetchPolicy::for_test(
            "site-a.example",
            &[
                "https://site-a.example/gallery",
                "https://site-c.example/mirror",
            ],
            &["token"],
            &[
                ("site-a.example", LOCAL),
                ("img.site-a.example", LOCAL),
                ("site-b.example", LOCAL),
                ("not-site-a.example", LOCAL),
                ("site-c.example", LOCAL),
            ],
        )
    }

    #[tokio::test]
    async fn direct_hit_has_a_one_element_trail_and_no_redirects() {
        let port = spawn(Router::new().route("/page", get(|| async { "hello" }))).await;
        let result = fetch_with_policy(
            &format!("http://127.0.0.1:{port}/page"),
            &no_headers(),
            &HashMap::new(),
            &open_policy(),
        )
        .await
        .unwrap();
        assert_eq!(result.redirect_trail.len(), 1);
        assert_eq!(result.status, StatusCode::OK);
        assert_eq!(result.body, "hello");
        assert_eq!(result.final_url.path(), "/page");
        assert!(!result.body_truncated);
    }

    #[tokio::test]
    async fn a_normal_redirect_chain_is_followed_and_fully_recorded() {
        let router = Router::new()
            .route("/start", get(|| async { Redirect::to("/middle") }))
            .route("/middle", get(|| async { Redirect::to("/end") }))
            .route("/end", get(|| async { "landed" }));
        let port = spawn(router).await;
        let result = fetch_with_policy(
            &format!("http://127.0.0.1:{port}/start"),
            &no_headers(),
            &HashMap::new(),
            &open_policy(),
        )
        .await
        .unwrap();
        assert_eq!(result.redirect_trail.len(), 3);
        assert_eq!(result.redirect_trail[1].path(), "/middle");
        assert_eq!(result.final_url.path(), "/end");
        assert_eq!(result.body, "landed");
    }

    #[tokio::test]
    async fn exceeding_the_redirect_cap_reports_the_trail_so_far() {
        let router = Router::new().route(
            "/hop/{n}",
            get(|Path(n): Path<usize>| async move { Redirect::to(&format!("/hop/{}", n + 1)) }),
        );
        let port = spawn(router).await;
        let err = fetch_with_policy(
            &format!("http://127.0.0.1:{port}/hop/0"),
            &no_headers(),
            &HashMap::new(),
            &open_policy(),
        )
        .await
        .unwrap_err();
        match err {
            FetchError::RedirectCapExceeded { trail } => assert_eq!(trail.len(), REDIRECT_CAP),
            other => panic!("expected RedirectCapExceeded, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn an_unparseable_url_is_refused_not_a_panic() {
        let err = fetch_with_policy("not a url", &no_headers(), &HashMap::new(), &open_policy())
            .await
            .unwrap_err();
        assert!(matches!(
            err,
            FetchError::Refused {
                refusal: Refusal::SchemeNotAllowed,
                ..
            }
        ));
    }

    #[test]
    fn host_matching_respects_label_boundaries() {
        assert!(host_matches("example.com", "example.com"));
        assert!(host_matches("img.example.com", "example.com"));
        assert!(host_matches("IMG.Example.com.", "example.com"));
        assert!(!host_matches("evil-example.com", "example.com"));
        assert!(!host_matches("example.com.evil.net", "example.com"));
        assert!(!host_matches("com", "example.com"));
    }

    #[test]
    fn forbidden_address_classes() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.1",
            "192.168.1.1",
            "169.254.169.254",
            "0.0.0.0",
            "100.64.0.1",
            "::1",
            "fc00::1",
            "fe80::1",
            "::ffff:127.0.0.1",
            "224.0.0.1",
        ] {
            assert!(is_forbidden_address(ip.parse().unwrap()), "{ip}");
        }
        for ip in ["93.184.216.34", "2606:4700::6810:84e5", "8.8.8.8"] {
            assert!(!is_forbidden_address(ip.parse().unwrap()), "{ip}");
        }
    }

    #[test]
    fn hosts_come_only_from_user_input() {
        let policy = FetchPolicy::new(
            Some("Site-A.example"),
            ["https://site-c.example/x", "not a url at all"],
            ["token".to_string()],
        );
        assert_eq!(policy.fetch_hosts(), ["site-a.example", "site-c.example"]);
        assert!(policy.allows_credentials("img.site-a.example"));
        assert!(!policy.allows_credentials("site-c.example"));
    }

    // Quickstart S1, steps 1–10.

    #[tokio::test]
    async fn s1_credential_reaches_the_target_site() {
        let a = Recorder::default();
        let port = spawn(a.router("/docs", "docs")).await;
        let result = fetch_with_policy(
            &format!("http://site-a.example:{port}/docs"),
            &token_header(),
            &token_value(),
            &three_sites(),
        )
        .await
        .unwrap();
        assert_eq!(result.body, "docs");
        assert_eq!(a.auth.lock().unwrap()[0].as_deref(), Some("Key SECRET-1"));
    }

    #[tokio::test]
    async fn s1_foreign_host_is_refused_with_zero_traffic() {
        let b = Recorder::default();
        let port = spawn(b.router("/", "b")).await;
        let err = fetch_with_policy(
            &format!("http://site-b.example:{port}/"),
            &token_header(),
            &token_value(),
            &three_sites(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            FetchError::Refused {
                refusal: Refusal::HostNotPermitted { .. },
                ..
            }
        ));
        assert_eq!(b.hits(), 0);
    }

    #[tokio::test]
    async fn s1_redirect_off_the_supplied_sites_ends_the_chain() {
        let b = Recorder::default();
        let b_port = spawn(b.router("/x", "b")).await;
        let a_port = spawn(Router::new().route(
            "/start",
            get(move || async move { Redirect::to(&format!("http://site-b.example:{b_port}/x")) }),
        ))
        .await;
        let err = fetch_with_policy(
            &format!("http://site-a.example:{a_port}/start"),
            &token_header(),
            &token_value(),
            &three_sites(),
        )
        .await
        .unwrap_err();
        match err {
            FetchError::Refused {
                refusal: Refusal::RedirectLeftPermittedSet { host },
                trail,
            } => {
                assert_eq!(host, "site-b.example");
                assert_eq!(trail.len(), 2);
            }
            other => panic!("expected RedirectLeftPermittedSet, got {other:?}"),
        }
        assert_eq!(b.hits(), 0);
    }

    #[tokio::test]
    async fn s1_redirect_to_a_supplied_site_continues_without_credentials() {
        let c = Recorder::default();
        let c_port = spawn(c.router("/img", "c")).await;
        let a = Recorder::default();
        let a_port = spawn(a.router("/cdn", "unused").route(
            "/go",
            get(
                move || async move { Redirect::to(&format!("http://site-c.example:{c_port}/img")) },
            ),
        ))
        .await;
        let result = fetch_with_policy(
            &format!("http://site-a.example:{a_port}/go"),
            &token_header(),
            &token_value(),
            &three_sites(),
        )
        .await
        .unwrap();
        assert_eq!(result.body, "c");
        assert_eq!(result.withheld_credentials_from_hop, Some(1));
        assert_eq!(c.auth.lock().unwrap().as_slice(), [None]);
    }

    #[tokio::test]
    async fn s1_subdomain_allowed_and_suffix_lookalike_refused() {
        let img = Recorder::default();
        let port = spawn(img.router("/p.png", "png")).await;
        let ok = fetch_with_policy(
            &format!("http://img.site-a.example:{port}/p.png"),
            &no_headers(),
            &HashMap::new(),
            &three_sites(),
        )
        .await
        .unwrap();
        assert_eq!(ok.body, "png");

        let err = fetch_with_policy(
            &format!("http://not-site-a.example:{port}/p.png"),
            &no_headers(),
            &HashMap::new(),
            &three_sites(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            FetchError::Refused {
                refusal: Refusal::HostNotPermitted { .. },
                ..
            }
        ));
        assert_eq!(img.hits(), 1);
    }

    #[tokio::test]
    async fn s1_private_and_non_http_destinations_are_never_dialled() {
        // The real policy, without the test escape hatch for loopback.
        let policy = FetchPolicy::new(
            Some("127.0.0.1"),
            ["http://169.254.169.254/"],
            Vec::<String>::new(),
        );
        for (url, reason) in [
            ("http://127.0.0.1:8500/", "private_address"),
            ("http://169.254.169.254/", "private_address"),
            ("file:///etc/passwd", "scheme_not_allowed"),
        ] {
            match fetch_with_policy(url, &no_headers(), &HashMap::new(), &policy).await {
                Err(FetchError::Refused { refusal, .. }) => {
                    assert_eq!(refusal.reason(), reason, "{url}")
                }
                other => panic!("{url}: expected refusal, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn s1_unknown_credential_reference_is_refused_not_emptied() {
        let a = Recorder::default();
        let port = spawn(a.router("/", "a")).await;
        let headers = HashMap::from([(
            "Authorization".to_string(),
            "Key {{nonexistent}}".to_string(),
        )]);
        let err = fetch_with_policy(
            &format!("http://site-a.example:{port}/"),
            &headers,
            &token_value(),
            &three_sites(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            FetchError::Refused {
                refusal: Refusal::UnknownCredentialReference { .. },
                ..
            }
        ));
        assert_eq!(a.hits(), 0);
    }

    #[tokio::test]
    async fn s1_credential_to_a_supplied_non_target_site_is_refused() {
        let c = Recorder::default();
        let port = spawn(c.router("/", "c")).await;
        let err = fetch_with_policy(
            &format!("http://site-c.example:{port}/"),
            &token_header(),
            &token_value(),
            &three_sites(),
        )
        .await
        .unwrap_err();
        assert!(matches!(
            err,
            FetchError::Refused {
                refusal: Refusal::CredentialHostNotPermitted { .. },
                ..
            }
        ));
        assert_eq!(c.hits(), 0);

        // Without a credential the same site is fetched normally.
        let ok = fetch_with_policy(
            &format!("http://site-c.example:{port}/"),
            &no_headers(),
            &HashMap::new(),
            &three_sites(),
        )
        .await
        .unwrap();
        assert_eq!(ok.body, "c");
        assert_eq!(c.auth.lock().unwrap().as_slice(), [None]);
    }

    #[tokio::test]
    async fn an_oversized_body_is_read_only_up_to_the_cap() {
        let big: &'static str = Box::leak("x".repeat(MAX_READ_BYTES + 10_000).into_boxed_str());
        let port = spawn(Router::new().route("/big", get(move || async move { big }))).await;
        let result = fetch_with_policy(
            &format!("http://127.0.0.1:{port}/big"),
            &no_headers(),
            &HashMap::new(),
            &open_policy(),
        )
        .await
        .unwrap();
        assert!(result.body_truncated);
        assert_eq!(result.body.len(), MAX_READ_BYTES);
        assert!(result.total_bytes > MAX_READ_BYTES);
    }
}
