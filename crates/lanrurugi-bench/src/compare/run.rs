//! Cross-system comparison harness (T086, research.md #11): drives a running legacy LANraragi
//! instance and the new `lanrurugi-server` binary against the same synthetic library copy on the
//! same hardware, measuring wall-clock time for two operations (FR-020):
//!
//! - **`full_library_scan_ingestion`**: legacy — `POST /api/shinobu/rescan` against an instance
//!   whose filemap is empty (verified: `reset_filemap` deletes the filemap and forces a full
//!   rescan), polled via `GET /api/archives` until every archive is catalogued; new —
//!   `POST /database/rebuild-index` against an instance with an empty Redis archive DB, where
//!   `rekey_all` is a no-op (nothing tracked yet) and `full_scan` does all the discovery/catalogue
//!   work — functionally equivalent to a first-time ingestion.
//! - **`duplicate_repair_reindex`**: legacy has no dedicated repair feature (that gap is this
//!   rewrite's headline fix, US2/US6) — the closest available analog is running
//!   `shinobu/rescan` a *second* time against an already-fully-scanned library, which deletes the
//!   filemap and re-hashes every file (verified via container logs: unlike the first scan, this
//!   pass skips thumbnail/plugin work for IDs that already have an archive record, so it's
//!   considerably cheaper than the initial scan — not "recompute everything from scratch" in the
//!   expensive sense, just the hashing/filemap-repopulation part); new — a second
//!   `POST /database/rebuild-index` call against the now-populated instance, which is the actual
//!   US6 operation being measured (`rekey_all` re-verifies every tracked archive's hash in
//!   parallel, `full_scan` re-walks the filesystem finding nothing new). Both sides end up doing
//!   comparably little work per file on this pass, so expect this operation's speedup factor to
//!   sit much closer to 1.0 than `full_library_scan_ingestion`'s.

use std::time::{Duration, Instant};

use base64::engine::general_purpose::STANDARD as BASE64;
use base64::Engine;
use reqwest::Client;
use serde::Deserialize;

use super::interactive_load::{self, InteractiveLoadResult};
use super::report::{
    BenchmarkReport, Hardware, LibraryScale, OperationKind, OperationResult, SystemResult,
};

#[derive(Debug, Clone)]
pub struct SystemEndpoint {
    pub base_url: String,
    pub api_key: Option<String>,
}

impl SystemEndpoint {
    fn authorize(&self, builder: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(key) => builder.header("Authorization", format!("Bearer {}", BASE64.encode(key))),
            None => builder,
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url.trim_end_matches('/'))
    }
}

#[derive(Debug, Clone)]
pub struct CompareConfig {
    pub legacy: SystemEndpoint,
    pub new: SystemEndpoint,
    pub archive_count: u64,
    pub total_size_bytes: u64,
    pub hardware_description: String,
    pub poll_interval: Duration,
    /// Per-operation ceiling; a run that exceeds this is treated as a failure rather than
    /// blocking forever on a stuck legacy/new instance.
    pub operation_timeout: Duration,
    /// A substring present in (most of) the synthetic library's titles, for the interactive-load
    /// sub-benchmark's "high match rate" search case (T088).
    pub title_needle: String,
    pub interactive_load_iterations: usize,
}

#[derive(Debug, Deserialize)]
struct JobStatusResponse {
    state: String,
    error: Option<String>,
}

async fn poll_new_job_until_done(
    client: &Client,
    endpoint: &SystemEndpoint,
    job_id: &str,
    poll_interval: Duration,
    timeout: Duration,
) -> anyhow::Result<()> {
    let deadline = Instant::now() + timeout;
    loop {
        let status: JobStatusResponse = endpoint
            .authorize(client.get(endpoint.url(&format!("/api/minion/{job_id}"))))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        match status.state.as_str() {
            "finished" => return Ok(()),
            "failed" => anyhow::bail!("job {job_id} failed: {:?}", status.error),
            _ => {
                if Instant::now() >= deadline {
                    anyhow::bail!("timed out after {timeout:?} waiting for job {job_id}");
                }
                tokio::time::sleep(poll_interval).await;
            }
        }
    }
}

