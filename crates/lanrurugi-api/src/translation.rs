//! Per-page translation endpoints (`contracts/translation-api.md`, T028/T031/T035/T040/T049a/T057).
//!
//! Two paths, split by where the translated text can legally exist (constitution Principle V):
//!
//! - **Cloud backend** → `GET /archives/{id}/page/{page}/translation`. The server proxies the LLM
//!   call (so the credential never reaches the browser), composites the page, and caches it.
//! - **Locally-hosted backend** → `GET /archives/{id}/page/{page}/text-regions` plus
//!   `POST .../translation/record`. The browser calls its own loopback model directly and
//!   composites client-side; the server only supplies detection/context and records discovered
//!   terms back into the shared glossary.
//!
//! **Disabled costs nothing (FR-007)**: with translation off, every handler here returns
//! immediately after one settings read. No OCR model is loaded, no page is decoded, no background
//! work is scheduled — a reader who never enables translation takes no extra code path at all.

use axum::extract::{Path, Query, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;

use lanrurugi_core::ids::ArchiveId;
use lanrurugi_fontcache::FontPatternRepository;
use lanrurugi_ocr::entities::{PageNumber, VolumeId};
use lanrurugi_storage::activity::action_types;
use lanrurugi_translate::context_assembly::{assemble, capture_terms, TranslatedNeighbor};
use lanrurugi_translate::glossary::GlossaryRepository;
use lanrurugi_translate::regions::RegionRepository;
use lanrurugi_translate::settings::TranslationSettingsRepository;

use crate::auth_context::AuthContext;
use crate::common::{error, not_found};
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/archives/{id}/page/{page}/translation",
            get(get_page_translation),
        )
        .route(
            "/archives/{id}/page/{page}/text-regions",
            get(get_page_text_regions),
        )
        .route(
            "/archives/{id}/page/{page}/translation/record",
            post(record_local_translation),
        )
        .route(
            "/archives/{id}/translation/scope",
            get(get_translation_scope).put(update_translation_scope),
        )
}

#[derive(Debug, Deserialize)]
pub struct TranslationQuery {
    /// BCP-47 target language. Absent falls back to the server-stored preference; if that's also
    /// unset the client is expected to have resolved the browser language itself (FR-004).
    pub lang: Option<String>,
}

fn settings_repo(state: &AppState) -> TranslationSettingsRepository {
    TranslationSettingsRepository::new(state.redis.config.clone())
}

fn region_repo(state: &AppState) -> RegionRepository {
    RegionRepository::new(state.redis.config.clone())
}

fn glossary_repo(state: &AppState) -> GlossaryRepository {
    GlossaryRepository::new(state.redis.config.clone())
}

fn font_repo(state: &AppState) -> FontPatternRepository {
    FontPatternRepository::new(state.redis.config.clone())
}

/// Every Tankoubon `archive_id` currently belongs to — index-backed (`GroupingRepository::
/// for_archive`), shared by `resolve_volume_id` and the translation-enabled scope resolution so
/// both agree on membership without either re-deriving it its own way.
async fn tankoubon_memberships(
    state: &AppState,
    archive_id: &ArchiveId,
) -> Vec<lanrurugi_core::ids::TankId> {
    state
        .repos
        .groupings
        .for_archive(archive_id)
        .await
        .unwrap_or_default()
        .into_iter()
        .map(|g| g.tankid)
        .collect()
}

/// The volume scope for a given archive: its Tankoubon grouping if it belongs to one, else the
/// archive itself (data-model.md — "per-archive if ungrouped"). Shared by every endpoint here so
/// the glossary and font pattern always agree on scope.
async fn resolve_volume_id(state: &AppState, archive_id: &ArchiveId) -> VolumeId {
    match tankoubon_memberships(state, archive_id).await.first() {
        Some(tank_id) => VolumeId::from(tank_id.as_str()),
        None => VolumeId::from_archive(archive_id),
    }
}

