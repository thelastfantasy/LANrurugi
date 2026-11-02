//! `lanrurugi-server telemetry-ingest`: the sink for `lanrurugi_api::heartbeat`'s startup ping.
//!
//! One instance of this serves the whole project (behind a Cloudflare Tunnel), not one per
//! deployment. It stays deliberately small: validate four fields, take the country from
//! `CF-IPCountry` (Cloudflare sets it at the edge, so no IP is ever read, logged or stored), and
//! fold the id into per-day Redis sets.
//!
//! Run it against its own Redis, never the instance a library uses, and never publish 6379 — this
//! process is the only thing that should reach it.

use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, DefaultBodyLimit, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::{Config, Pool, Runtime};
use serde::Deserialize;
use tower_http::services::ServeDir;

/// The four fields plus JSON punctuation sit far below this; anything larger is not a client this
/// service supports.
const MAX_BODY: usize = 1024;
/// One installation sends one request a day. This only stops a buggy client loop or a scanner from
/// becoming a Redis write amplifier.
const MAX_PER_MINUTE: u32 = 10;
/// Per-day keys live this long unless `--retention-days` says otherwise.
const DEFAULT_RETENTION_DAYS: u64 = 400;

#[derive(clap::Args)]
pub struct TelemetryIngestArgs {
    /// Address to bind. Keep it off the public interface: the tunnel reaches it on the container
    /// network, so nothing but `cloudflared` should be able to connect.
    #[arg(long, env = "LANRURUGI_TELEMETRY_BIND", default_value = "0.0.0.0:8099")]
    pub bind: String,

    /// Redis this sink writes to. Deliberately its own instance/database, not a deployment's.
    #[arg(
        long,
        env = "LANRURUGI_TELEMETRY_REDIS_URL",
        default_value = "redis://127.0.0.1:6379/0"
    )]
    pub redis_url: String,

    /// Days a per-day key is kept before Redis drops it.
    #[arg(
        long,
        env = "LANRURUGI_TELEMETRY_RETENTION_DAYS",
        default_value_t = DEFAULT_RETENTION_DAYS
    )]
    pub retention_days: u64,

    /// Enables the read side: the dashboard at `/`, `/stats` (JSON) and `/api/series`. Callers
    /// either log in at `/` with this value as the password (which sets a session cookie) or send
    /// `Authorization: Bearer <token>`. Unset — the default — means the read side does not exist.
    #[arg(long, env = "LANRURUGI_TELEMETRY_STATS_TOKEN")]
    pub stats_token: Option<String>,

    /// The built dashboard (`telemetry/ui/dist`). Served as static files rather than embedded, so
    /// this binary still compiles without a Node build; when the directory is absent, `/` 404s and
    /// the JSON endpoints keep working.
    #[arg(
        long,
        env = "LANRURUGI_TELEMETRY_UI_DIR",
        default_value = "/usr/local/share/lanrurugi/telemetry"
    )]
    pub ui_dir: PathBuf,
}

struct Ingest {
    pool: Pool,
    retention_secs: i64,
    /// `None` disables the read side entirely (the login route 404s, everything else 401s).
    stats_token: Option<String>,
    ui_dir: PathBuf,
    /// Fixed-window per-IP counter. Bounded by clearing wholesale once it grows past `CLEAR_AT`
    /// (dropping the counts only loosens the limit for one window, never blocks anyone).
    hits: Mutex<HashMap<IpAddr, (u32, Instant)>>,
}

const CLEAR_AT: usize = 10_000;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Heartbeat {
    /// Random per-install id (UUIDv4). The only identifier this service ever stores.
    id: String,
    version: String,
    #[serde(default)]
    arch: Option<String>,
    #[serde(default)]
    os: Option<String>,
    /// Which published artifact this is (`ghcr`, `dockerhub`, `native`, `source`). Optional so a
    /// client from before this field existed is still accepted — old clients keep reporting.
    #[serde(default)]
    channel: Option<String>,
}

