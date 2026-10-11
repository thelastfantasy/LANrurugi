//! The host's own HTTP client for plugin traffic: every `fetch()` a plugin makes is relayed here
//! (`dispatcher.ts` overrides `globalThis.fetch` to call back over the plugin's own stdin/stdout
//! channel), which is what lets the host see, meter and de-duplicate plugin network work at all.
//!
//! Two rules shape it:
//!
//! * **Never serve data the origin has not just confirmed.** A stored response is only ever handed
//!   back in answer to a *conditional* request whose reply was `304 Not Modified` — so the plugin
//!   always sees what the origin says is current, and this is a revalidation store, not a cache
//!   with a freshness guess. Responses the origin marks `no-store`, anything non-2xx, and anything
//!   carrying `Set-Cookie` are never stored at all.
//! * **One upstream request per distinct piece of work.** Concurrent identical requests collapse
//!   onto a single in-flight call, and each host gets a bounded number of sockets.
//!
//! Bodies are relayed as UTF-8 text, which is all the plugin corpus reads (`res.text()`/
//! `res.json()`); binary relay is not implemented — a plugin asking for bytes would need a
//! base64-shaped addition to the protocol.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::Duration;

use lanrurugi_core::singleflight::Singleflight;
use tokio::sync::{Mutex, Semaphore};

/// Concurrent plugin requests allowed per upstream host.
const PER_HOST_CONCURRENCY: usize = 6;
/// Distinct in-flight upstream requests across all hosts.
const MAX_INFLIGHT_REQUESTS: usize = 16;
/// Upper bound on a relayed body (a plugin pulling something huge should fail loudly, in the
/// plugin, rather than through the host's own memory).
const MAX_BODY_BYTES: usize = 32 * 1024 * 1024;
/// Redirect limit used when a plugin asks to follow redirects without naming a count.
const DEFAULT_REDIRECT_LIMIT: usize = 10;
/// Whole-request budget. Deliberately well under the pool's own per-call timeout
/// (`pool::DEFAULT_TIMEOUT`, 30s): a plugin must *see* a failing request and handle it (every
/// plugin in this corpus catches and falls back), rather than the whole invocation being killed
/// from outside with no chance to react.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(20);
/// Connect-phase budget (DNS resolution, TCP, TLS). A hostname that cannot be resolved must fail
/// in seconds, not eat the plugin's entire call budget — this was a real regression when plugin
/// HTTP moved host-side: a request for an unresolvable host used to be refused instantly by Deno's
/// `--allow-net` flag, and became a multi-second stall once a real resolver was involved.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// One relayed request, as the dispatcher sends it.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct RelayRequest {
    pub method: String,
    pub url: String,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    #[serde(default)]
    pub body: Option<String>,
    /// `Some(0)` means "do not follow redirects, hand the 3xx back to the plugin"
    /// (`init.redirect === "manual"`); `None` uses [`DEFAULT_REDIRECT_LIMIT`].
    #[serde(default)]
    pub redirect_limit: Option<usize>,
}

/// What a relayed request produced — for the plugin, a `Response`-shaped record.
#[derive(Debug, Clone)]
pub struct RelayResponse {
    pub status: u16,
    pub headers: BTreeMap<String, String>,
    pub url: String,
    pub body: String,
}

/// A stored response plus the validators that can prove it is still current.
#[derive(Debug, Clone)]
struct Stored {
    status: u16,
    headers: BTreeMap<String, String>,
    url: String,
    body: String,
    etag: Option<String>,
    last_modified: Option<String>,
}

/// The relay itself: one per [`crate::pool::PluginPool`], so the revalidation store is shared by
/// every plugin namespace this host runs.
pub struct HostHttp {
    client: reqwest::Client,
    /// One client per (redirect limit, declared-host set): `reqwest`'s redirect policy is
    /// client-level, and that policy now enforces the plugin's own declared hosts on **every** hop,
    /// so keying by limit alone would hand a plugin a client built from another plugin's allow-list
    /// (`0` = hand the 3xx back, which needs no client of its own).
    redirecting_clients: Mutex<HashMap<(usize, Vec<String>), reqwest::Client>>,
    store: Mutex<HashMap<String, Stored>>,
    inflight: Singleflight<String, Arc<Result<RelayResponse, String>>>,
    hosts: Mutex<HashMap<String, Arc<Semaphore>>>,
}

