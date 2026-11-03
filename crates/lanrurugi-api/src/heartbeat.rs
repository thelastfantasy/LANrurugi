//! Anonymous startup heartbeat, one request per installation per day (opt-out).
//!
//! This is *not* diagnostics: `diagnostics` captures errors locally and only ever leaves the
//! machine through an administrator-opened forwarding window. The heartbeat is the opposite — it
//! exists so the project can answer "how many deployments are out there, in which countries", and
//! carries nothing about them beyond a random install id, the version and the architecture.
//!
//! **On by default in every release build** ([`DEFAULT_ENDPOINT`]), because the requirement is to
//! count installations across all packaging channels at once — container and native alike — and
//! any per-packaging default (a Dockerfile `ENV`, a package script) would cover one channel and
//! silently miss the rest. A source build that does not want this uses `--no-telemetry`, and a
//! debug build never sends at all. The country is derived by the sink from the connection itself;
//! no address is ever sent.
//!
//! Startup must never wait on this: the send is a detached task with a short timeout whose result
//! is discarded.

use std::time::Duration;

use deadpool_redis::redis::AsyncCommands;
use lanrurugi_storage::redis::RedisDbs;
use serde_json::json;

/// Random per-install id, created on first run and kept for the installation's lifetime.
const INSTANCE_ID_KEY: &str = "LANRURUGI_INSTANCE_ID";
/// UTC day of the last successful send (`YYYY-MM-DD`), so a restart does not send twice.
const LAST_SENT_KEY: &str = "LANRURUGI_HEARTBEAT_LAST";
/// Kept a few days past the send it records, purely so a clock-skewed restart still sees it.
const LAST_SENT_TTL_SECS: u64 = 3 * 86_400;

/// The project's own sink, baked into the source rather than into one packaging.
///
/// The requirement is to count *every* installation, not only the container ones — so the default
/// cannot live in a Dockerfile `ENV` or a packaging script, which would cover one channel each and
/// silently miss the others. A constant covers every release build from this tree at once: the
/// image, the native packages, `cargo install`, a distro build.
const DEFAULT_ENDPOINT: &str = "https://hb.lrrg.dev/v1/hb";

/// Why this build may not send. Both are deploy-time, like `--disable-update-check`.
#[derive(Debug, Clone, Default)]
pub struct Options {
    /// The sink's endpoint. Without one, nothing is sent and no id is created.
    pub url: Option<String>,
    /// `--no-telemetry` / `LANRURUGI_NO_TELEMETRY`: an operator's explicit refusal.
    pub disabled: bool,
}

impl Options {
    fn active(&self) -> Option<&str> {
        if self.disabled {
            return None;
        }
        if let Some(url) = self
            .url
            .as_deref()
            .map(str::trim)
            .filter(|url| url.starts_with("https://") || url.starts_with("http://"))
        {
            return Some(url);
        }
        default_endpoint()
    }
}

/// `None` in a debug build: `cargo run`, tests and the dev container are not installations, and
/// their heartbeats would pollute the very numbers this exists to report.
fn default_endpoint() -> Option<&'static str> {
    if cfg!(debug_assertions) {
        None
    } else {
        Some(DEFAULT_ENDPOINT)
    }
}

/// Fire and forget. Call once, after Redis is up, from `serve`.
pub fn spawn(redis: RedisDbs, options: Options) {
    let Some(url) = options.active().map(str::to_string) else {
        return;
    };
    tokio::spawn(async move {
        if let Err(error) = send_once(&redis, &url).await {
            // Debug, not warn: an unreachable sink is an ordinary condition for a self-hosted
            // instance behind a restrictive network, not something an operator needs to fix.
            tracing::debug!(%error, "heartbeat: not sent");
        }
    });
}

