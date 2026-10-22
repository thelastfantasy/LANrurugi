//! `GET /api/llm/usage` (`specs/014-ai-harness-hardening/contracts/llm-usage-api.md`): per-feature
//! model usage over a day or a week, read from the day hashes `lanrurugi_llm::usage::record`
//! writes. Observes only — it never refuses a call.

use std::collections::HashMap;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use lanrurugi_llm::FeatureId;
use serde::Deserialize;
use serde_json::{json, Map, Value};

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new().route("/llm/usage", get(usage))
}

#[derive(Deserialize)]
struct UsageQuery {
    #[serde(default)]
    window: Option<String>,
}

/// The UTC dates a window covers, newest first.
fn window_dates(window: &str, today: chrono::NaiveDate) -> Option<Vec<String>> {
    let days = match window {
        "day" => 1,
        "week" => 7,
        _ => return None,
    };
    Some(
        (0..days)
            .map(|back| {
                (today - chrono::Duration::days(back))
                    .format("%Y-%m-%d")
                    .to_string()
            })
            .collect(),
    )
}

const TOKEN_FIELDS: [(&str, &str); 6] = [
    ("prompt_tokens", "prompt"),
    ("completion_tokens", "completion"),
    ("total_tokens", "total"),
    ("prompt_cache_hit_tokens", "cache_hit"),
    ("prompt_cache_miss_tokens", "cache_miss"),
    ("reasoning_tokens", "reasoning"),
];

const FAILURE_FIELDS: [(&str, &str); 7] = [
    ("fail_auth", "auth"),
    ("fail_balance", "balance"),
    ("fail_rate_limited", "rate_limited"),
    ("fail_transient", "transient"),
    ("fail_timeout", "timeout"),
    ("fail_malformed", "malformed"),
    ("fail_rejected", "rejected"),
];

/// One feature's row from its day hashes (missing days simply absent). `no_data` when no day had
/// a call; a token figure appears only when some call reported it.
fn feature_row(feature: FeatureId, days: &[HashMap<String, u64>]) -> Value {
    let mut sum: HashMap<&str, u64> = HashMap::new();
    let mut present: std::collections::HashSet<&str> = std::collections::HashSet::new();
    for day in days {
        for (field, value) in day {
            let key = field.as_str();
            let name = TOKEN_FIELDS
                .iter()
                .chain(&FAILURE_FIELDS)
                .map(|(f, _)| *f)
                .chain([
                    "requests",
                    "elapsed_ms",
                    "unreported_token_calls",
                    "cancelled",
                    "local_cache_hits",
                    "prefix_breaks",
                ])
                .find(|f| *f == key);
            if let Some(name) = name {
                *sum.entry(name).or_default() += value;
                present.insert(name);
            }
        }
    }
    let requests = sum.get("requests").copied().unwrap_or(0);
    let local_cache_hits = sum.get("local_cache_hits").copied().unwrap_or(0);
    if requests == 0 && local_cache_hits == 0 {
        return json!({ "feature": feature.as_str(), "status": "no_data" });
    }
    let mut tokens = Map::new();
    for (field, name) in TOKEN_FIELDS {
        if present.contains(field) {
            tokens.insert(name.into(), json!(sum[field]));
        }
    }
    tokens.insert(
        "unreported_calls".into(),
        json!(sum.get("unreported_token_calls").copied().unwrap_or(0)),
    );
    let failures: Map<String, Value> = FAILURE_FIELDS
        .iter()
        .map(|(field, name)| ((*name).into(), json!(sum.get(field).copied().unwrap_or(0))))
        .collect();
    json!({
        "feature": feature.as_str(),
        "status": "reported",
        "requests": requests,
        "local_cache_hits": local_cache_hits,
        "elapsed_ms": sum.get("elapsed_ms").copied().unwrap_or(0),
        "tokens": tokens,
        "cancelled": sum.get("cancelled").copied().unwrap_or(0),
        "prefix_breaks": sum.get("prefix_breaks").copied().unwrap_or(0),
        "failures": failures,
    })
}