impl Heartbeat {
    fn validate(&self) -> Result<(), &'static str> {
        uuid::Uuid::parse_str(&self.id).map_err(|_| "id must be a UUID")?;
        if !valid_version(&self.version) {
            return Err("version must look like semver");
        }
        for (field, value) in [("arch", &self.arch), ("os", &self.os)] {
            if let Some(value) = value {
                if value.is_empty() || value.len() > 32 || !value.bytes().all(is_word_byte) {
                    return Err(if field == "arch" {
                        "arch must be a short identifier"
                    } else {
                        "os must be a short identifier"
                    });
                }
            }
        }
        if let Some(channel) = &self.channel {
            if channel.is_empty()
                || channel.len() > 16
                || !channel
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
            {
                return Err("channel must be a short lowercase identifier");
            }
        }
        Ok(())
    }
}

/// `0.2.0`, `1.0.0-rc.1`, and the `0.0.0-unknown` fallback a non-tagged build reports.
fn valid_version(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 32
        && value
            .bytes()
            .all(|b| is_word_byte(b) || b == b'.' || b == b'-' || b == b'+')
        && value.bytes().any(|b| b.is_ascii_digit())
}

fn is_word_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Cloudflare's own two-character country code — which is not always a pair of letters (`T1` marks
/// Tor, `XX` an address it cannot place). Anything else (a local `curl`, a header a client forged)
/// folds into `ZZ` so every write still has a country dimension.
fn country(headers: &HeaderMap) -> String {
    headers
        .get("cf-ipcountry")
        .and_then(|value| value.to_str().ok())
        .map(|value| value.trim().to_ascii_uppercase())
        .filter(|value| value.len() == 2 && value.bytes().all(|b| b.is_ascii_alphanumeric()))
        .unwrap_or_else(|| "ZZ".to_string())
}

fn allowed(app: &Ingest, peer: IpAddr) -> bool {
    let mut hits = app.hits.lock().unwrap_or_else(|e| e.into_inner());
    if hits.len() > CLEAR_AT {
        hits.clear();
    }
    let now = Instant::now();
    let entry = hits.entry(peer).or_insert((0, now));
    if now.duration_since(entry.1) >= Duration::from_secs(60) {
        *entry = (0, now);
    }
    entry.0 += 1;
    entry.0 <= MAX_PER_MINUTE
}

/// Who to bucket the rate limit by. The direct peer is `cloudflared`/nginx — one address for every
/// client on earth — so `CF-Connecting-IP` (overwritten by Cloudflare at the edge, never by the
/// client) is what actually identifies a caller. Falling back to the peer keeps a LAN `curl` from
/// sharing a bucket with everyone else.
fn client_ip(headers: &HeaderMap, peer: IpAddr) -> IpAddr {
    headers
        .get("cf-connecting-ip")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.trim().parse().ok())
        .unwrap_or(peer)
}

async fn heartbeat(
    State(app): State<Arc<Ingest>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Heartbeat>,
) -> Response {
    if !allowed(&app, client_ip(&headers, peer.ip())) {
        return (StatusCode::TOO_MANY_REQUESTS, "slow down").into_response();
    }
    if let Err(reason) = body.validate() {
        return (StatusCode::BAD_REQUEST, reason).into_response();
    }
    let country = country(&headers);
    let day = chrono::Utc::now().format("%Y-%m-%d").to_string();
    match record(&app, &body, &country, &day).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(error) => {
            tracing::warn!(%error, "telemetry-ingest: redis write failed");
            (StatusCode::SERVICE_UNAVAILABLE, "storage unavailable").into_response()
        }
    }
}

