//! `GET`/`PUT /translation/settings` and `GET /translation/usage`
//! (`specs/004-ocr-manga-translation/contracts/translation-api.md`, T025/T044).
//!
//! All paths here are new and additive — no Phase 1 endpoint's shape changes (constitution
//! Principle II).
//!
//! **Credential handling (FR-006, Principle V)**: a `PUT` may *set* a provider API key, but no
//! response ever returns one. The read shape carries only a `credential_set` boolean, mirroring how
//! Phase 1's own `llm_api_key`/`llm_api_key_set` pair already works.
//!
//! Local-backend configuration deliberately has **no endpoint at all** — it lives in the browser's
//! `localStorage` (research.md §8), since a loopback address is only meaningful on the device it
//! was configured on.

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, put};
use axum::{Json, Router};
use serde::{Deserialize, Deserializer};
use serde_json::json;

use lanrurugi_translate::budget::BudgetRepository;
use lanrurugi_translate::credentials::CredentialStore;
use lanrurugi_translate::settings::{
    CloudProvider, TranslationSettings, TranslationSettingsRepository,
};

use crate::auth_context::AuthContext;
use crate::common::error;
use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route(
            "/translation/settings",
            get(get_translation_settings).put(update_translation_settings),
        )
        .route("/translation/usage", get(get_translation_usage))
        .route("/translation/budget", put(update_budget))
}

fn settings_repo(state: &AppState) -> TranslationSettingsRepository {
    TranslationSettingsRepository::new(state.redis.config.clone())
}

fn credential_store(state: &AppState) -> CredentialStore {
    CredentialStore::new(state.redis.config.clone())
}

fn budget_repo(state: &AppState) -> BudgetRepository {
    BudgetRepository::new(state.redis.config.clone())
}

/// Whether the project-wide DeepSeek key (`LRR_CONFIG.llm_api_key`, or the `DEEPSEEK_API_KEY` env
/// fallback) is available.
///
/// When it is, DeepSeek translation reuses it instead of a translation-specific credential: keeping
/// two independently-editable DeepSeek keys for one account is meaningless, so the global one wins
/// unconditionally and the translation settings UI hides its own key input.
pub async fn global_deepseek_key(state: &AppState) -> Option<String> {
    lanrurugi_llm::resolve_api_key(&state.redis.config).await
}

/// Whether *this* provider is served by the global key — DeepSeek only.
pub async fn uses_global_key(state: &AppState, provider: CloudProvider) -> bool {
    provider == CloudProvider::DeepSeek && global_deepseek_key(state).await.is_some()
}

/// The `credentialSet` value for the currently selected provider.
///
/// DeepSeek reports "configured" from the global key alone, so a user who already set the project's
/// DeepSeek key never has to re-enter it here.
async fn credential_set_for(state: &AppState, provider: Option<CloudProvider>) -> (bool, bool) {
    let Some(provider) = provider else {
        return (false, false);
    };
    if uses_global_key(state, provider).await {
        return (true, true);
    }
    let set = credential_store(state)
        .is_set(&provider.credential_ref())
        .await
        .unwrap_or(false);
    (set, false)
}

/// Response shape — note the absence of any credential field.
fn settings_json(
    settings: &TranslationSettings,
    credential_set: bool,
    uses_global_key: bool,
    global_deepseek_key_set: bool,
) -> serde_json::Value {
    json!({
        "provider": settings.provider.map(|p| p.as_str()),
        "endpoint": settings.endpoint,
        "model": settings.model,
        "targetLanguage": settings.target_language,
        "lookaheadPages": settings.lookahead_pages,
        "batchPages": settings.batch_pages,
        // The only credential fact any response may expose (FR-006).
        "credentialSet": credential_set,
        // Tells the frontend to hide its own API-key input: this provider's key comes from the
        // project-wide DeepSeek setting and cannot be overridden here. Computed against the
        // *currently saved* `settings.provider` — see `globalDeepseekKeySet` below for the
        // provider-independent fact the frontend needs while the user is still choosing.
        "usesGlobalKey": uses_global_key,
        // Independent of `settings.provider`: whether the project-wide DeepSeek key exists at
        // all. The frontend needs this the moment a user picks "DeepSeek" in the provider
        // dropdown, before saving — `usesGlobalKey` above can't answer that because it's derived
        // from the provider already persisted on the server, not whatever's currently selected in
        // the (unsaved) form.
        "globalDeepseekKeySet": global_deepseek_key_set,
    })
}

async fn get_translation_settings(State(state): State<AppState>) -> Response {
    let settings = match settings_repo(&state).get().await {
        Ok(s) => s,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_translation_settings",
                e.to_string(),
            )
        }
    };

    let (credential_set, uses_global) = credential_set_for(&state, settings.provider).await;
    let global_deepseek_key_set = global_deepseek_key(&state).await.is_some();

    Json(settings_json(
        &settings,
        credential_set,
        uses_global,
        global_deepseek_key_set,
    ))
    .into_response()
}