impl HostHttp {
    pub fn new() -> Self {
        Self {
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(CONNECT_TIMEOUT)
                .timeout(REQUEST_TIMEOUT)
                .build()
                .expect("a plain rustls reqwest client builds"),
            redirecting_clients: Mutex::new(HashMap::new()),
            store: Mutex::new(HashMap::new()),
            inflight: Singleflight::new(MAX_INFLIGHT_REQUESTS),
            hosts: Mutex::new(HashMap::new()),
        }
    }

    /// Relays one plugin request, enforcing `allowed_hosts` (the plugin's own declared
    /// `declared_permissions.net`, matched the way Deno's `--allow-net` matched it: an entry
    /// covers that host and its subdomains).
    pub async fn relay(
        &self,
        allowed_hosts: &[String],
        request: RelayRequest,
    ) -> Result<RelayResponse, String> {
        let parsed = url::Url::parse(&request.url)
            .map_err(|e| format!("invalid URL {:?}: {e}", request.url))?;
        let host = parsed
            .host_str()
            .ok_or_else(|| format!("URL {:?} has no host", request.url))?
            .to_ascii_lowercase();
        if !host_allowed(allowed_hosts, &host) {
            return Err(format!(
                "network access to {host:?} is not permitted: this plugin declares {:?}",
                allowed_hosts
            ));
        }

        // The redirect policy needs the declaration too (every hop is checked), and the singleflight
        // closure outlives this borrow, so it takes its own canonical copy.
        let allow_list = normalized_hosts(allowed_hosts);

        // Concurrent identical requests are one upstream request: the plugin's own sequential
        // awaits (E-Hentai's sixteen `gdata` batches, say) still cost one round trip each, but two
        // plugins — or a preview and a check — asking for the same page at the same moment do not.
        let key = request_key(&request);
        Arc::unwrap_or_clone(
            self.inflight
                .run(key, || async {
                    Arc::new(self.send(&host, &allow_list, request).await)
                })
                .await,
        )
    }

    async fn send(
        &self,
        host: &str,
        allowed_hosts: &[String],
        request: RelayRequest,
    ) -> Result<RelayResponse, String> {
        let key = request_key(&request);

        // Revalidate rather than guess: whatever was stored for this exact request is only reused
        // if the origin answers 304 to a conditional request right now.
        let (conditional, stored) = {
            let store = self.store.lock().await;
            match store.get(&key) {
                Some(entry) => (
                    Some((entry.etag.clone(), entry.last_modified.clone())),
                    Some(entry.clone()),
                ),
                None => (None, None),
            }
        };

        let limit = request.redirect_limit.unwrap_or(DEFAULT_REDIRECT_LIMIT);
        let client = self.client_for(limit, allowed_hosts).await;
        let method = reqwest::Method::from_bytes(request.method.as_bytes())
            .map_err(|e| format!("invalid HTTP method {:?}: {e}", request.method))?;
        let mut builder = client.request(method.clone(), request.url.clone());
        for (name, value) in &request.headers {
            // Hop-by-hop and length headers belong to the transport, not to the plugin.
            if matches!(
                name.to_ascii_lowercase().as_str(),
                "host" | "content-length" | "connection" | "accept-encoding"
            ) {
                continue;
            }
            builder = builder.header(name, value);
        }
        if let Some((etag, last_modified)) = conditional {
            if let Some(etag) = etag {
                builder = builder.header("If-None-Match", etag);
            }
            if let Some(last_modified) = last_modified {
                builder = builder.header("If-Modified-Since", last_modified);
            }
        }
        if let Some(body) = request.body {
            builder = builder.body(body);
        }

        let _permit = self.host_permit(host).await;
        let response = builder
            .send()
            .await
            .map_err(|e| format!("request to {} failed: {e}", redact(&request.url)))?;

        let status = response.status();
        let final_url = response.url().to_string();
        let headers = collect_headers(response.headers());

        if status == reqwest::StatusCode::NOT_MODIFIED {
            return match stored {
                Some(entry) => Ok(RelayResponse {
                    status: entry.status,
                    headers: entry.headers,
                    url: entry.url,
                    body: entry.body,
                }),
                // A 304 without anything stored cannot be answered — ask again unconditionally
                // rather than inventing an empty body the plugin would misread.
                None => Err("origin answered 304 but nothing was stored for this request".into()),
            };
        }

        let bytes = response
            .bytes()
            .await
            .map_err(|e| format!("reading the response body failed: {e}"))?;
        if bytes.len() > MAX_BODY_BYTES {
            return Err(format!(
                "response body of {} bytes exceeds the {} byte relay limit",
                bytes.len(),
                MAX_BODY_BYTES
            ));
        }
        let body = String::from_utf8_lossy(&bytes).into_owned();
        let relayed = RelayResponse {
            status: status.as_u16(),
            headers: headers.clone(),
            url: final_url.clone(),
            body: body.clone(),
        };

        if storable(status, &headers) {
            let etag = headers.get("etag").cloned();
            let last_modified = headers.get("last-modified").cloned();
            if etag.is_some() || last_modified.is_some() {
                self.store.lock().await.insert(
                    key,
                    Stored {
                        status: status.as_u16(),
                        headers,
                        url: final_url,
                        body,
                        etag,
                        last_modified,
                    },
                );
            }
        } else if status.is_success() {
            // A successful but non-storable response still replaces any older copy: keeping the
            // stale one around would let a later 304 resurrect it.
            self.store.lock().await.remove(&key);
        }
        Ok(relayed)
    }