/// Three idempotent writes, so a retry from the same installation on the same day is free: the
/// day set answers "how many are there", the per-country HLL "where", the per-version set "on
/// what". SADD/PFADD both ignore duplicates.
async fn record(
    app: &Ingest,
    body: &Heartbeat,
    country: &str,
    day: &str,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let mut conn = app
        .pool
        .get()
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let ttl = app.retention_secs;

    let day_key = format!("LANRURUGI_TELEMETRY:DAY:{day}");
    let _: () = conn.sadd(&day_key, &body.id).await?;
    let _: () = conn.expire(&day_key, ttl).await?;

    let geo_key = format!("LANRURUGI_TELEMETRY:GEO:{day}:{country}");
    let _: () = conn.pfadd(&geo_key, &body.id).await?;
    let _: () = conn.expire(&geo_key, ttl).await?;

    let version_key = format!("LANRURUGI_TELEMETRY:VER:{day}:{}", body.version);
    let _: () = conn.sadd(&version_key, &body.id).await?;
    let _: () = conn.expire(&version_key, ttl).await?;

    // Which artifact the deployment came from. Nearly free (a handful of distinct channels) and
    // the only way to tell a GHCR pull from a Docker Hub one, since the bytes are identical.
    if let Some(channel) = &body.channel {
        let channel_key = format!("LANRURUGI_TELEMETRY:CHAN:{day}:{channel}");
        let _: () = conn.sadd(&channel_key, &body.id).await?;
        let _: () = conn.expire(&channel_key, ttl).await?;
        let _: () = conn
            .sadd("LANRURUGI_TELEMETRY:VALUES:CHAN", channel)
            .await?;
    }

    // The registry of seen values, so the read side never has to `SCAN` (or `KEYS`) the keyspace.
    // Bounded by construction: one entry per release / channel / country ever seen.
    let _: () = conn
        .sadd("LANRURUGI_TELEMETRY:VALUES:VER", &body.version)
        .await?;
    let _: () = conn.sadd("LANRURUGI_TELEMETRY:VALUES:GEO", country).await?;

    Ok(())
}

/// One row of a breakdown, in the shape the JSON response serialises.
#[derive(Debug, Clone, serde::Serialize)]
struct Entry {
    name: String,
    count: u64,
}

#[derive(Debug, Clone, serde::Serialize)]
struct Stats {
    generated_at: String,
    dau: u64,
    wau: u64,
    mau: u64,
    countries: Vec<Entry>,
    versions: Vec<Entry>,
    channels: Vec<Entry>,
}

const WAU_DAYS: i64 = 7;
const MAU_DAYS: i64 = 30;
/// Caps each breakdown so one pathological value cannot make the page (or the query) enormous.
const MAX_ENTRIES: usize = 20;

fn day(offset: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(offset))
        .format("%Y-%m-%d")
        .to_string()
}

/// Counts distinct installations across `days` days ending today. `SUNIONSTORE` into a scratch key
/// is O(total members) — fine for the 30-day window the dashboard asks for, and the reason the
/// page never aggregates the whole 400-day retention.
async fn union_count(
    conn: &mut deadpool_redis::Connection,
    keys: &[String],
) -> Result<u64, Box<dyn std::error::Error + Send + Sync>> {
    if keys.is_empty() {
        return Ok(0);
    }
    let scratch = format!("LANRURUGI_TELEMETRY:SCRATCH:{}", uuid::Uuid::new_v4());
    let _: () = conn.sunionstore(&scratch, keys).await?;
    let count: u64 = conn.scard(&scratch).await?;
    let _: () = conn.del(&scratch).await?;
    Ok(count)
}

async fn day_keys(kind: &str, days: i64) -> Vec<String> {
    (0..days)
        .map(|offset| format!("LANRURUGI_TELEMETRY:{kind}:{}", day(offset)))
        .collect()
}

/// Every value this dimension has ever recorded.
async fn known_values(
    conn: &mut deadpool_redis::Connection,
    dimension: &str,
) -> Result<Vec<String>, Box<dyn std::error::Error + Send + Sync>> {
    let key = format!("LANRURUGI_TELEMETRY:VALUES:{dimension}");
    let values: Vec<String> = conn.smembers(&key).await?;
    Ok(values)
}