fn scope_repo(state: &AppState) -> lanrurugi_translate::settings::TranslationScopeRepository {
    lanrurugi_translate::settings::TranslationScopeRepository::new(state.redis.config.clone())
}

/// Reports whether translation is on for this archive, and whether that's this archive's own
/// setting or inherited from a Tankoubon it belongs to — the latter so the reader can tell the
/// user "this follows the whole book's setting" rather than presenting it as if toggling here only
/// ever affects this one file.
async fn get_translation_scope(State(state): State<AppState>, Path(id): Path<String>) -> Response {
    let archive_id = ArchiveId::from(id);
    let member_of = tankoubon_memberships(&state, &archive_id).await;

    match scope_repo(&state).is_enabled(&archive_id, &member_of).await {
        Ok(enabled) => Json(json!({
            "enabled": enabled,
            "scope": if member_of.is_empty() { "archive" } else { "tankoubon" },
        }))
        .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_translation_scope",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
pub struct UpdateScopeBody {
    pub enabled: bool,
}

/// Sets the switch for whichever scope actually governs this archive right now (its Tankoubon if
/// it has one, else the archive itself) — see `TranslationScopeRepository::set_enabled`'s own docs
/// for why toggling here only ever targets one or the other, never both.
async fn update_translation_scope(
    State(state): State<AppState>,
    Path(id): Path<String>,
    Json(body): Json<UpdateScopeBody>,
) -> Response {
    let archive_id = ArchiveId::from(id);
    let member_of = tankoubon_memberships(&state, &archive_id).await;

    match scope_repo(&state)
        .set_enabled(&archive_id, &member_of, body.enabled)
        .await
    {
        Ok(()) => Json(json!({
            "enabled": body.enabled,
            "scope": if member_of.is_empty() { "archive" } else { "tankoubon" },
        }))
        .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_translation_scope",
            e.to_string(),
        ),
    }
}

/// "Translation is unavailable for this page" — never a blank image or a broken response
/// (FR-019). The reader shows the original page alongside this.
fn translation_unavailable(kind: &str, message: impl Into<String>) -> Response {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({
            "operation": "get_page_translation",
            "success": 0,
            "translationAvailable": false,
            "kind": kind,
            "error": message.into(),
        })),
    )
        .into_response()
}

/// "Not ready yet, still processing" — deliberately NOT an error, so the frontend shows the
/// non-obscuring loading state (FR-012) rather than a failure indicator.
fn not_ready() -> Response {
    (
        StatusCode::ACCEPTED,
        Json(json!({
            "operation": "get_page_translation",
            "success": 1,
            "ready": false,
        })),
    )
        .into_response()
}