    async fn client_for(&self, limit: usize, allowed_hosts: &[String]) -> reqwest::Client {
        if limit == 0 {
            return self.client.clone();
        }
        let allow_list = normalized_hosts(allowed_hosts);
        let mut clients = self.redirecting_clients.lock().await;
        clients
            .entry((limit, allow_list.clone()))
            .or_insert_with(|| {
                reqwest::Client::builder()
                    .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                        // The declared-host check has to happen *per hop*: validating only the URL
                        // the plugin asked for let any 3xx carry the request to a host the plugin
                        // never declared, which is exactly what a plugin's `net` permission is
                        // supposed to prevent.
                        //
                        // `previous` includes the initial URL, and `limited` compares with `>`
                        // (reqwest 0.13's own `PolicyKind::Limit` arm), so the bound is checked
                        // first and identically — an over-long chain must still fail as
                        // "too many redirects" rather than silently becoming a stopped 3xx.
                        if attempt.previous().len() > limit {
                            attempt.error("too many redirects")
                        } else if attempt
                            .url()
                            .host_str()
                            .is_none_or(|host| !host_allowed(&allow_list, host))
                        {
                            // `stop()` hands the 3xx itself back, so the plugin can see it was not
                            // followed; `storable()` keeps a non-2xx out of the revalidation store.
                            attempt.stop()
                        } else {
                            attempt.follow()
                        }
                    }))
                    .connect_timeout(CONNECT_TIMEOUT)
                    .timeout(REQUEST_TIMEOUT)
                    .build()
                    .unwrap_or_else(|_| self.client.clone())
            })
            .clone()
    }

    async fn host_permit(&self, host: &str) -> tokio::sync::OwnedSemaphorePermit {
        let semaphore = {
            let mut hosts = self.hosts.lock().await;
            hosts
                .entry(host.to_string())
                .or_insert_with(|| Arc::new(Semaphore::new(PER_HOST_CONCURRENCY)))
                .clone()
        };
        semaphore
            .acquire_owned()
            .await
            .expect("the host semaphore is never closed")
    }

    /// Number of stored revalidatable responses — diagnostics and tests only.
    pub async fn stored_count(&self) -> usize {
        self.store.lock().await.len()
    }
}

impl Default for HostHttp {
    fn default() -> Self {
        Self::new()
    }
}

/// The declared hosts in a canonical shape (`host_allowed` trims and lowercases on every
/// comparison anyway) so two declarations that mean the same thing share one cached client.
fn normalized_hosts(allowed_hosts: &[String]) -> Vec<String> {
    let mut hosts: Vec<String> = allowed_hosts
        .iter()
        .map(|entry| entry.trim().to_ascii_lowercase())
        .collect();
    hosts.sort();
    hosts.dedup();
    hosts
}