/// One breakdown over the last 30 days: for each known value, how many distinct installations
/// reported it. `exact` picks between a Set union and a HyperLogLog merge — the country dimension
/// is HLL by construction, the other two are exact sets.
async fn breakdown(
    conn: &mut deadpool_redis::Connection,
    dimension: &str,
    exact: bool,
) -> Result<Vec<Entry>, Box<dyn std::error::Error + Send + Sync>> {
    let mut entries = Vec::new();
    for value in known_values(conn, dimension).await? {
        let keys: Vec<String> = (0..MAU_DAYS)
            .map(|offset| format!("LANRURUGI_TELEMETRY:{dimension}:{}:{value}", day(offset)))
            .collect();
        let count = if exact {
            union_count(conn, &keys).await?
        } else {
            conn.pfcount(keys).await?
        };
        if count > 0 {
            entries.push(Entry { name: value, count });
        }
    }
    entries.sort_by(|a, b| b.count.cmp(&a.count).then_with(|| a.name.cmp(&b.name)));
    entries.truncate(MAX_ENTRIES);
    Ok(entries)
}

async fn collect(app: &Ingest) -> Result<Stats, Box<dyn std::error::Error + Send + Sync>> {
    let mut conn = app
        .pool
        .get()
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;

    let dau: u64 = conn
        .scard(format!("LANRURUGI_TELEMETRY:DAY:{}", day(0)))
        .await?;
    let wau = union_count(&mut conn, &day_keys("DAY", WAU_DAYS).await).await?;
    let mau = union_count(&mut conn, &day_keys("DAY", MAU_DAYS).await).await?;

    Ok(Stats {
        generated_at: chrono::Utc::now().to_rfc3339(),
        dau,
        wau,
        mau,
        countries: breakdown(&mut conn, "GEO", false).await?,
        versions: breakdown(&mut conn, "VER", true).await?,
        channels: breakdown(&mut conn, "CHAN", true).await?,
    })
}

/// Constant-time compare: a wrong secret must not be findable byte by byte.
fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

/// Bearer token, for scripts and monitoring.
fn bearer_ok(app: &Ingest, headers: &HeaderMap) -> bool {
    let Some(expected) = app.stats_token.as_deref() else {
        return false;
    };
    headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(|token| constant_time_eq(token.trim().as_bytes(), expected.as_bytes()))
        .unwrap_or(false)
}

const SESSION_COOKIE: &str = "hb_session";
const SESSION_TTL_SECS: i64 = 7 * 86_400;

/// `<expiry>.<mac>`, where the mac is a keyed hash of the expiry keyed by the access secret itself
/// — no second secret to manage, and a cookie cannot outlive or outrun the configured token.
fn session_value(secret: &str, expires: i64) -> String {
    let key = blake3::hash(secret.as_bytes());
    let mac = blake3::keyed_hash(key.as_bytes(), expires.to_string().as_bytes());
    format!("{expires}.{}", mac.to_hex())
}

fn session_valid(secret: &str, raw: &str) -> bool {
    let Some((expires, _)) = raw.split_once('.') else {
        return false;
    };
    let Ok(expires) = expires.parse::<i64>() else {
        return false;
    };
    expires > chrono::Utc::now().timestamp()
        && constant_time_eq(session_value(secret, expires).as_bytes(), raw.as_bytes())
}

fn session_cookie(headers: &HeaderMap) -> Option<&str> {
    headers
        .get(axum::http::header::COOKIE)?
        .to_str()
        .ok()?
        .split(';')
        .filter_map(|part| part.trim().split_once('='))
        .find(|(name, _)| *name == SESSION_COOKIE)
        .map(|(_, value)| value)
}

