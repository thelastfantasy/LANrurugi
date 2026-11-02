//! `POST /volumes/{id}/font-pattern/reset` (FR-010, T036).
//!
//! Exists for the case where a volume's pattern locked on unrepresentative early pages and is wrong
//! for the rest of the volume. Resetting clears `vote_pool`, `golden_set`, and `meltdown_tally`
//! back to unlocked so voting starts over — a bulk operation that makes sense here precisely
//! because the pattern is one derived thing, unlike the glossary's independent per-term entries.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::json;

use lanrurugi_fontcache::FontPatternRepository;
use lanrurugi_ocr::entities::VolumeId;

use crate::auth_context::AuthContext;
use crate::common::error;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/volumes/{id}/font-pattern/reset", post(reset_font_pattern))
        .route(
            "/volumes/{id}/font-pattern",
            axum::routing::get(get_font_pattern),
        )
}

fn repo(state: &AppState) -> FontPatternRepository {
    FontPatternRepository::new(state.redis.config.clone())
}

async fn get_font_pattern(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match repo(&state).get(&VolumeId::from(id)).await {
        Ok(pattern) => Json(json!({
            "volumeId": pattern.volume_id,
            "isLocked": pattern.is_locked,
            "goldenSet": pattern.golden_set,
            "voteTotal": pattern.total_votes(),
        }))
        .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_font_pattern",
            e.to_string(),
        ),
    }
}

async fn reset_font_pattern(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path(id): Path<String>,
) -> Response {
    let volume_id = VolumeId::from(id);

    match repo(&state).reset(&volume_id).await {
        Ok(pattern) => {
            crate::activity::record_font_pattern_reset(
                &state,
                auth.as_ref().map(|e| &e.0),
                volume_id.as_str(),
            )
            .await;
            Json(json!({
                "volumeId": pattern.volume_id,
                "isLocked": pattern.is_locked,
                "goldenSet": pattern.golden_set,
            }))
            .into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "reset_font_pattern",
            e.to_string(),
        ),
    }
}