/// One startup line so the behaviour is visible in `docker logs` rather than a surprise found in
/// the README: on by default in the published image, one anonymous request a day, and here is the
/// switch. Logged by `serve`, which is the only mode that ever sends.
pub fn log_status(options: &Options) {
    match options.active() {
        Some(endpoint) => tracing::info!(
            %endpoint,
            "telemetry: one anonymous heartbeat per day (version/arch only) — disable with --no-telemetry"
        ),
        None if options.disabled => tracing::info!("telemetry: disabled by --no-telemetry"),
        None => tracing::debug!("telemetry: no endpoint configured, nothing is sent"),
    }
}

pub(crate) fn payload(instance_id: &str) -> serde_json::Value {
    json!({
        "id": instance_id,
        "version": crate::version::current_version(),
        "arch": std::env::consts::ARCH,
        "os": std::env::consts::OS,
        // Which published artifact this is (`ghcr` / `dockerhub` / `native` / `source`), stamped
        // in at build time — the only place that information exists at all.
        "channel": crate::version::build_channel(),
    })
}

async fn send_once(redis: &RedisDbs, url: &str) -> Result<(), Box<dyn std::error::Error>> {
    let mut conn = redis.config.get().await?;

    // `SET NX` then read back: two first-run processes racing produce one id, not two.
    let existing: Option<String> = conn.get(INSTANCE_ID_KEY).await?;
    let instance_id = match existing {
        Some(id) => id,
        None => {
            let candidate = uuid::Uuid::new_v4().to_string();
            let won: bool = conn.set_nx(INSTANCE_ID_KEY, &candidate).await?;
            if won {
                candidate
            } else {
                let stored: Option<String> = conn.get(INSTANCE_ID_KEY).await?;
                stored.unwrap_or(candidate)
            }
        }
    };

    let today = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let last: Option<String> = conn.get(LAST_SENT_KEY).await?;
    if last.as_deref() == Some(today.as_str()) {
        return Ok(());
    }

    let client = reqwest::Client::builder()
        // Three seconds, because this runs beside a user's startup and a sink that cannot answer
        // in that window is not worth waiting for.
        .connect_timeout(Duration::from_secs(2))
        .timeout(Duration::from_secs(3))
        .build()
        .unwrap_or_else(|_| lanrurugi_core::http_client::default_client());

    let response = client.post(url).json(&payload(&instance_id)).send().await?;
    if !response.status().is_success() {
        return Err(format!("sink answered {}", response.status()).into());
    }

    let _: () = conn
        .set_ex(LAST_SENT_KEY, &today, LAST_SENT_TTL_SECS)
        .await?;
    tracing::info!(
        version = crate::version::current_version(),
        "heartbeat: sent"
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn debug_builds_report_nothing() {
        // `cargo test` is a debug build — the same path `cargo run` and the dev container take.
        // Counting them would inflate the very numbers this feature reports.
        assert!(default_endpoint().is_none());
        assert!(Options::default().active().is_none());
        // A stray empty env var (compose passthrough) must not count as a configured sink.
        let blank = Options {
            url: Some("  ".to_string()),
            disabled: false,
        };
        assert!(blank.active().is_none());
    }

    #[test]
    fn an_explicit_endpoint_reaches_a_debug_build_so_it_can_be_tested_locally() {
        let options = Options {
            url: Some("http://127.0.0.1:8099/v1/hb".to_string()),
            disabled: false,
        };
        assert_eq!(options.active(), Some("http://127.0.0.1:8099/v1/hb"));
    }

    #[test]
    fn the_refusal_flag_wins_over_a_configured_sink() {
        let options = Options {
            url: Some("https://hb.example.com/v1/hb".to_string()),
            disabled: true,
        };
        assert!(options.active().is_none());
    }

    #[test]
    fn payload_carries_no_library_or_user_data() {
        let value = payload("3f2a1c4e-0000-4000-8000-000000000000");
        let object = value.as_object().expect("object");
        let mut keys: Vec<&str> = object.keys().map(String::as_str).collect();
        keys.sort_unstable();
        assert_eq!(keys, ["arch", "channel", "id", "os", "version"]);
    }
}
