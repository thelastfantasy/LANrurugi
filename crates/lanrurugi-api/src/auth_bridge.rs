//! Symmetric cross-origin login bridge (single-backend mini-SSO).
//!
//! `trusted_origins` is an equivalence group: any listed origin can be the login source for any
//! other. When an unauthenticated browser visits peer B, B prepares a one-time `state` cookie and
//! redirects through the other peers (`A`, then `C`, ...) until one reports a valid session. That
//! peer stores a one-time code in the shared Redis and redirects back to B's callback, which
//! consumes the code and issues B its own access/refresh cookies. No single "auth origin" is
//! required.
//!
//! Loopback aliases (`localhost`, `127.0.0.1`, `::1`, same scheme/port) are built in: a loopback
//! origin automatically trusts its loopback siblings without any user configuration.

use std::net::SocketAddr;

use axum::extract::{ConnectInfo, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use serde::Deserialize;
use serde_json::json;
use url::Url;

use crate::auth::LiveAuthConfig;
use crate::common::{error, not_found};
use crate::state::AppState;
use lanrurugi_storage::keys::CONFIG_KEY;
use lanrurugi_storage::refresh_tokens::SessionContext;

const SSO_STATE_COOKIE: &str = "lanrurugi_sso_state";
const SSO_STATE_PATH: &str = "/api/auth/bridge";
const SSO_STATE_MAX_AGE_SECS: i64 = 300;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/config", get(get_auth_config))
        .route("/auth/bridge/prepare", get(prepare_bridge))
        .route("/auth/bridge/start", get(start_bridge))
        .route("/auth/bridge/callback", get(bridge_callback))
}

fn now_secs() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the Unix epoch")
        .as_secs() as i64
}

fn normalize_origin(value: &str) -> Option<String> {
    crate::settings::normalize_origin(value)
}

fn request_origin(headers: &HeaderMap) -> Option<String> {
    let host = headers
        .get("x-forwarded-host")
        .or_else(|| headers.get(header::HOST))
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())?;
    let scheme = headers
        .get("x-forwarded-proto")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(',').next())
        .map(str::trim)
        .filter(|v| !v.is_empty())
        .unwrap_or("http");
    normalize_origin(&format!("{scheme}://{host}"))
}

fn is_https_origin(origin: &str) -> bool {
    Url::parse(origin)
        .ok()
        .is_some_and(|url| url.scheme() == "https")
}

fn is_loopback_origin(origin: &str) -> bool {
    let Ok(url) = Url::parse(origin) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.to_ascii_lowercase();
    host == "localhost" || host == "127.0.0.1" || host == "::1"
}

fn same_authority(a: &str, b: &str) -> bool {
    let (Ok(a), Ok(b)) = (Url::parse(a), Url::parse(b)) else {
        return false;
    };
    a.host_str() == b.host_str() && a.port_or_known_default() == b.port_or_known_default()
}

/// Loopback siblings for a loopback origin, same scheme/port: `localhost`, `127.0.0.1`, `[::1]`.
fn loopback_siblings(origin: &str) -> Vec<String> {
    let Ok(url) = Url::parse(origin) else {
        return Vec::new();
    };
    if !is_loopback_origin(origin) {
        return Vec::new();
    }
    let scheme = url.scheme();
    let port = url.port();
    ["localhost", "127.0.0.1", "::1"]
        .iter()
        .filter_map(|host| {
            let mut candidate = format!("{scheme}://{host}");
            if host == &"::1" {
                candidate = format!("{scheme}://[::1]");
            }
            if let Some(port) = port {
                candidate = format!("{candidate}:{port}");
            }
            normalize_origin(&candidate).filter(|candidate| candidate != origin)
        })
        .collect()
}

#[derive(Clone)]
struct BridgeConfig {
    live: LiveAuthConfig,
    trusted: Vec<String>,
    auto_redirect: bool,
}

impl BridgeConfig {
    fn is_trusted_origin(&self, origin: &str) -> bool {
        self.trusted.iter().any(|trusted| trusted == origin) || is_loopback_origin(origin)
    }

    /// The target must either be explicitly in `trusted_origins`, or be a loopback sibling of a
    /// loopback current origin. This keeps the built-in loopback behavior from accidentally
    /// allowing arbitrary `127.0.0.1:<port>` targets from a real configured domain.
    fn is_trusted_target(&self, current: &str, target: &str) -> bool {
        self.trusted.iter().any(|trusted| trusted == target)
            || (is_loopback_origin(current)
                && is_loopback_origin(target)
                && same_authority(current, target))
    }

