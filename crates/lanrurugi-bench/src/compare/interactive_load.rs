//! Search/browse responsiveness benchmark (T088, SC-008): repeatedly hits the endpoints a user's
//! browser actually calls while browsing/searching/tag-filtering — `GET /archives` (the full
//! library listing; legacy has no server-side pagination on this path, verified against
//! `tools/openapi.yaml`, so this *is* the realistic "browsing" hot path at scale) and
//! `GET /search?filter=...` (title-fuzzy-match, `lanrurugi-search::engine`'s `LRR_TITLES` scan) —
//! and reports latency percentiles, rather than only measuring bulk-operation throughput like the
//! `compare` module's two named operations do.

use std::time::{Duration, Instant};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LatencyStats {
    pub p50_ms: f64,
    pub p95_ms: f64,
    pub p99_ms: f64,
    pub max_ms: f64,
}

impl LatencyStats {
    fn from_samples(mut samples: Vec<Duration>) -> Self {
        samples.sort();
        let percentile = |p: f64| -> f64 {
            if samples.is_empty() {
                return 0.0;
            }
            let idx = ((samples.len() - 1) as f64 * p).round() as usize;
            samples[idx].as_secs_f64() * 1000.0
        };
        LatencyStats {
            p50_ms: percentile(0.50),
            p95_ms: percentile(0.95),
            p99_ms: percentile(0.99),
            max_ms: samples
                .last()
                .map(|d| d.as_secs_f64() * 1000.0)
                .unwrap_or(0.0),
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct InteractiveLoadResult {
    /// `GET /archives` — the full-library listing a browsing client fetches.
    pub archive_list: LatencyStats,
    /// `GET /search?filter=<substring present in most titles>` — the common "matches almost
    /// everything" search case.
    pub title_search_match: LatencyStats,
    /// `GET /search?filter=<substring present in no title>` — the worst-case "scan everything,
    /// find nothing" search case.
    pub title_search_no_match: LatencyStats,
}

async fn time_requests(
    client: &reqwest::Client,
    url: &str,
    iterations: usize,
) -> anyhow::Result<LatencyStats> {
    let mut samples = Vec::with_capacity(iterations);
    for _ in 0..iterations {
        let start = Instant::now();
        let response = client.get(url).send().await?;
        response.error_for_status_ref()?;
        // Fully drain the body — a real client parses/renders the payload, so time-to-first-byte
        // alone would understate the cost that actually determines perceived responsiveness.
        let _ = response.bytes().await?;
        samples.push(start.elapsed());
    }
    Ok(LatencyStats::from_samples(samples))
}

/// Runs `iterations` requests against each of the three endpoints and returns latency
/// percentiles. `title_needle` should be a substring present in (most of) the synthetic library's
/// titles (e.g. `"Synthetic"`), so `title_search_match` exercises a realistic high-match-rate
/// query.
pub async fn run(
    client: &reqwest::Client,
    base_url: &str,
    title_needle: &str,
    iterations: usize,
) -> anyhow::Result<InteractiveLoadResult> {
    let archive_list =
        time_requests(client, &format!("{base_url}/api/archives"), iterations).await?;
    let title_search_match = time_requests(
        client,
        &format!("{base_url}/api/search?filter={title_needle}"),
        iterations,
    )
    .await?;
    let title_search_no_match = time_requests(
        client,
        &format!("{base_url}/api/search?filter=zzz_no_such_archive_title_zzz"),
        iterations,
    )
    .await?;

    Ok(InteractiveLoadResult {
        archive_list,
        title_search_match,
        title_search_no_match,
    })
}