/// Either credential opens the read side: a session cookie from the login form, or the bearer token
/// for anything scripted.
fn read_authorized(app: &Ingest, headers: &HeaderMap) -> bool {
    if bearer_ok(app, headers) {
        return true;
    }
    match (app.stats_token.as_deref(), session_cookie(headers)) {
        (Some(secret), Some(raw)) => session_valid(secret, raw),
        _ => false,
    }
}

fn unauthorized() -> Response {
    (StatusCode::UNAUTHORIZED, "unauthorized").into_response()
}

#[derive(Debug, Deserialize)]
struct LoginBody {
    password: String,
}

async fn login(
    State(app): State<Arc<Ingest>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<LoginBody>,
) -> Response {
    let Some(secret) = app.stats_token.clone() else {
        // No token configured means the read side does not exist, so there is nothing to log into.
        return StatusCode::NOT_FOUND.into_response();
    };
    if !allowed(&app, client_ip(&headers, peer.ip())) {
        return (StatusCode::TOO_MANY_REQUESTS, "slow down").into_response();
    }
    if !constant_time_eq(body.password.trim().as_bytes(), secret.as_bytes()) {
        return (StatusCode::UNAUTHORIZED, "wrong password").into_response();
    }
    let expires = chrono::Utc::now().timestamp() + SESSION_TTL_SECS;
    let cookie = format!(
        "{SESSION_COOKIE}={}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age={SESSION_TTL_SECS}",
        session_value(&secret, expires)
    );
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        axum::http::header::SET_COOKIE,
        axum::http::HeaderValue::from_str(&cookie).expect("cookie is ASCII"),
    );
    response
}

async fn logout() -> Response {
    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        axum::http::header::SET_COOKIE,
        axum::http::HeaderValue::from_static(
            "hb_session=; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=0",
        ),
    );
    response
}

async fn session_info(State(app): State<Arc<Ingest>>, headers: HeaderMap) -> Response {
    axum::Json(serde_json::json!({ "authenticated": read_authorized(&app, &headers) }))
        .into_response()
}

async fn stats_json(State(app): State<Arc<Ingest>>, headers: HeaderMap) -> Response {
    if !read_authorized(&app, &headers) {
        return unauthorized();
    }
    match collect(&app).await {
        Ok(stats) => {
            let mut response = axum::Json(stats).into_response();
            response.headers_mut().insert(
                axum::http::header::CACHE_CONTROL,
                axum::http::HeaderValue::from_static("no-store"),
            );
            response
        }
        Err(error) => {
            tracing::warn!(%error, "telemetry-ingest: stats query failed");
            (StatusCode::SERVICE_UNAVAILABLE, "stats unavailable").into_response()
        }
    }
}

#[derive(Debug, Deserialize)]
struct SeriesQuery {
    days: Option<i64>,
}

/// Daily distinct installations, newest last — the dashboard's chart. One `SCARD` per day rather
/// than a union, because "how many that day" is already the cardinality of that day's set.
async fn series(
    State(app): State<Arc<Ingest>>,
    headers: HeaderMap,
    Query(q): Query<SeriesQuery>,
) -> Response {
    if !read_authorized(&app, &headers) {
        return unauthorized();
    }
    let days = q.days.unwrap_or(30).clamp(1, 90);
    let mut conn = match app.pool.get().await {
        Ok(conn) => conn,
        Err(error) => {
            tracing::warn!(%error, "telemetry-ingest: series query failed");
            return (StatusCode::SERVICE_UNAVAILABLE, "stats unavailable").into_response();
        }
    };
    let mut points = Vec::with_capacity(days as usize);
    for offset in (0..days).rev() {
        let date = day(offset);
        let key = format!("LANRURUGI_TELEMETRY:DAY:{date}");
        match conn.scard::<_, u64>(&key).await {
            Ok(count) => points.push(serde_json::json!({ "date": date, "count": count })),
            Err(error) => {
                tracing::warn!(%error, "telemetry-ingest: series query failed");
                return (StatusCode::SERVICE_UNAVAILABLE, "stats unavailable").into_response();
            }
        }
    }
    axum::Json(serde_json::json!({ "days": points })).into_response()
}