/// Serves the composited, translated page for the cloud-backend path (T028).
///
/// Returns the cached rendering when one exists. On a miss it **triggers** the real orchestration —
/// OCR → font routing → batched translation → compositing → cache write
/// (`translation_pipeline::translate_page`) — for this page and its look-ahead window, then reports
/// not-ready immediately.
///
/// The work is deliberately not awaited here: a full OCR + LLM cycle takes tens of seconds, and
/// FR-012 requires the reader to keep showing the original page with a non-blocking indicator
/// meanwhile. The client polls; a later poll finds the cache entry this triggered. Reading must
/// never wait on translation (SC-007).
async fn get_page_translation(
    State(state): State<AppState>,
    Path((id, page)): Path<(String, u32)>,
    Query(q): Query<TranslationQuery>,
) -> Response {
    let archive_id = ArchiveId::from(id);
    let member_of = tankoubon_memberships(&state, &archive_id).await;

    // FR-007: with translation off this is the entire cost of the request. Scoped per-archive/
    // per-Tankoubon, not account-wide — turning translation on for one book must never turn it on
    // for every unrelated archive in the library.
    let enabled = match scope_repo(&state).is_enabled(&archive_id, &member_of).await {
        Ok(e) => e,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_page_translation",
                e.to_string(),
            )
        }
    };
    if !enabled {
        return translation_unavailable("disabled", "translation is disabled");
    }

    let settings = match settings_repo(&state).get().await {
        Ok(s) => s,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_page_translation",
                e.to_string(),
            )
        }
    };

    // FR-021: enabled but unconfigured is a guidance case, not a silent failure.
    let Some(provider) = settings.provider else {
        return translation_unavailable("not_configured", "no translation backend is configured");
    };

    let page_number = PageNumber(page);
    let Some(target_language) = q.lang.or_else(|| settings.target_language.clone()) else {
        return translation_unavailable("no_target_language", "no target language selected");
    };

    let cache = lanrurugi_translate::cache::TranslationImageCache::new(&state.library.temp_dir);
    let key = lanrurugi_translate::cache::TranslationCacheKey::new(
        archive_id.clone(),
        page_number,
        target_language.clone(),
        provider.as_str(),
    );

    if let Some(bytes) = cache.get(&key).await {
        state.translation_telemetry.record_lookahead(true);
        return ([(header::CONTENT_TYPE, "image/webp")], bytes).into_response();
    }

    // An already-failed page must surface as FR-019's per-page "unavailable", not as a fresh 202
    // for every poll. The scheduler deliberately does not auto-retry failures, so returning
    // not-ready here would leave the reader's loading indicator spinning forever. This is checked
    // before loading the OCR runtime so a failed page doesn't keep paying model-load cost on every
    // poll.
    if let Some(lanrurugi_translate::prefetch::PageState::Failed(kind)) = state
        .translation_scheduler
        .state(&archive_id, page_number)
        .await
    {
        tracing::debug!(%archive_id, %page_number, kind = %kind, "returning cached page failure state");
        return translation_unavailable(&kind, "the previous translation attempt failed");
    }

    // --- Cache miss: trigger the real pipeline ------------------------------------------------
    // Loading the OCR models/fonts happens here, on first actual use. A deployment with neither
    // installed reports FR-019's "unavailable" rather than leaving the reader polling forever.
    let handles = match crate::translation_pipeline::build_handles(
        &state,
        settings,
        provider,
        target_language,
    )
    .await
    {
        Ok(handles) => handles,
        Err(e) => {
            tracing::warn!(error = %e, "translation runtime unavailable");
            return translation_unavailable("runtime_unavailable", e.to_string());
        }
    };

    let Some(total) = crate::translation_pipeline::total_pages(&state, &archive_id).await else {
        return not_found("get_page_translation", "no such archive");
    };

    // Records where the reader is and spawns this page plus its look-ahead window (T039). Returns
    // as soon as the work is scheduled — never awaits it.
    state
        .translation_scheduler
        .on_reader_at(&state, &handles, &archive_id, page_number, total)
        .await;

    state.translation_telemetry.record_lookahead(false);
    not_ready()
}