/// Measures the new system's `POST /database/rebuild-index` — used for both operations (see
/// module docs): the first call (empty index) times ingestion, the second (populated index)
/// times reindex.
async fn measure_new_rebuild_index(
    client: &Client,
    endpoint: &SystemEndpoint,
    config: &CompareConfig,
) -> anyhow::Result<Duration> {
    let start = Instant::now();
    let response: serde_json::Value = endpoint
        .authorize(client.post(endpoint.url("/api/database/rebuild-index")))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let job_id = response["job"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("rebuild-index response missing job id: {response}"))?;

    poll_new_job_until_done(
        client,
        endpoint,
        job_id,
        config.poll_interval,
        config.operation_timeout,
    )
    .await?;
    Ok(start.elapsed())
}

/// Measures a legacy `shinobu/rescan` pass: triggers it, then polls `GET /api/archives` until
/// every archive is catalogued (legacy exposes no job-id for this operation, unlike the new
/// system's Minion-shaped jobs — verified against `tools/openapi.yaml`'s `shinobu/rescan`
/// response shape, which only returns the watcher's new PID).
async fn measure_legacy_rescan(
    client: &Client,
    endpoint: &SystemEndpoint,
    config: &CompareConfig,
) -> anyhow::Result<Duration> {
    let start = Instant::now();
    endpoint
        .authorize(client.post(endpoint.url("/api/shinobu/rescan")))
        .send()
        .await?
        .error_for_status()?;

    let deadline = Instant::now() + config.operation_timeout;
    loop {
        let archives: Vec<serde_json::Value> = endpoint
            .authorize(client.get(endpoint.url("/api/archives")))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        if archives.len() as u64 >= config.archive_count {
            return Ok(start.elapsed());
        }
        if Instant::now() >= deadline {
            anyhow::bail!(
                "timed out after {:?} waiting for legacy rescan ({} of {} archives catalogued)",
                config.operation_timeout,
                archives.len(),
                config.archive_count
            );
        }
        tokio::time::sleep(config.poll_interval).await;
    }
}

pub async fn run_full_comparison(config: &CompareConfig) -> anyhow::Result<BenchmarkReport> {
    let client = Client::new();

    tracing::info!("Measuring legacy full_library_scan_ingestion (shinobu/rescan, pass 1)...");
    let legacy_full_scan = measure_legacy_rescan(&client, &config.legacy, config).await?;

    tracing::info!(
        "Measuring new-system full_library_scan_ingestion (rebuild-index on empty index)..."
    );
    let new_full_scan = measure_new_rebuild_index(&client, &config.new, config).await?;

    tracing::info!("Measuring legacy duplicate_repair_reindex analog (shinobu/rescan, pass 2)...");
    let legacy_reindex = measure_legacy_rescan(&client, &config.legacy, config).await?;

    tracing::info!("Measuring new-system duplicate_repair_reindex (rebuild-index, pass 2)...");
    let new_reindex = measure_new_rebuild_index(&client, &config.new, config).await?;

    let interactive_load: Option<InteractiveLoadResult> = match interactive_load::run(
        &client,
        config.new.base_url.trim_end_matches('/'),
        &config.title_needle,
        config.interactive_load_iterations,
    )
    .await
    {
        Ok(result) => Some(result),
        Err(e) => {
            tracing::warn!(error = %e, "interactive-load sub-benchmark failed; report will omit it");
            None
        }
    };

    let operations = vec![
        OperationResult::new(
            OperationKind::FullLibraryScanIngestion,
            SystemResult::new(legacy_full_scan, config.archive_count),
            SystemResult::new(new_full_scan, config.archive_count),
        ),
        OperationResult::new(
            OperationKind::DuplicateRepairReindex,
            SystemResult::new(legacy_reindex, config.archive_count),
            SystemResult::new(new_reindex, config.archive_count),
        ),
    ];

    let generated_at = time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .unwrap_or_default();

    Ok(BenchmarkReport {
        report_id: uuid::Uuid::new_v4().to_string(),
        generated_at,
        library_scale: LibraryScale {
            archive_count: config.archive_count,
            total_size_bytes: config.total_size_bytes,
        },
        hardware: Hardware {
            cpu_cores: std::thread::available_parallelism()
                .map(|n| n.get() as u32)
                .unwrap_or(1),
            description: config.hardware_description.clone(),
        },
        operations,
        interactive_load,
    })
}