    fn candidate_peers(&self, current: &str) -> Vec<String> {
        let mut peers: Vec<String> = self
            .trusted
            .iter()
            .filter(|origin| origin.as_str() != current)
            .cloned()
            .collect();
        for sibling in loopback_siblings(current) {
            if !peers.contains(&sibling) {
                peers.push(sibling);
            }
        }
        peers
    }
}

async fn load_bridge_config(state: &AppState) -> Result<BridgeConfig, String> {
    let live = crate::auth::load(state).await.map_err(|e| e.to_string())?;
    let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
    let fields: std::collections::HashMap<String, String> =
        conn.hgetall(CONFIG_KEY).await.map_err(|e| e.to_string())?;

    let mut trusted = Vec::new();
    if let Some(raw) = fields.get("trusted_origins") {
        for entry in raw
            .split([',', '\n'])
            .map(str::trim)
            .filter(|v| !v.is_empty())
        {
            if let Some(origin) = normalize_origin(entry) {
                if !trusted.contains(&origin) {
                    trusted.push(origin);
                }
            }
        }
    }
    let auto_redirect = fields
        .get("sso_auto_redirect")
        .map(|value| value != "0")
        .unwrap_or(true);

    Ok(BridgeConfig {
        live,
        trusted,
        auto_redirect,
    })
}

fn relative_return_to(value: Option<&str>) -> Option<String> {
    let value = value.unwrap_or("/").trim();
    if value.is_empty() || !value.starts_with('/') || value.starts_with("//") {
        return None;
    }
    Some(value.to_string())
}

fn set_cookie(name: &str, value: &str, path: &str, max_age: i64, secure: bool) -> String {
    format!(
        "{name}={value}; Path={path}; Max-Age={max_age}; HttpOnly; SameSite=Lax{}",
        if secure { "; Secure" } else { "" }
    )
}

fn redirect_with_cookies(location: &str, cookies: Vec<String>) -> Response {
    let mut headers = HeaderMap::new();
    headers.insert(
        header::LOCATION,
        HeaderValue::from_str(location).expect("redirect location is a valid header value"),
    );
    for cookie in cookies {
        if let Ok(value) = HeaderValue::from_str(&cookie) {
            headers.append(header::SET_COOKIE, value);
        }
    }
    (StatusCode::SEE_OTHER, headers).into_response()
}

fn redirect(location: &str) -> Response {
    redirect_with_cookies(location, Vec::new())
}

fn login_error_redirect(origin: &str, reason: &str) -> Response {
    redirect(&format!("{origin}/login?sso_error={reason}"))
}

fn encode_peers(peers: &[String]) -> String {
    peers.join(",")
}

fn parse_peers(raw: Option<&str>) -> Vec<String> {
    raw.unwrap_or("")
        .split(',')
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .filter_map(normalize_origin)
        .collect()
}

fn start_url(
    peer: &str,
    target_origin: &str,
    return_to: &str,
    state: &str,
    remaining_peers: &[String],
) -> Option<String> {
    let mut url = Url::parse(&format!("{peer}/api/auth/bridge/start")).ok()?;
    {
        let mut query = url.query_pairs_mut();
        query
            .append_pair("target_origin", target_origin)
            .append_pair("return_to", return_to)
            .append_pair("state", state);
        if !remaining_peers.is_empty() {
            query.append_pair("peers", &encode_peers(remaining_peers));
        }
    }
    Some(url.to_string())
}

#[derive(Deserialize)]
struct PrepareQuery {
    return_to: Option<String>,
}

#[derive(Deserialize)]
struct StartQuery {
    target_origin: String,
    return_to: Option<String>,
    state: String,
    #[serde(default)]
    peers: Option<String>,
}

#[derive(Deserialize)]
struct CallbackQuery {
    code: String,
    state: String,
}

async fn get_auth_config(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let cfg = match load_bridge_config(&state).await {
        Ok(cfg) => cfg,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "auth_config", e),
    };
    let current_origin = request_origin(&headers);
    let (trusted, peers) = match current_origin.as_deref() {
        Some(origin) if cfg.is_trusted_origin(origin) => (true, cfg.candidate_peers(origin)),
        _ => (false, Vec::new()),
    };
    let enabled = trusted && !peers.is_empty();
    axum::Json(json!({
        "sso_enabled": enabled,
        "auto_redirect": enabled && cfg.auto_redirect,
        "auth_origin": "",
        "current_origin": current_origin,
        "current_origin_trusted": trusted,
        "current_origin_is_auth_origin": false,
    }))
    .into_response()
}

