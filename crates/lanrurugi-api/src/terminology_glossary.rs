//! Terminology Glossary management endpoints (FR-007d, T032a,
//! `contracts/translation-api.md`).
//!
//! `GET`/`PUT`/`DELETE` on individual entries. There is deliberately **no bulk-clear endpoint**,
//! unlike font-pattern reset: a wrong glossary entry is independent of the others, so wiping the
//! whole glossary would discard already-correct entries (many of them possibly hand-corrected) for
//! no benefit.

use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use lanrurugi_ocr::entities::VolumeId;
use lanrurugi_storage::activity::action_types;
use lanrurugi_translate::glossary::GlossaryRepository;

use crate::auth_context::AuthContext;
use crate::common::{error, not_found};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/volumes/{id}/terminology-glossary", get(get_glossary))
        .route(
            "/volumes/{id}/terminology-glossary/{term}",
            axum::routing::put(update_entry).delete(delete_entry),
        )
}

fn repo(state: &AppState) -> GlossaryRepository {
    GlossaryRepository::new(state.redis.config.clone())
}

async fn get_glossary(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    match repo(&state).get(&VolumeId::from(id)).await {
        Ok(glossary) => Json(json!({
            "volumeId": glossary.volume_id,
            "entries": glossary.entries,
        }))
        .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_terminology_glossary",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateEntryBody {
    pub translation: String,
}

/// Edits a single entry. Takes effect on the next translation request (FR-007d).
async fn update_entry(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path((id, term)): Path<(String, String)>,
    Json(body): Json<UpdateEntryBody>,
) -> Response {
    if body.translation.trim().is_empty() {
        return error(
            StatusCode::BAD_REQUEST,
            "update_glossary_entry",
            "translation must not be empty",
        );
    }

    let volume_id = VolumeId::from(id);
    let repo = repo(&state);

    let mut glossary = match repo.get(&volume_id).await {
        Ok(g) => g,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_glossary_entry",
                e.to_string(),
            )
        }
    };

    // This endpoint predates issue #105's per-(archive, chapter) disambiguation and still only
    // identifies an entry by (volume, term) — in practice this project's own glossary is populated
    // entirely by automatic capture during translation, never hand-edited through this endpoint, so
    // the ambiguous case (a term with more than one source candidate) is expected to be rare to
    // nonexistent. Rather than guess which source a caller meant, an ambiguous term is refused
    // outright; an unambiguous one (the overwhelming common case, and the only case this endpoint
    // was ever exercised against) edits that one entry exactly as before.
    match glossary.entries.get(term.trim()).map(Vec::len).unwrap_or(0) {
        0 => {}
        1 => {
            let (archive_id, chapter_name) = {
                let only = &glossary.entries[term.trim()][0];
                (only.archive_id.clone(), only.chapter_name.clone())
            };
            glossary.set(
                &term,
                &body.translation,
                &archive_id,
                chapter_name.as_deref(),
            );
        }
        _ => {
            return error(
                StatusCode::CONFLICT,
                "update_glossary_entry",
                "this term has translations from more than one source archive/chapter; editing \
                 through this endpoint isn't supported while ambiguous",
            )
        }
    }

    if let Err(e) = repo.save(&glossary).await {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_glossary_entry",
            e.to_string(),
        );
    }

    crate::activity::record_glossary_change(
        &state,
        crate::activity::GlossaryActor::Manual(auth.as_ref().map(|e| &e.0)),
        action_types::TRANSLATION_GLOSSARY_EDIT,
        volume_id.as_str(),
        &term,
        Some(&body.translation),
    )
    .await;

    Json(json!({ "sourceTerm": term, "translation": body.translation })).into_response()
}

async fn delete_entry(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path((id, term)): Path<(String, String)>,
) -> Response {
    let volume_id = VolumeId::from(id);
    let repo = repo(&state);

    let mut glossary = match repo.get(&volume_id).await {
        Ok(g) => g,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "delete_glossary_entry",
                e.to_string(),
            )
        }
    };

    if !glossary.remove(&term) {
        return not_found("delete_glossary_entry", "no such glossary entry");
    }

    if let Err(e) = repo.save(&glossary).await {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "delete_glossary_entry",
            e.to_string(),
        );
    }

    crate::activity::record_glossary_change(
        &state,
        crate::activity::GlossaryActor::Manual(auth.as_ref().map(|e| &e.0)),
        action_types::TRANSLATION_GLOSSARY_DELETE,
        volume_id.as_str(),
        &term,
        None,
    )
    .await;

    crate::common::ok("delete_glossary_entry", [])
}