async fn healthz() -> &'static str {
    "ok"
}

fn router(app: Arc<Ingest>) -> Router {
    // The built Svelte bundle (`telemetry/ui/dist`) is served from disk, not embedded: the binary
    // stays compilable without a Node build, and an image without the directory simply 404s `/`.
    let ui = ServeDir::new(app.ui_dir.clone()).append_index_html_on_directories(true);
    Router::new()
        .route("/v1/hb", post(heartbeat))
        .route("/healthz", get(healthz))
        .route("/stats", get(stats_json))
        .route("/api/series", get(series))
        .route("/api/login", post(login))
        .route("/api/logout", post(logout))
        .route("/api/session", get(session_info))
        .fallback_service(ui)
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(app)
}

pub async fn run(args: TelemetryIngestArgs) -> anyhow::Result<()> {
    let pool = Config::from_url(&args.redis_url).create_pool(Some(Runtime::Tokio1))?;
    let app = Arc::new(Ingest {
        pool,
        retention_secs: (args.retention_days as i64) * 86_400,
        stats_token: args
            .stats_token
            .as_deref()
            .map(str::trim)
            .filter(|token| !token.is_empty())
            .map(str::to_string),
        ui_dir: args.ui_dir.clone(),
        hits: Mutex::new(HashMap::new()),
    });
    let listener = tokio::net::TcpListener::bind(&args.bind).await?;
    tracing::info!(bind = %args.bind, retention_days = args.retention_days, "telemetry-ingest: listening");
    axum::serve(
        listener,
        router(app).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hb() -> Heartbeat {
        Heartbeat {
            id: "3f2a1c4e-0000-4000-8000-000000000000".to_string(),
            version: "0.2.0".to_string(),
            arch: Some("x86_64".to_string()),
            os: Some("linux".to_string()),
            channel: Some("ghcr".to_string()),
        }
    }

    #[test]
    fn accepts_a_well_formed_heartbeat() {
        assert!(hb().validate().is_ok());
    }

    #[test]
    fn a_client_that_predates_the_channel_field_is_still_accepted() {
        let mut body = hb();
        body.channel = None;
        assert!(body.validate().is_ok());
    }

    #[test]
    fn rejects_odd_channels() {
        for channel in ["", "GHCR", "a b", "x".repeat(17).as_str(), "ghcr;drop"] {
            let mut body = hb();
            body.channel = Some(channel.to_string());
            assert!(body.validate().is_err(), "{channel:?} must be rejected");
        }
    }

    #[test]
    fn rejects_a_non_uuid_id() {
        let mut body = hb();
        body.id = "not-a-uuid".to_string();
        assert!(body.validate().is_err());
    }

    #[test]
    fn rejects_odd_versions() {
        for version in ["", "abc", "0.2.0; DROP", &"9".repeat(40)] {
            let mut body = hb();
            body.version = version.to_string();
            assert!(body.validate().is_err(), "{version:?} must be rejected");
        }
        for version in ["0.2.0", "1.0.0-rc.1", "0.0.0-unknown"] {
            let mut body = hb();
            body.version = version.to_string();
            assert!(body.validate().is_ok(), "{version:?} must be accepted");
        }
    }

    #[test]
    fn country_falls_back_to_zz() {
        let mut headers = HeaderMap::new();
        assert_eq!(country(&headers), "ZZ");
        headers.insert("cf-ipcountry", "jp".parse().unwrap());
        assert_eq!(country(&headers), "JP");
        headers.insert("cf-ipcountry", "T1".parse().unwrap());
        assert_eq!(country(&headers), "T1");
        headers.insert("cf-ipcountry", "JAPAN".parse().unwrap());
        assert_eq!(country(&headers), "ZZ");
    }

    #[test]
    fn rate_limit_is_per_ip_and_per_window() {
        let app = Ingest {
            pool: Config::from_url("redis://127.0.0.1:6379/0")
                .create_pool(Some(Runtime::Tokio1))
                .expect("pool builds without connecting"),
            retention_secs: 86_400,
            stats_token: None,
            ui_dir: PathBuf::from("/nonexistent"),
            hits: Mutex::new(HashMap::new()),
        };
        let ip: IpAddr = "203.0.113.1".parse().unwrap();
        for _ in 0..MAX_PER_MINUTE {
            assert!(allowed(&app, ip));
        }
        assert!(!allowed(&app, ip));
        assert!(allowed(&app, "203.0.113.2".parse().unwrap()));
    }

    #[test]
    fn rate_limit_buckets_by_the_forwarded_client_not_the_proxy() {
        let proxy: IpAddr = "172.18.0.5".parse().unwrap();
        let mut headers = HeaderMap::new();
        assert_eq!(client_ip(&headers, proxy), proxy);

        headers.insert("cf-connecting-ip", "198.51.100.7".parse().unwrap());
        let forwarded: IpAddr = "198.51.100.7".parse().unwrap();
        assert_eq!(client_ip(&headers, proxy), forwarded);

        // A value that is not an address must not turn into one.
        headers.insert("cf-connecting-ip", "not-an-ip".parse().unwrap());
        assert_eq!(client_ip(&headers, proxy), proxy);
    }

    fn app_with_token(token: Option<&str>) -> Ingest {
        Ingest {
            pool: Config::from_url("redis://127.0.0.1:6379/0")
                .create_pool(Some(Runtime::Tokio1))
                .expect("pool builds without connecting"),
            retention_secs: 86_400,
            stats_token: token.map(str::to_string),
            ui_dir: PathBuf::from("/nonexistent"),
            hits: Mutex::new(HashMap::new()),
        }
    }

    #[test]
    fn the_read_side_needs_the_exact_token() {
        let app = app_with_token(Some("s3cret"));

        let mut correct = HeaderMap::new();
        correct.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer s3cret".parse().unwrap(),
        );
        assert!(read_authorized(&app, &correct));

        let mut wrong = HeaderMap::new();
        wrong.insert(
            axum::http::header::AUTHORIZATION,
            "Bearer s3cres".parse().unwrap(),
        );
        assert!(!read_authorized(&app, &wrong));

        // No header at all, and no token configured: both refuse.
        assert!(!read_authorized(&app, &HeaderMap::new()));
        assert!(!read_authorized(&app_with_token(None), &correct));
    }

    #[test]
    fn a_session_cookie_is_scoped_to_the_configured_secret_and_expiry() {
        let secret = "s3cret";
        let future = chrono::Utc::now().timestamp() + 60;

        let value = session_value(secret, future);
        assert!(session_valid(secret, &value));

        // A cookie minted for another secret, or one that has expired, must not open anything.
        assert!(!session_valid("other-secret", &value));
        assert!(!session_valid(secret, &session_value(secret, 1)));
        assert!(!session_valid(secret, "garbage"));
        assert!(!session_valid(secret, "notanumber.deadbeef"));
    }

    #[test]
    fn a_session_cookie_opens_the_read_side_without_a_bearer() {
        let app = app_with_token(Some("s3cret"));
        let expires = chrono::Utc::now().timestamp() + 60;
        let cookie = format!("{SESSION_COOKIE}={}", session_value("s3cret", expires));

        let mut headers = HeaderMap::new();
        headers.insert(axum::http::header::COOKIE, cookie.parse().unwrap());
        assert!(read_authorized(&app, &headers));

        // A cookie that was not signed by this instance's secret is refused.
        let mut forged = HeaderMap::new();
        forged.insert(
            axum::http::header::COOKIE,
            format!("{SESSION_COOKIE}={}", session_value("other", expires))
                .parse()
                .unwrap(),
        );
        assert!(!read_authorized(&app, &forged));
    }
}
