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
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, DefaultBodyLimit, State};
use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use deadpool_redis::redis::AsyncCommands;
use deadpool_redis::{Config, Pool, Runtime};
use serde::Deserialize;

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
}

struct Ingest {
    pool: Pool,
    retention_secs: i64,
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

async fn heartbeat(
    State(app): State<Arc<Ingest>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    headers: HeaderMap,
    Json(body): Json<Heartbeat>,
) -> Response {
    if !allowed(&app, peer.ip()) {
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

    Ok(())
}

async fn healthz() -> &'static str {
    "ok"
}

fn router(app: Arc<Ingest>) -> Router {
    Router::new()
        .route("/v1/hb", post(heartbeat))
        .route("/healthz", get(healthz))
        .fallback(|| async { StatusCode::NOT_FOUND })
        .layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(app)
}

pub async fn run(args: TelemetryIngestArgs) -> anyhow::Result<()> {
    let pool = Config::from_url(&args.redis_url).create_pool(Some(Runtime::Tokio1))?;
    let app = Arc::new(Ingest {
        pool,
        retention_secs: (args.retention_days as i64) * 86_400,
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
        }
    }

    #[test]
    fn accepts_a_well_formed_heartbeat() {
        assert!(hb().validate().is_ok());
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
            hits: Mutex::new(HashMap::new()),
        };
        let ip: IpAddr = "203.0.113.1".parse().unwrap();
        for _ in 0..MAX_PER_MINUTE {
            assert!(allowed(&app, ip));
        }
        assert!(!allowed(&app, ip));
        assert!(allowed(&app, "203.0.113.2".parse().unwrap()));
    }
}