/// Whether a plugin declaring `allowed_hosts` may reach `host`. An entry covers itself and any
/// subdomain, mirroring what Deno's own `--allow-net=<entry>` flag allowed before requests were
/// relayed through the host.
pub fn host_allowed(allowed_hosts: &[String], host: &str) -> bool {
    let host = host.to_ascii_lowercase();
    allowed_hosts.iter().any(|entry| {
        let entry = entry.trim().to_ascii_lowercase();
        !entry.is_empty() && (entry == host || host.ends_with(&format!(".{entry}")))
    })
}

/// One key per distinct piece of upstream work: method, URL and body (the plugin-visible request
/// identity — headers are deliberately excluded, since the same page must not be fetched twice
/// just because one caller passed a different `Accept`).
fn request_key(request: &RelayRequest) -> String {
    format!(
        "{}\n{}\n{}",
        request.method.to_ascii_uppercase(),
        request.url,
        request.body.as_deref().unwrap_or("")
    )
}

fn storable(status: reqwest::StatusCode, headers: &BTreeMap<String, String>) -> bool {
    if !status.is_success() {
        return false;
    }
    if headers.contains_key("set-cookie") {
        return false;
    }
    !headers
        .get("cache-control")
        .is_some_and(|value| value.to_ascii_lowercase().contains("no-store"))
}

/// Response headers as the plugin's `Headers`-shaped shim reads them. Repeated header names are
/// joined with a newline rather than overwritten — `Set-Cookie` is the one that actually matters
/// (a login rotates several at once, and the dispatcher's cookie jar reads them individually).
fn collect_headers(headers: &reqwest::header::HeaderMap) -> BTreeMap<String, String> {
    let mut out: BTreeMap<String, String> = BTreeMap::new();
    for (name, value) in headers.iter() {
        let name = name.as_str().to_ascii_lowercase();
        let value = value.to_str().unwrap_or_default();
        match out.get_mut(&name) {
            Some(existing) => {
                existing.push('\n');
                existing.push_str(value);
            }
            None => {
                out.insert(name, value.to_string());
            }
        }
    }
    out
}