/// Detection output plus everything the browser needs to translate and composite locally.
///
/// Includes the same `context` the cloud path assembles, so a locally-hosted backend gets
/// FR-007a–e's consistency benefit even though its translation call never touches this server.
async fn get_page_text_regions(
    State(state): State<AppState>,
    Path((id, page)): Path<(String, u32)>,
) -> Response {
    let archive_id = ArchiveId::from(id);
    let page_number = PageNumber(page);

    if state
        .repos
        .archives
        .get(&archive_id)
        .await
        .ok()
        .flatten()
        .is_none()
    {
        return not_found("get_page_text_regions", "no such archive");
    }

    let regions = match region_repo(&state)
        .get_detected(&archive_id, page_number)
        .await
    {
        Ok(Some(regions)) => regions,
        // Detection hasn't run for this page yet. The local-backend path has no look-ahead of its
        // own to wait for — the browser translates and composites itself, and the *only* thing it
        // needs from this server is detection. So trigger OCR here rather than reporting not-ready
        // forever (the same never-actually-wired defect T028 fixes on the cloud path).
        //
        // Note this deliberately stops at detection: no translation, no LLM call, no credential.
        // The translated text for this path only ever exists in the browser (constitution
        // Principle V).
        Ok(None) => match trigger_local_path_detection(&state, &archive_id, page_number).await {
            Ok(regions) => regions,
            Err(response) => return response,
        },
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_page_text_regions",
                e.to_string(),
            )
        }
    };

    let volume_id = resolve_volume_id(&state, &archive_id).await;

    let glossary = glossary_repo(&state)
        .get(&volume_id)
        .await
        .unwrap_or_else(|_| lanrurugi_translate::glossary::TerminologyGlossary::new(&volume_id));

    let golden_set = font_repo(&state)
        .get(&volume_id)
        .await
        .map(|p| p.golden_set)
        .unwrap_or_default();

    let sources: Vec<String> = regions.iter().map(|r| r.source_text.clone()).collect();
    let neighbors: Vec<TranslatedNeighbor> = regions
        .iter()
        .filter_map(|r| {
            r.translated_text.as_ref().map(|t| TranslatedNeighbor {
                source_text: r.source_text.clone(),
                translated_text: t.clone(),
            })
        })
        .collect();

    let context = assemble(&glossary, &sources, &neighbors);

    Json(json!({
        "volumeId": volume_id.as_str(),
        "regions": regions,
        "goldenSet": golden_set,
        "context": {
            "glossaryMatches": context.glossary_matches,
            "knownNames": context.known_names,
            "toneReference": context.tone_reference,
        },
    }))
    .into_response()
}