async fn prepare_bridge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<PrepareQuery>,
) -> Response {
    let cfg = match load_bridge_config(&state).await {
        Ok(cfg) => cfg,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "bridge_prepare", e),
    };
    let Some(current_origin) = request_origin(&headers) else {
        return error(StatusCode::BAD_REQUEST, "bridge_prepare", "missing Host.");
    };
    if !cfg.is_trusted_origin(&current_origin) {
        return error(
            StatusCode::FORBIDDEN,
            "bridge_prepare",
            "current origin is not a trusted SSO peer.",
        );
    }
    let peers = cfg.candidate_peers(&current_origin);
    if peers.is_empty() {
        return not_found("bridge_prepare", "no equivalent peer origins configured.");
    }
    let Some(return_to) = relative_return_to(query.return_to.as_deref()) else {
        return error(
            StatusCode::BAD_REQUEST,
            "bridge_prepare",
            "return_to must be a relative path.",
        );
    };
    let state_value = uuid::Uuid::new_v4().simple().to_string();
    let first = peers[0].clone();
    let remaining = peers[1..].to_vec();
    let Some(start) = start_url(
        &first,
        &current_origin,
        &return_to,
        &state_value,
        &remaining,
    ) else {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "bridge_prepare",
            "invalid peer origin.",
        );
    };
    let secure = is_https_origin(&current_origin) || cfg.live.force_secure_cookies;
    let cookie = set_cookie(
        SSO_STATE_COOKIE,
        &state_value,
        SSO_STATE_PATH,
        SSO_STATE_MAX_AGE_SECS,
        secure,
    );
    (
        [(header::SET_COOKIE, cookie)],
        axum::Json(json!({ "start_url": start })),
    )
        .into_response()
}

async fn start_bridge(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<StartQuery>,
) -> Response {
    let cfg = match load_bridge_config(&state).await {
        Ok(cfg) => cfg,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "bridge_start", e),
    };
    let Some(current_origin) = request_origin(&headers) else {
        return error(StatusCode::BAD_REQUEST, "bridge_start", "missing Host.");
    };
    if !cfg.is_trusted_origin(&current_origin) {
        return error(
            StatusCode::FORBIDDEN,
            "bridge_start",
            "current origin is not a trusted SSO peer.",
        );
    }
    let Some(target_origin) = normalize_origin(&query.target_origin) else {
        return error(
            StatusCode::BAD_REQUEST,
            "bridge_start",
            "target_origin must be an exact http(s) origin.",
        );
    };
    if target_origin == current_origin || !cfg.is_trusted_target(&current_origin, &target_origin) {
        return error(
            StatusCode::FORBIDDEN,
            "bridge_start",
            "target_origin is not a trusted SSO peer.",
        );
    }
    let Some(return_to) = relative_return_to(query.return_to.as_deref()) else {
        return error(
            StatusCode::BAD_REQUEST,
            "bridge_start",
            "return_to must be a relative path.",
        );
    };
    if query.state.trim().is_empty() {
        return error(StatusCode::BAD_REQUEST, "bridge_start", "missing state.");
    }

    // This peer has a valid session: generate the one-time code and send the browser back to the
    // target's callback, where it will receive its own local cookies.
    if crate::auth::session_is_valid(&cfg.live, &headers) {
        let Some(family_id) = crate::auth::session_family_id(&cfg.live, &headers) else {
            return login_error_redirect(&current_origin, "session");
        };
        let now = now_secs();
        match state.refresh_tokens.get_family_meta(&family_id).await {
            Ok(Some(meta)) if now <= meta.expires_at && now <= meta.idle_expires_at => {}
            Ok(_) => return login_error_redirect(&current_origin, "session"),
            Err(_) => return login_error_redirect(&current_origin, "session"),
        }
        let code = match state
            .refresh_tokens
            .create_handoff_code(
                &family_id,
                &target_origin,
                &return_to,
                query.state.trim(),
                now,
            )
            .await
        {
            Ok(code) => code,
            Err(_) => return login_error_redirect(&current_origin, "handoff"),
        };
        let mut callback = match Url::parse(&format!("{target_origin}/api/auth/bridge/callback")) {
            Ok(url) => url,
            Err(_) => return login_error_redirect(&current_origin, "handoff"),
        };
        callback
            .query_pairs_mut()
            .append_pair("code", &code)
            .append_pair("state", query.state.trim());
        return redirect(callback.as_str());
    }

    // This peer has no session either: try the next candidate. The chain is finite because each
    // hop removes itself from the `peers` list.
    let remaining = parse_peers(query.peers.as_deref())
        .into_iter()
        .filter(|peer| peer != &current_origin && peer != &target_origin)
        .filter(|peer| cfg.is_trusted_target(&target_origin, peer))
        .collect::<Vec<_>>();
    if let Some((next, rest)) = remaining.split_first() {
        if let Some(next_url) =
            start_url(next, &target_origin, &return_to, query.state.trim(), rest)
        {
            return redirect(&next_url);
        }
    }

    // No peer had a session. Fall back to the target's own local login, preserving its return path.
    let next = format!("{target_origin}{return_to}");
    let login = format!("{target_origin}/login?next={}", urlencoding(&next));
    redirect(&login)
}