async fn usage(State(state): State<AppState>, Query(query): Query<UsageQuery>) -> Response {
    let window = query.window.as_deref().unwrap_or("day");
    let Some(dates) = window_dates(window, chrono::Utc::now().date_naive()) else {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "error": "invalid_window", "detail": "window must be day or week" })),
        )
            .into_response();
    };

    let mut pipe = deadpool_redis::redis::pipe();
    for feature in FeatureId::ALL {
        for date in &dates {
            pipe.hgetall(lanrurugi_storage::keys::llm_usage_daily_key(
                feature.as_str(),
                date,
            ));
        }
    }
    let fetched: Result<Vec<HashMap<String, u64>>, String> = async {
        let mut conn = state.redis.config.get().await.map_err(|e| e.to_string())?;
        pipe.query_async(&mut conn).await.map_err(|e| e.to_string())
    }
    .await;
    let Ok(hashes) = fetched else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({ "error": "llm_usage_unavailable" })),
        )
            .into_response();
    };

    let features: Vec<Value> = FeatureId::ALL
        .iter()
        .zip(hashes.chunks(dates.len()))
        .map(|(feature, days)| feature_row(*feature, days))
        .collect();
    Json(json!({ "window": window, "features": features })).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(fields: &[(&str, u64)]) -> HashMap<String, u64> {
        fields.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    /// Quickstart S3: a recorded call is readable from the endpoint by a fresh state (restart),
    /// and an unknown window is a 400.
    #[tokio::test]
    async fn s3_recorded_usage_survives_a_restart_and_bad_windows_are_rejected() {
        let Some(state) = crate::plugins::tests::test_state().await else {
            eprintln!("skipping: LANRURUGI_TEST_REDIS_URL not set");
            return;
        };
        let report = lanrurugi_llm::usage::ModelCallReport {
            feature: FeatureId::SUBFOLDER_TANKOUBON_NAMING,
            outcome: None,
            attempts: 1,
            elapsed: std::time::Duration::from_millis(250),
            usage: lanrurugi_llm::TokenUsage {
                prompt: Some(40),
                completion: Some(4),
                total: Some(44),
                cache_hit: Some(30),
                cache_miss: Some(10),
                reasoning: None,
            },
        };
        lanrurugi_llm::usage::record(&state.redis.config, &report).await;

        let fresh = crate::plugins::tests::test_state().await.unwrap();
        let response = usage(
            State(fresh.clone()),
            Query(UsageQuery {
                window: Some("week".into()),
            }),
        )
        .await;
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let body: Value = serde_json::from_slice(&bytes).unwrap();
        let features = body["features"].as_array().unwrap();
        assert_eq!(features.len(), FeatureId::ALL.len());
        let row = features
            .iter()
            .find(|f| f["feature"] == "subfolder_tankoubon_naming")
            .unwrap();
        assert_eq!(row["status"], "reported");
        assert!(row["requests"].as_u64().unwrap() >= 1);
        assert!(row["tokens"]["cache_hit"].as_u64().unwrap() >= 30);

        let bad = usage(
            State(fresh),
            Query(UsageQuery {
                window: Some("bogus".into()),
            }),
        )
        .await;
        assert_eq!(bad.status(), StatusCode::BAD_REQUEST);
    }

    #[test]
    fn windows_cover_today_and_the_previous_six_days() {
        let today = chrono::NaiveDate::from_ymd_opt(2026, 10, 3).unwrap();
        assert_eq!(window_dates("day", today).unwrap(), ["2026-10-03"]);
        let week = window_dates("week", today).unwrap();
        assert_eq!(week.len(), 7);
        assert_eq!(week[6], "2026-09-27");
        assert!(window_dates("bogus", today).is_none());
    }

    #[test]
    fn cache_hits_alone_still_make_a_row() {
        let row = feature_row(
            FeatureId::TANKOUBON_GROUPING_REVIEW,
            &[
                day(&[("local_cache_hits", 3)]),
                day(&[("local_cache_hits", 2)]),
            ],
        );
        assert_eq!(row["status"], "reported");
        assert_eq!(row["requests"], 0);
        assert_eq!(row["local_cache_hits"], 5);
    }

    #[test]
    fn a_feature_with_no_calls_is_no_data_not_zero() {
        let row = feature_row(FeatureId::ARCHIVE_SPLIT, &[HashMap::new(), HashMap::new()]);
        assert_eq!(
            row,
            json!({ "feature": "archive_split", "status": "no_data" })
        );
    }

    #[test]
    fn days_sum_and_missing_days_count_as_nothing() {
        let row = feature_row(
            FeatureId::PLUGIN_GENERATION,
            &[
                day(&[
                    ("requests", 2),
                    ("elapsed_ms", 900),
                    ("prompt_tokens", 100),
                    ("fail_timeout", 1),
                ]),
                HashMap::new(),
                day(&[
                    ("requests", 1),
                    ("elapsed_ms", 100),
                    ("prompt_tokens", 50),
                    ("cancelled", 1),
                ]),
            ],
        );
        assert_eq!(row["status"], "reported");
        assert_eq!(row["requests"], 3);
        assert_eq!(row["elapsed_ms"], 1000);
        assert_eq!(row["tokens"]["prompt"], 150);
        assert_eq!(row["failures"]["timeout"], 1);
        assert_eq!(row["cancelled"], 1);
    }

    #[test]
    fn unreported_token_figures_are_omitted_rather_than_zeroed() {
        let row = feature_row(
            FeatureId::RECOMMEND_RERANK,
            &[day(&[
                ("requests", 4),
                ("elapsed_ms", 40),
                ("unreported_token_calls", 4),
            ])],
        );
        assert!(row["tokens"].get("prompt").is_none());
        assert!(row["tokens"].get("cache_hit").is_none());
        assert_eq!(row["tokens"]["unreported_calls"], 4);
    }

    #[test]
    fn cache_and_reasoning_figures_appear_when_reported() {
        let row = feature_row(
            FeatureId::PLUGIN_GENERATION,
            &[day(&[
                ("requests", 1),
                ("prompt_cache_hit_tokens", 70),
                ("prompt_cache_miss_tokens", 30),
                ("reasoning_tokens", 5),
            ])],
        );
        assert_eq!(row["tokens"]["cache_hit"], 70);
        assert_eq!(row["tokens"]["cache_miss"], 30);
        assert_eq!(row["tokens"]["reasoning"], 5);
    }
}
