//! Local model usage and load state.
//!
//! Two different things are reported here, and conflating them would be misleading. `usage` is
//! today's per-feature call counters, which `lanrurugi_llm::usage::record` already writes to Redis
//! day hashes. `in_process` is the state of the models loaded *inside* this process — their memory
//! is part of `server.rss_bytes` and is deliberately not added on top of it; `note` says so, because
//! a table that simply lists them invites exactly that double count.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use lanrurugi_llm::FeatureId;

use crate::resources::model::{InProcessModel, ModelUsage};
use crate::AppState;

/// Redis is queried at most this often; the counters move at the speed of model calls, not of the
/// sampler's 1 Hz tick.
const USAGE_CACHE_TTL: Duration = Duration::from_secs(5);

static USAGE_CACHE: Mutex<Option<(Instant, Vec<ModelUsage>)>> = Mutex::new(None);

pub const NOTE: &str = "Process-local model memory (embedding, OCR, translation, inpainting) is \
included in server.rss_bytes; these rows are not additional memory.";

/// Today's (UTC) per-feature usage, summed over whatever fields a call actually recorded. Failure
/// counters are folded into one `failures` number — the panel exists to show load, and
/// `GET /api/llm/usage` is where the per-kind breakdown lives.
pub async fn usage(state: &AppState) -> Vec<ModelUsage> {
    {
        let cache = USAGE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, rows)) = cache.as_ref()
            && at.elapsed() < USAGE_CACHE_TTL
        {
            return rows.clone();
        }
    }

    let date = chrono::Utc::now().format("%Y-%m-%d").to_string();
    let mut pipe = deadpool_redis::redis::pipe();
    for feature in FeatureId::ALL {
        pipe.hgetall(lanrurugi_storage::keys::llm_usage_daily_key(
            feature.as_str(),
            &date,
        ));
    }
    let fetched: Result<Vec<HashMap<String, u64>>, String> = async {
        let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
        pipe.query_async(&mut conn).await.map_err(|e| e.to_string())
    }
    .await;

    let rows = match fetched {
        Ok(hashes) => FeatureId::ALL
            .iter()
            .zip(hashes)
            .filter_map(|(feature, fields)| row(feature, &fields))
            .collect::<Vec<_>>(),
        // A Redis hiccup must not hide the rest of the snapshot; an empty list reads as "no calls
        // recorded", which is also what a quiet day looks like.
        Err(_) => Vec::new(),
    };

    let mut cache = USAGE_CACHE.lock().unwrap_or_else(|e| e.into_inner());
    *cache = Some((Instant::now(), rows.clone()));
    rows
}

fn row(feature: &FeatureId, fields: &HashMap<String, u64>) -> Option<ModelUsage> {
    let requests = fields.get("requests").copied().unwrap_or(0);
    let local_cache_hits = fields.get("local_cache_hits").copied().unwrap_or(0);
    if requests == 0 && local_cache_hits == 0 {
        return None;
    }
    let failures = fields
        .iter()
        .filter(|(key, _)| key.starts_with("fail_"))
        .map(|(_, value)| *value)
        .sum();
    Some(ModelUsage {
        feature: feature.as_str().to_string(),
        requests,
        failures,
        elapsed_ms_total: fields.get("elapsed_ms").copied().unwrap_or(0),
        tokens_in: fields.get("prompt_tokens").copied().unwrap_or(0),
        tokens_out: fields.get("completion_tokens").copied().unwrap_or(0),
    })
}

/// The four model families this process can load. `loaded` is `unknown` rather than a guess: none of
/// the in-process clients expose a cheap "is the model resident" bit, and inventing one would put a
/// fabricated number in front of the user (spec FR-003).
pub fn in_process() -> Vec<InProcessModel> {
    ["embedding", "ocr", "translation", "inpainting"]
        .into_iter()
        .map(|name| InProcessModel {
            name: name.to_string(),
            loaded: "unknown".to_string(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fields(pairs: &[(&str, u64)]) -> HashMap<String, u64> {
        pairs
            .iter()
            .map(|(key, value)| ((*key).to_string(), *value))
            .collect()
    }

    #[test]
    fn a_feature_without_calls_is_omitted_rather_than_reported_as_zero() {
        assert!(row(&FeatureId::SUBFOLDER_TANKOUBON_NAMING, &fields(&[])).is_none());
        assert!(row(
            &FeatureId::SUBFOLDER_TANKOUBON_NAMING,
            &fields(&[("requests", 0), ("local_cache_hits", 0)])
        )
        .is_none());
    }

    #[test]
    fn a_feature_with_calls_reports_every_failure_kind_as_one_number() {
        let usage = row(
            &FeatureId::SUBFOLDER_TANKOUBON_NAMING,
            &fields(&[
                ("requests", 7),
                ("elapsed_ms", 1234),
                ("prompt_tokens", 100),
                ("completion_tokens", 20),
                ("fail_transient", 2),
                ("fail_timeout", 1),
            ]),
        )
        .expect("has calls");
        assert_eq!(usage.requests, 7);
        assert_eq!(usage.failures, 3);
        assert_eq!(usage.tokens_in, 100);
        assert_eq!(usage.tokens_out, 20);
        assert_eq!(usage.elapsed_ms_total, 1234);
    }

    #[test]
    fn a_cache_hit_only_row_still_appears() {
        let usage = row(
            &FeatureId::SUBFOLDER_TANKOUBON_NAMING,
            &fields(&[("local_cache_hits", 4)]),
        )
        .expect("cache hits are activity");
        assert_eq!(usage.requests, 0);
    }

    #[test]
    fn the_note_names_the_double_count_it_prevents() {
        assert!(NOTE.contains("server.rss_bytes"));
        assert_eq!(in_process().len(), 4);
    }
}