/// Distinguishes "field absent" (outer `None`, leave unchanged) from "field explicitly `null`"
/// (outer `Some(None)`, clear it) for a `Option<Option<T>>` body field — serde's default
/// `Option<T>` deserialization collapses both an absent field and an explicit `null` to `None`,
/// which would make a client's "clear this field" request indistinguishable from "didn't send
/// this field at all" and silently no-op (confirmed via a standalone repro before fixing this).
fn double_option<'de, D, T>(deserializer: D) -> Result<Option<Option<T>>, D::Error>
where
    D: Deserializer<'de>,
    T: Deserialize<'de>,
{
    Option::<T>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateSettingsBody {
    #[serde(default, deserialize_with = "double_option")]
    pub provider: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub endpoint: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub model: Option<Option<String>>,
    #[serde(default, deserialize_with = "double_option")]
    pub target_language: Option<Option<String>>,
    pub lookahead_pages: Option<u32>,
    pub batch_pages: Option<u32>,
    /// Write-only. Never echoed back in any response.
    pub api_key: Option<String>,
}

async fn update_translation_settings(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<AuthContext>>,
    Json(body): Json<UpdateSettingsBody>,
) -> Response {
    let repo = settings_repo(&state);
    let mut settings = match repo.get().await {
        Ok(s) => s,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_translation_settings",
                e.to_string(),
            )
        }
    };

    if let Some(provider) = body.provider {
        match provider {
            Some(name) => match CloudProvider::parse(&name) {
                Some(p) => settings.provider = Some(p),
                None => {
                    return error(
                        StatusCode::BAD_REQUEST,
                        "update_translation_settings",
                        format!("unknown provider {name:?}"),
                    )
                }
            },
            None => settings.provider = None,
        }
    }
    if let Some(endpoint) = body.endpoint {
        settings.endpoint = endpoint.filter(|e| !e.trim().is_empty());
    }
    if let Some(model) = body.model {
        settings.model = model.filter(|m| !m.trim().is_empty());
    }
    if let Some(target_language) = body.target_language {
        // An empty value clears the preference back to "use the browser's language" (FR-004).
        settings.target_language = target_language.filter(|l| !l.trim().is_empty());
    }
    if let Some(lookahead) = body.lookahead_pages {
        settings.lookahead_pages = lookahead.min(20);
    }
    if let Some(batch) = body.batch_pages {
        settings.batch_pages = batch;
    }

    if let Err(e) = repo.save(&settings).await {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_translation_settings",
            e.to_string(),
        );
    }

    // Store the credential separately, and only ever write it — never read it back out.
    // A provider served by the global key ignores any submitted value outright: allowing a second,
    // translation-only DeepSeek key would create two sources of truth for one account.
    if let (Some(key), Some(provider)) = (body.api_key.as_ref(), settings.provider) {
        if !key.trim().is_empty() && !uses_global_key(&state, provider).await {
            if let Err(e) = credential_store(&state)
                .store(&provider.credential_ref(), key.trim())
                .await
            {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "update_translation_settings",
                    e.to_string(),
                );
            }
        }
    }

    crate::activity::record_translation_settings_change(
        &state,
        auth.as_ref().map(|e| &e.0),
        &settings,
    )
    .await;

    let (credential_set, uses_global) = credential_set_for(&state, settings.provider).await;
    let global_deepseek_key_set = global_deepseek_key(&state).await.is_some();

    Json(settings_json(
        &settings,
        credential_set,
        uses_global,
        global_deepseek_key_set,
    ))
    .into_response()
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UsageQuery {
    pub archive_id: Option<String>,
    pub page: Option<u32>,
}

/// Current consumption at the four granularities FR-014 requires.
///
/// Returns a zeroed result rather than an error when no metered backend is configured — a
/// locally-hosted backend simply has no usage to report.
async fn get_translation_usage(
    State(state): State<AppState>,
    axum::extract::Query(q): axum::extract::Query<UsageQuery>,
) -> Response {
    let settings = match settings_repo(&state).get().await {
        Ok(s) => s,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_translation_usage",
                e.to_string(),
            )
        }
    };

    let Some(provider) = settings.provider else {
        return Json(json!({
            "provider": null,
            "limit": null,
            "consumptionCurrentPage": 0,
            "consumptionCurrentArchive": 0,
            "consumptionToday": 0,
            "consumptionCurrentWeek": 0,
        }))
        .into_response();
    };

    let archive_id = lanrurugi_core::ids::ArchiveId::from(q.archive_id.unwrap_or_default());
    let page = lanrurugi_ocr::entities::PageNumber(q.page.unwrap_or(0));

    match budget_repo(&state)
        .snapshot(provider.as_str(), &archive_id, page)
        .await
    {
        Ok(snapshot) => Json(json!({
            "provider": snapshot.provider,
            "limit": snapshot.limit,
            "consumptionCurrentPage": snapshot.consumption_current_page,
            "consumptionCurrentArchive": snapshot.consumption_current_archive,
            "consumptionToday": snapshot.consumption_today,
            "consumptionCurrentWeek": snapshot.consumption_current_week,
        }))
        .into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_translation_usage",
            e.to_string(),
        ),
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateBudgetBody {
    /// `null` clears the limit.
    pub limit: Option<u64>,
}

async fn update_budget(
    State(state): State<AppState>,
    Json(body): Json<UpdateBudgetBody>,
) -> Response {
    let settings = match settings_repo(&state).get().await {
        Ok(s) => s,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "update_budget",
                e.to_string(),
            )
        }
    };

    let Some(provider) = settings.provider else {
        return error(
            StatusCode::BAD_REQUEST,
            "update_budget",
            "no metered backend is configured",
        );
    };

    match budget_repo(&state)
        .set_limit(provider.as_str(), body.limit.filter(|&l| l > 0))
        .await
    {
        Ok(()) => Json(json!({ "limit": body.limit })).into_response(),
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "update_budget",
            e.to_string(),
        ),
    }
}