/// Keeps query strings out of error messages (a public API token can live there).
fn redact(url: &str) -> String {
    match url::Url::parse(url) {
        Ok(mut parsed) => {
            parsed.set_query(None);
            parsed.to_string()
        }
        Err(_) => "<unparsable url>".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(method: &str, url: &str, body: Option<&str>) -> RelayRequest {
        RelayRequest {
            method: method.to_string(),
            url: url.to_string(),
            headers: BTreeMap::new(),
            body: body.map(str::to_string),
            redirect_limit: None,
        }
    }

    #[test]
    fn a_declared_host_covers_itself_and_its_subdomains() {
        let allowed = vec!["e-hentai.org".to_string()];
        assert!(host_allowed(&allowed, "e-hentai.org"));
        assert!(host_allowed(&allowed, "api.e-hentai.org"));
        assert!(host_allowed(&allowed, "API.E-Hentai.ORG"));
        // A different registrable domain that merely ends with the same letters is not a subdomain.
        assert!(!host_allowed(&allowed, "not-e-hentai.org"));
        assert!(!host_allowed(&allowed, "e-hentai.org.evil.test"));
        assert!(!host_allowed(&[], "e-hentai.org"));
    }

    #[test]
    fn the_request_key_covers_method_url_and_body() {
        assert_eq!(
            request_key(&request("get", "https://a.test/x", None)),
            request_key(&request("GET", "https://a.test/x", None)),
            "method case is not part of the identity"
        );
        assert_ne!(
            request_key(&request("POST", "https://a.test/x", Some("{\"a\":1}"))),
            request_key(&request("POST", "https://a.test/x", Some("{\"a\":2}"))),
            "a different gdata batch is a different request"
        );
        assert_ne!(
            request_key(&request("GET", "https://a.test/x", None)),
            request_key(&request("GET", "https://a.test/y", None))
        );
    }

    #[test]
    fn only_revalidatable_successes_are_stored() {
        let with = |pairs: &[(&str, &str)]| {
            pairs
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect::<BTreeMap<String, String>>()
        };
        assert!(storable(
            reqwest::StatusCode::OK,
            &with(&[("etag", "\"v1\"")])
        ));
        assert!(!storable(reqwest::StatusCode::NOT_FOUND, &with(&[])));
        assert!(
            !storable(
                reqwest::StatusCode::OK,
                &with(&[("set-cookie", "session=1")])
            ),
            "a per-session response must never be replayed"
        );
        assert!(!storable(
            reqwest::StatusCode::OK,
            &with(&[("cache-control", "no-store")])
        ));
    }

    /// Regression guard for a real bug this relay introduced: a plugin request to a host that
    /// cannot be resolved used to be refused *instantly* by Deno's own `--allow-net` flag, and
    /// became a stall long enough to eat the plugin's whole call budget (30s) once a real resolver
    /// was involved. The connect budget must bound it, so the plugin sees a catchable error.
    #[tokio::test]
    async fn an_unresolvable_host_fails_within_the_connect_budget() {
        let http = HostHttp::new();
        let started = std::time::Instant::now();
        let error = http
            .relay(
                &["unresolvable.invalid".to_string()],
                request("GET", "https://unresolvable.invalid/x", None),
            )
            .await
            .expect_err("an unresolvable host must fail");
        let elapsed = started.elapsed();
        assert!(
            elapsed < Duration::from_secs(15),
            "took {elapsed:?} (should be bounded by the connect budget): {error}"
        );
    }

    /// A one-shot local HTTP server: each accepted connection is answered with the next scripted
    /// response, and the returned counter is how many connections actually arrived. Blocking
    /// `std::net` on its own thread deliberately — the tests only need "did a second request
    /// happen, and what did it get", and this keeps the plugin crate's tokio features untouched.
    fn spawn_server(
        responses: Vec<&'static str>,
    ) -> (u16, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind a local port");
        let port = listener.local_addr().expect("local addr").port();
        let seen = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = seen.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { break };
                let index = counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let response = responses
                    .get(index)
                    .or_else(|| responses.last())
                    .copied()
                    .unwrap_or("HTTP/1.1 200 OK\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
                // Drain the request head first: answering before the client finishes sending makes
                // some clients report a connection error instead of the response.
                let mut scratch = [0u8; 4096];
                let _ = stream.read(&mut scratch);
                let _ = stream.write_all(response.as_bytes());
                let _ = stream.flush();
            }
        });
        (port, seen)
    }

    /// The security fix this test exists for: a plugin's `net` declaration has to hold on *every*
    /// hop, not just the URL it asked for. The redirect target is `.invalid`, so a followed
    /// redirect could not even resolve — `Ok` with the 3xx itself is only reachable by stopping
    /// before the request is made.
    #[tokio::test]
    async fn a_redirect_to_an_undeclared_host_is_not_followed() {
        let (port, seen) = spawn_server(vec![
            "HTTP/1.1 302 Found\r\nLocation: http://blocked.invalid/secret\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
        ]);
        let http = HostHttp::new();
        let mut req = request("GET", &format!("http://127.0.0.1:{port}/start"), None);
        req.redirect_limit = Some(10);

        let response = http
            .relay(&["127.0.0.1".to_string()], req)
            .await
            .expect("a stopped redirect must not turn into an error");
        assert_eq!(response.status, 302, "the 3xx itself is handed back");
        assert_eq!(
            seen.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "only the declared host may be contacted"
        );
    }

    /// Positive control: the same setup, redirecting to a host the plugin *did* declare, must still
    /// be followed — otherwise "deny everything" would pass the test above.
    #[tokio::test]
    async fn a_redirect_within_a_declared_host_is_still_followed() {
        let (port, seen) = spawn_server(vec![
            "HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\nConnection: close\r\n\r\n",
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
        ]);
        let http = HostHttp::new();
        let mut req = request("GET", &format!("http://127.0.0.1:{port}/start"), None);
        req.redirect_limit = Some(10);

        let response = http
            .relay(&["127.0.0.1".to_string()], req)
            .await
            .expect("a declared-host redirect must succeed");
        assert_eq!(response.status, 200);
        assert_eq!(response.body, "ok");
        assert_eq!(seen.load(std::sync::atomic::Ordering::SeqCst), 2);
    }

    #[test]
    fn error_messages_do_not_leak_query_strings() {
        assert_eq!(redact("https://a.test/x?token=secret"), "https://a.test/x");
    }
}
