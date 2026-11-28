//! `GET /api/resources` and `GET /api/resources/stream` (contract `resources-api.md`).
//!
//! The one-shot route deliberately does **not** register an observer: opening a feedback bundle in
//! the settings page must not leave a sampling task behind. The stream route is what starts and
//! stops sampling, and the lease it holds is dropped when the response stream is dropped, which is
//! what makes "close the panel, cost goes to zero" true rather than hopeful.

use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{Query, State};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::StreamExt;
use serde::Deserialize;
use tokio::sync::broadcast::error::RecvError;

use crate::resources::collect;
use crate::resources::watch::ResourceWatch;
use crate::AppState;

/// The contract's clamp: below 500 ms the panel is just watching the sampler's own jitter, above
/// 5 s it stops being a live view.
const MIN_INTERVAL_MS: u64 = 500;
const MAX_INTERVAL_MS: u64 = 5_000;
const DEFAULT_INTERVAL_MS: u64 = 1_000;
/// The sampler runs at 1 Hz, so a request for exactly 1 s lands half a tick either side of the
/// boundary; without this slack every other frame is dropped and the client sees 2 s pacing.
const PACING_SLACK_MS: i64 = 500;
/// Comment frames keep proxies from closing an idle stream.
const KEEP_ALIVE: Duration = Duration::from_secs(15);

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/resources", get(one_shot))
        .route("/resources/stream", get(stream))
}

/// Whether a frame recorded at `at` is far enough past the last one this subscriber saw.
fn due(last_at: i64, at: i64, interval_ms: u64) -> bool {
    at.saturating_sub(last_at) + PACING_SLACK_MS >= interval_ms as i64
}

fn clamp_interval(requested: Option<u64>) -> u64 {
    requested
        .unwrap_or(DEFAULT_INTERVAL_MS)
        .clamp(MIN_INTERVAL_MS, MAX_INTERVAL_MS)
}

#[derive(Debug, Deserialize)]
struct StreamQuery {
    #[serde(default)]
    interval_ms: Option<u64>,
}

/// One full observation, read fresh. Registers no observer, so this route is safe to call from a
/// script — it costs one `/proc` walk and nothing that outlives the request.
async fn one_shot(State(state): State<AppState>) -> Response {
    let _task = ResourceWatch::global().track_task();
    Json(collect(&state).await).into_response()
}

fn event_json(name: &str, value: &serde_json::Value) -> Option<Event> {
    serde_json::to_string(value)
        .ok()
        .map(|body| Event::default().event(name).data(body))
}

async fn stream(State(state): State<AppState>, Query(query): Query<StreamQuery>) -> Response {
    let interval_ms = clamp_interval(query.interval_ms);
    let watch: &'static Arc<ResourceWatch> = ResourceWatch::global();
    // Subscribe before taking the snapshot: a frame published in between would be lost, and the
    // first `sample` would then jump backwards in time relative to the snapshot.
    let receiver = watch.subscribe();
    let lease = watch.attach(&state).await;
    let tracked = watch.track_task();

    let first = match watch.last() {
        Some(snapshot) => snapshot,
        None => collect(&state).await,
    };
    let Some(first_event) = serde_json::to_value(&first)
        .ok()
        .and_then(|value| event_json("snapshot", &value))
    else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({ "error": "resources unavailable: snapshot is not serialisable" })),
        )
            .into_response();
    };

    let body = futures_util::stream::once(async move { Ok::<_, Infallible>(first_event) }).chain(
        futures_util::stream::unfold(
            (receiver, lease, tracked, first.at),
            move |(mut receiver, lease, tracked, last_at)| async move {
                loop {
                    match receiver.recv().await {
                        Ok(sample) => {
                            // Per-subscriber pacing: the sampler runs at 1 Hz, a client asking for
                            // 5 s should get every fifth frame rather than a re-timed sampler.
                            if !due(last_at, sample.at, interval_ms) {
                                continue;
                            }
                            let Some(value) = serde_json::to_value(&sample).ok() else {
                                continue;
                            };
                            let Some(event) = event_json("sample", &value) else {
                                continue;
                            };
                            return Some((
                                Ok::<_, Infallible>(event),
                                (receiver, lease, tracked, sample.at),
                            ));
                        }
                        // A slow client missed frames; the next one it gets is still an absolute
                        // reading, so skipping is safe.
                        Err(RecvError::Lagged(_)) => continue,
                        Err(RecvError::Closed) => return None,
                    }
                }
            },
        ),
    );

    Sse::new(body)
        .keep_alive(KeepAlive::new().interval(KEEP_ALIVE).text("keep-alive"))
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_one_second_request_keeps_every_frame_of_a_one_hertz_sampler() {
        // The sampler drifts either side of the boundary; both 999 and 1001 are "one tick later".
        assert!(due(1_000, 1_999, 1_000));
        assert!(due(1_000, 2_001, 1_000));
        assert!(!due(1_000, 1_200, 1_000));
        // A 5 s request still waits for the fifth frame.
        assert!(!due(1_000, 5_400, 5_000));
        assert!(due(1_000, 6_000, 5_000));
        // 500 ms cannot be faster than the sampler, so every frame qualifies.
        assert!(due(1_000, 2_000, 500));
    }

    #[test]
    fn the_interval_is_clamped_to_the_contract_range() {
        assert_eq!(clamp_interval(None), DEFAULT_INTERVAL_MS);
        assert_eq!(clamp_interval(Some(1)), MIN_INTERVAL_MS);
        assert_eq!(clamp_interval(Some(60_000)), MAX_INTERVAL_MS);
        assert_eq!(clamp_interval(Some(2_000)), 2_000);
    }
}