async fn bridge_callback(
    State(state): State<AppState>,
    ConnectInfo(peer_addr): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Query(query): Query<CallbackQuery>,
) -> Response {
    let cfg = match load_bridge_config(&state).await {
        Ok(cfg) => cfg,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, "bridge_callback", e),
    };
    let Some(current_origin) = request_origin(&headers) else {
        return error(StatusCode::BAD_REQUEST, "bridge_callback", "missing Host.");
    };
    let state_cookie = headers
        .get(header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .and_then(|raw| crate::auth::find_cookie(raw, SSO_STATE_COOKIE));
    if state_cookie.as_deref() != Some(query.state.trim()) {
        return login_error_redirect(&current_origin, "state");
    }

    let now = now_secs();
    let Some(record) = (match state
        .refresh_tokens
        .consume_handoff_code(&query.code, now)
        .await
    {
        Ok(record) => record,
        Err(_) => return login_error_redirect(&current_origin, "handoff"),
    }) else {
        return login_error_redirect(&current_origin, "handoff");
    };
    if record.target_origin != current_origin || record.state != query.state.trim() {
        return login_error_redirect(&current_origin, "handoff");
    }
    let Some(return_to) = relative_return_to(Some(&record.return_to)) else {
        return login_error_redirect(&current_origin, "handoff");
    };

    let ip = crate::procedure::client_ip(&headers, peer_addr);
    let ua = crate::procedure::user_agent(&headers);
    let device_info = crate::device_info::build(ua.as_deref(), ip.as_deref(), None);
    // Carry the source family's stable device identity onto the target origin, so a.com/b.com
    // share the same logical device and a later re-login still inherits the custom name.
    let device_id = state
        .refresh_tokens
        .get_family_meta(&record.family_id)
        .await
        .ok()
        .flatten()
        .and_then(|meta| meta.device_id)
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    let issued = match state
        .refresh_tokens
        .issue_handoff_token(
            &record.family_id,
            now,
            cfg.live.refresh_token_idle_lifetime_secs as i64,
            SessionContext {
                device_info,
                client_ip: ip,
                device_id: Some(device_id.clone()),
            },
        )
        .await
    {
        Ok(Some(issued)) => issued,
        Ok(None) | Err(_) => return login_error_redirect(&current_origin, "handoff"),
    };
    let access_token = lanrurugi_core::session::issue_access_token(
        &cfg.live.session_secret,
        now as u64,
        cfg.live.access_token_lifetime_secs,
        &issued.record.family_id,
    );
    let refresh_cookie_value = format!("{}.{}", issued.record.token_id, issued.secret);
    let refresh_max_age = (issued
        .record
        .idle_expires_at
        .unwrap_or(issued.record.expires_at)
        - now)
        .max(1) as u64;
    let cookies = crate::login::auth_cookies(
        &cfg.live,
        &access_token,
        &refresh_cookie_value,
        refresh_max_age,
    );
    let clear_state = set_cookie(SSO_STATE_COOKIE, "", SSO_STATE_PATH, 0, false);
    let mut all_cookies = cookies.to_vec();
    all_cookies.push(clear_state);
    all_cookies.push(crate::login::device_id_cookie(
        &device_id,
        is_https_origin(&current_origin) || cfg.live.force_secure_cookies,
    ));
    let target = format!("{current_origin}{return_to}");
    redirect_with_cookies(&target, all_cookies)
}

fn urlencoding(value: &str) -> String {
    url::form_urlencoded::byte_serialize(value.as_bytes()).collect()
}