/// Runs OCR for the locally-hosted-backend path and persists the result (US4's half of T028).
///
/// Detection only. The browser owns translation and compositing on this path, so nothing here
/// calls a provider, resolves a credential, or composites an image — `ensure_detected` is exactly
/// the shared step both paths need and neither should duplicate.
///
/// `Err` carries an already-formed response: OCR being unavailable (no model installed) is FR-019's
/// "translation unavailable", not a server error.
async fn trigger_local_path_detection(
    state: &AppState,
    archive_id: &ArchiveId,
    page_number: PageNumber,
) -> Result<Vec<lanrurugi_ocr::entities::DetectedTextRegion>, Response> {
    let settings = settings_repo(state).get().await.unwrap_or_default();

    // The local path has no server-side provider by definition. This value never reaches a network
    // call here — `ensure_detected` doesn't translate — it only satisfies the shared handle shape.
    let provider = settings
        .provider
        .unwrap_or(lanrurugi_translate::settings::CloudProvider::OpenAiCompatible);
    let target_language = settings.target_language.clone().unwrap_or_default();

    let handles =
        crate::translation_pipeline::build_handles(state, settings, provider, target_language)
            .await
            .map_err(|e| {
                tracing::warn!(error = %e, "OCR runtime unavailable for the local-backend path");
                translation_unavailable("runtime_unavailable", e.to_string())
            })?;

    // One up-front bubble-segmentation call, same as the cloud path (`translate_page`'s own doc
    // comment covers why this must not run a second time inside `ensure_detected` itself — a real
    // cross-worker VRAM race found 2026-09-14).
    let image_worker_for_bubbles = std::sync::Arc::clone(&handles.image_worker);
    let bubble_page_image =
        crate::translation_pipeline::load_page_image(state, archive_id, page_number)
            .await
            .map(|(image, _is_cover, _total)| image)
            .map_err(|e| {
                tracing::warn!(error = %e, "page image unavailable for the local-backend path");
                translation_unavailable("detection_failed", e.to_string())
            })?;
    let bubbles = lanrurugi_core::concurrency::run_blocking(move || {
        use lanrurugi_ocr::bubble_segment::BubbleSegmenterHandle;
        BubbleSegmenterHandle::detect(&*image_worker_for_bubbles, &bubble_page_image)
            .inspect_err(|e| {
                tracing::warn!(error = %e, "bubble segmentation failed for this page; regions stay unmerged")
            })
            .ok()
    })
    .await
    .unwrap_or(None);

    crate::translation_pipeline::ensure_detected(
        state,
        &handles,
        archive_id,
        page_number,
        bubbles.as_deref(),
    )
    .await
    .map_err(|e| {
        tracing::warn!(error = %e, "detection failed for the local-backend path");
        translation_unavailable("detection_failed", e.to_string())
    })
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordTranslationBody {
    /// Source/translation pairs the local backend produced.
    pub translations: Vec<RecordedPair>,
    pub target_language: String,
    /// The device's own local backend identifier — used to key the persisted regions so a local
    /// result never collides with a cloud one.
    pub provider: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordedPair {
    pub source_text: String,
    pub translated_text: String,
}

/// Records a translation the browser obtained from its own local model (T049a).
///
/// **No LLM call happens here** — this is a lightweight write. Its purpose is that a name/term the
/// local backend translated still reaches the shared, server-stored glossary, so later cloud-path
/// (or other-device) requests for the same volume stay consistent with it.
async fn record_local_translation(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Path((id, page)): Path<(String, u32)>,
    Json(body): Json<RecordTranslationBody>,
) -> Response {
    let archive_id = ArchiveId::from(id);
    let page_number = PageNumber(page);
    let volume_id = resolve_volume_id(&state, &archive_id).await;

    let glossary_repo = glossary_repo(&state);
    let mut glossary = match glossary_repo.get(&volume_id).await {
        Ok(g) => g,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "record_local_translation",
                e.to_string(),
            )
        }
    };

    // The locally-hosted-backend path doesn't yet ask the browser's own model for a `term_kind`
    // classification (its request/schema construction is separate front-end code that predates
    // this field) — every pair reports `TermKind::None` here, so nothing from this path is
    // captured into the glossary until that's wired up. Correct-by-omission rather than falling
    // back to the old length/punctuation heuristic for this path only, which would silently bring
    // back the exact misclassification bug the cloud path just moved away from.
    let pairs: Vec<(String, String, lanrurugi_translate::adapter::TermKind)> = body
        .translations
        .iter()
        .map(|p| {
            (
                p.source_text.clone(),
                p.translated_text.clone(),
                lanrurugi_translate::adapter::TermKind::None,
            )
        })
        .collect();

    let chapter_name =
        crate::translation_pipeline::resolve_chapter_name(&state, &archive_id, page).await;
    let captured = capture_terms(
        &mut glossary,
        &pairs,
        archive_id.as_str(),
        chapter_name.as_deref(),
    );

    if !captured.is_empty() {
        if let Err(e) = glossary_repo.save(&glossary).await {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "record_local_translation",
                e.to_string(),
            );
        }
        for term in &captured {
            let translation = pairs
                .iter()
                .find(|(s, _, _)| s == term)
                .map(|(_, t, _)| t.as_str());
            crate::activity::record_glossary_change(
                &state,
                crate::activity::GlossaryActor::Manual(auth.as_ref().map(|e| &e.0)),
                action_types::TRANSLATION_GLOSSARY_CAPTURE,
                volume_id.as_str(),
                term,
                translation,
            )
            .await;
        }
    }

    // Persist the translated regions too, so this device's work isn't lost and another device can
    // re-composite from it without re-translating (research.md §16).
    let region_repo = region_repo(&state);
    if let Ok(Some(mut regions)) = region_repo.get_detected(&archive_id, page_number).await {
        for region in &mut regions {
            if let Some((_, translation, _)) = pairs
                .iter()
                .find(|(source, _, _)| source == &region.source_text)
            {
                region.translated_text = Some(translation.clone());
            }
        }
        let _ = region_repo
            .save_translated(
                &archive_id,
                page_number,
                &body.target_language,
                &body.provider,
                &regions,
            )
            .await;
    }

    Json(json!({
        "operation": "record_local_translation",
        "success": 1,
        "capturedTerms": captured,
    }))
    .into_response()
}
