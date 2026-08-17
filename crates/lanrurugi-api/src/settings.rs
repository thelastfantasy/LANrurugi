//! `settings` endpoint group — additive, no legacy REST contract equivalent (legacy's `/config`
//! page is a server-rendered HTML form posting back to itself, verified via
//! `~/LANraragi/public/js/mod/server.js::saveFormData`, not part of `tools/openapi.yaml`).
//!
//! Backed by the **same** `LRR_CONFIG` hash on the config logical DB that legacy itself reads and
//! writes (`Model/Config.pm::get_redis_conf`), so a value already set through legacy's own
//! settings page (e.g. `theme`) is read correctly here with zero migration step (Principle I),
//! and a value written here is equally visible to a legacy instance sharing the same Redis.
//!
//! `password` is deliberately excluded from the generic get/put here — it needs bcrypt hashing on
//! the way in (see [`change_password`]) and must never be echoed back out in plaintext-hash form
//! to a settings-page GET, so it gets its own dedicated endpoint instead.

use std::collections::HashMap;

use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use lanrurugi_core::password;
use serde::Deserialize;
use serde_json::{json, Value};

use crate::common::error;
use crate::AppState;
use lanrurugi_storage::keys::CONFIG_KEY;

/// `NUMBER_FIELDS`' own defaults, re-exported so other modules' fallbacks (when a value is missing
/// from `LRR_CONFIG` entirely, e.g. a fresh install before the Settings page's own defaulting
/// logic below has ever run) can't drift out of sync with the one true default declared here.
pub(crate) const DEFAULT_PAGE_SIZE: i64 = 100;
/// Reader resize trigger: pages over this many KB get downscaled/re-encoded to WebP (1.5 MB —
/// large enough that normal manga pages pass through untouched, small enough that a
/// multi-megabyte PNG/webtoon strip still gets compressed for the reader).
pub(crate) const DEFAULT_SIZE_THRESHOLD: i64 = 1536;
pub(crate) const DEFAULT_READER_QUALITY: i64 = 85;
pub(crate) const DEFAULT_WEBP_QUALITY: i64 = 85;

pub fn router() -> Router<AppState> {
    // `/settings/password` ("账号安全类" danger) — no API token, admin-role or not, may change
    // the admin password; only a real session cookie can. See `crate::procedure`'s own module
    // docs for why this is a route-level layer on its own sub-router, not a check inside
    // `put_settings`. `PUT /settings` itself stays outside this gate — see that handler's own
    // docs for why its dangerous fields need a narrower, field-level check instead.
    let password = Router::new()
        .route("/settings/password", post(change_password))
        .route_layer(axum::middleware::from_fn(crate::procedure::require_session));

    Router::new()
        .route("/settings", get(get_settings).put(put_settings))
        .merge(password)
}

/// Deliberately separate from [`router`] and merged unprotected in `lanrurugi-server`'s
/// `build_app` (same pattern as `lanrurugi_api::login::router()`) — the Login page needs the
/// saved theme to render itself correctly, but it runs before any session exists, so it can't go
/// through the auth-gated `/settings` the rest of the Settings page uses. Exposes only `theme`,
/// not the full settings payload (which includes things like `apikey`).
pub fn public_router() -> Router<AppState> {
    Router::new().route("/theme", get(get_theme))
}

/// Every legacy theme filename this app ships CSS for (`apps/frontend/public/legacy/themes/`),
/// mirroring the frontend's own `theme.ts::THEMES` list — the one other place this exact set is
/// declared (that file can't be imported from Rust, so the two must be kept in sync by hand). Used
/// only by `fetch_theme_for_html_injection` below — plain `fetch_theme`/`get_theme` deliberately
/// stay permissive (any string round-trips through `/settings`/`/theme`, matching every other
/// generic settings field), since a value the *React* Login page consumes and renders through
/// ordinary JSX has no injection surface of its own to defend.
const KNOWN_THEME_FILES: &[&str] = &[
    "modern.css",
    "modern_red.css",
    "modern_clear.css",
    "g.css",
    "ex.css",
];

/// Raw `theme` field lookup, no validation — `None` only on an actual failure to reach Redis or
/// read the field (connection pool exhausted, command error, etc.), never on the *value* found.
async fn fetch_theme(state: &AppState) -> Option<String> {
    let mut conn = state.redis.config.get().await.ok()?;
    let theme: Option<String> = conn.hget(CONFIG_KEY, "theme").await.ok()?;
    Some(theme.unwrap_or_else(|| "modern.css".to_string()))
}

/// Used by `lanrurugi-server`'s own `serve_index` handler (the `index.html`-serving SPA fallback),
/// which substitutes this value directly into that file's inline anti-flash-of-default-theme
/// `<script>` body via a plain string replace — not an HTML/JS-aware templating step — so an
/// unconstrained value would be a real reflected-script-injection vector for anyone able to write
/// an arbitrary string to `LRR_CONFIG`'s `theme` field (an authenticated action via `PUT
/// /settings` today, which never validates `theme` against `KNOWN_THEME_FILES`; this check costs
/// nothing and turns that into a no-op rather than counting on every future caller of the raw
/// field to have independently remembered it's about to land inside a `<script>` tag). `None`
/// whenever `fetch_theme` itself fails *or* the stored value isn't recognized — both already mean
/// "fall back to `serve_index`'s own built-in default", so this doesn't distinguish them either.
pub async fn fetch_theme_for_html_injection(state: &AppState) -> Option<String> {
    fetch_theme(state)
        .await
        .filter(|theme| KNOWN_THEME_FILES.contains(&theme.as_str()))
}

async fn get_theme(State(state): State<AppState>) -> Response {
    match fetch_theme(&state).await {
        Some(theme) => axum::Json(json!({ "theme": theme })).into_response(),
        None => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_theme",
            "failed to reach redis".to_string(),
        ),
    }
}

/// Reads `LRR_CONFIG`'s `newbadgemode` (see that field's own doc in [`STRING_FIELDS`]),
/// defaulting to `until_opened` when unset — the one mode that matches legacy's own behavior, so
/// a fresh/legacy-shared Redis starts with the exact badge semantics legacy has.
pub(crate) async fn read_new_badge_mode(state: &AppState) -> String {
    match state.redis.config.get().await {
        Ok(mut conn) => conn
            .hget::<_, _, Option<String>>(CONFIG_KEY, "newbadgemode")
            .await
            .ok()
            .flatten()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| "until_opened".to_string()),
        Err(_) => "until_opened".to_string(),
    }
}

/// `(field, default)` pairs for every `LRR_CONFIG` value the Settings page's Global/Security/
/// Files/Tags sections read or write, verified against `Model/Config.pm`'s `get_redis_conf`
/// calls. Excludes `password` (see module docs) and `theme` (already had its own default before
/// this table existed, kept as a named constant below for that reason).
const STRING_FIELDS: &[(&str, &str)] = &[
    ("theme", "modern.css"),
    ("language", "auto"),
    ("htmltitle", "LANrurugi"),
    ("motd", "Welcome to this Library running LANrurugi!"),
    ("excludednamespaces", "source, date_added"),
    // IANA timezone identifier (e.g. `"Asia/Tokyo"`, `"UTC"` — anything `chrono_tz` accepts),
    // used by date display/search-range math (see `lanrurugi_search::engine`'s `date_added`
    // date-range handling) so that "the day an archive was added" is unambiguous regardless of
    // where the server happens to be deployed or which timezone the *viewer's* browser is in.
    // Defaults to UTC (timezone-independent) — a single admin-configured value shared across all
    // viewers, not per-user: two users looking at the same tag see the same `yyyy-mm-dd` string,
    // matching the same date-range a search by that string would resolve to.
    ("timezone", "UTC"),
    ("tagrules", "-already uploaded;-forbidden content;-incomplete;-ongoing;-complete;-various;-digital;-translated;-russian;-chinese;-portuguese;-french;-spanish;-italian;-vietnamese;-german;-indonesian"),
    // When an archive's "new" badge disappears: `until_opened` (cleared the moment the reader
    // loads — legacy's own behavior), `until_finished` (cleared only once the archive is read to
    // its last page), or a time window `3d`/`7d`/`10d` (cleared N days after `date_added`,
    // regardless of whether it was ever opened). Consumed by `archives::effective_isnew` (badge
    // display) and `lanrurugi_search::engine`'s `newonly` filter so both stay consistent.
    ("newbadgemode", "until_opened"),
    // Reader recommendation-cache precision (`low`/`medium`/`high` — see
    // `recommend_precompute::RecommendPrecision`) — how many candidates
    // `recommend_precompute.rs`'s background rebuild keeps per archive's cached Top-N similar-
    // archive list. Changing this bumps `LANRURUGI_RECOMMEND_META`'s `rebuild_generation` and
    // queues a full rebuild job (see the side-effect branch in `put_settings` below) since a
    // tier change can't be applied retroactively by the incremental one-way backfill alone.
    ("recommendprecision", "medium"),
];

/// `STRING_FIELDS`' own `tagrules` default, exposed for `plugins.rs::get_computed_tagrules` to
/// fall back to when the field has never been written to Redis at all — kept as a lookup over the
/// single source of truth above rather than a second copy of the literal string.
pub fn default_tagrules() -> &'static str {
    STRING_FIELDS
        .iter()
        .find(|(key, _)| *key == "tagrules")
        .map(|(_, default)| *default)
        .expect("\"tagrules\" is a real STRING_FIELDS entry")
}

const NUMBER_FIELDS: &[(&str, i64)] = &[
    ("pagesize", DEFAULT_PAGE_SIZE),
    ("tempmaxsize", 500),
    ("sizethreshold", DEFAULT_SIZE_THRESHOLD),
    ("readerquality", DEFAULT_READER_QUALITY),
    ("webpquality", DEFAULT_WEBP_QUALITY),
    // Issue #44 — the SPA login's JWT access-token / refresh-token lifetimes
    // (`lanrurugi_core::session`, `lanrurugi_api::auth::LiveAuthConfig::load`). The one true
    // default for each lives on `lanrurugi_core::session` itself (not duplicated here as a
    // literal) since that's also where the fallback used when the field is entirely absent from
    // `LRR_CONFIG` lives.
    (
        "access_token_lifetime_secs",
        lanrurugi_core::session::DEFAULT_ACCESS_TOKEN_LIFETIME_SECS as i64,
    ),
    (
        "refresh_token_lifetime_secs",
        lanrurugi_core::session::DEFAULT_REFRESH_TOKEN_LIFETIME_SECS as i64,
    ),
];

const BOOL_FIELDS: &[(&str, bool)] = &[
    ("enablepass", true),
    // `Model/Config.pm::enable_devmode` — legacy's Global-section "Debug Mode" checkbox. Consumed
    // client-side by `/api/info`'s `debug_mode` field (`misc.rs`), which
    // `apps/frontend/src/api/hooks.ts::useUpdateCheck` already reads to skip the GitHub-releases
    // update check while developing (`enabled: !debugMode`) — that consumer existed before this
    // field had any UI to set it from (issue #85's own follow-up survey). No server-side
    // `development`/`production` log-verbosity switch like legacy's `LANraragi.pm` — this port's
    // logging is already structured `tracing` output regardless of this flag, so only the
    // update-check suppression applies here.
    ("devmode", false),
    ("nofunmode", false),
    ("enablecors", false),
    ("localprogress", false),
    ("authprogress", false),
    ("enableresize", true),
    ("hqthumbpages", false),
    ("enablewebp", true),
    ("replacedupe", false),
    ("tagruleson", true),
    // `Model/Config.pm::enable_dateadded`/`use_lastmodified` — consumed by
    // `lanrurugi_scanner::pipeline::catalogue_new_archive` to auto-tag newly catalogued archives
    // with a `date_added:<unix timestamp>` tag, and by `plugins/metadata/dateadded.ts` for the
    // same tag as a manual, per-archive re-run.
    ("usedateadded", true),
    ("usedatemodified", false),
    // `Model/Config.pm::can_replacetitles` — gates whether a metadata plugin's returned `title`
    // is actually applied to an archive (vs. only its tags/summary). Rendered on the Plugins page
    // itself (`~/LANraragi/templates/plugins.html.tt2:84-91`), not the Settings page, even though
    // it's stored in the same `LRR_CONFIG` hash — `Controller/Plugins.pm:31,101-102` reads/writes
    // it directly rather than going through `Config.pm`'s settings-page field list.
    ("replacetitles", true),
];

async fn get_settings(State(state): State<AppState>) -> Response {
    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_settings",
                e.to_string(),
            )
        }
    };
    let fields: HashMap<String, String> = match conn.hgetall(CONFIG_KEY).await {
        Ok(f) => f,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "get_settings",
                e.to_string(),
            )
        }
    };

    let mut body = serde_json::Map::new();
    for (key, default) in STRING_FIELDS {
        body.insert(
            (*key).to_string(),
            json!(fields
                .get(*key)
                .cloned()
                .unwrap_or_else(|| default.to_string())),
        );
    }
    for (key, default) in NUMBER_FIELDS {
        let value = fields
            .get(*key)
            .and_then(|v| v.parse::<i64>().ok())
            .unwrap_or(*default);
        body.insert((*key).to_string(), json!(value));
    }
    for (key, default) in BOOL_FIELDS {
        let value = fields.get(*key).map(|v| v != "0").unwrap_or(*default);
        body.insert((*key).to_string(), json!(value));
    }
    // The real API key is NEVER sent to the frontend (security: even a password field's
    // `$0.value` is readable by any browser extension / console). The frontend only gets
    // a boolean so it can show "已设置" vs. the empty input.
    let key_set = fields
        .get("llm_api_key")
        .map(|s| !s.trim().is_empty())
        .unwrap_or(false)
        || std::env::var("DEEPSEEK_API_KEY")
            .map(|s| !s.trim().is_empty())
            .unwrap_or(false);
    body.insert("llm_api_key_set".to_string(), json!(key_set));

    axum::Json(Value::Object(body)).into_response()
}

/// Fields `PUT /settings` refuses to accept from an API-token-authenticated request (any role) —
/// "账号安全类" danger: toggling password protection off, toggling No-Fun mode, or widening the
/// login-session lifetimes are all things that let whoever holds a token entrench or extend its
/// own access, exactly what a stolen-but-scoped token must not be able to do. A narrower,
/// per-field check rather than a whole-route `require_session` (unlike `/settings/password` or
/// `/database/drop`) because this one endpoint also carries many harmless fields
/// (`theme`/`motd`/`pagesize`/...) an otherwise-trusted admin-role token should still be able to
/// update.
const TOKEN_AUTH_FORBIDDEN_SETTINGS_FIELDS: &[&str] = &[
    "enablepass",
    "nofunmode",
    "access_token_lifetime_secs",
    "refresh_token_lifetime_secs",
];

async fn put_settings(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let Value::Object(fields) = body else {
        return error(
            StatusCode::BAD_REQUEST,
            "put_settings",
            "Expected a JSON object.",
        );
    };

    if auth
        .as_ref()
        .is_some_and(|axum::extract::Extension(a)| a.is_token())
    {
        if let Some(field) = TOKEN_AUTH_FORBIDDEN_SETTINGS_FIELDS
            .iter()
            .find(|f| fields.contains_key(**f))
        {
            return error(
                StatusCode::FORBIDDEN,
                "put_settings",
                format!(
                    "API tokens cannot change \"{field}\" — this requires a real login session."
                ),
            );
        }
    }

    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "put_settings",
                e.to_string(),
            )
        }
    };

    // Captured before `fields` is consumed by the write loop below — used afterwards to decide
    // whether this request actually flips the thumbnail format (`enablewebp`), which needs a full
    // regen so the library stays in one uniform format rather than a jpg/webp mix. A quality-only
    // change (`webpquality`/`hqthumbpages`) intentionally does *not* trigger this — it only
    // affects thumbnails generated from here on.
    let new_enablewebp = fields.get("enablewebp").and_then(Value::as_bool);
    let previous_enablewebp = conn
        .hget::<_, _, Option<String>>(CONFIG_KEY, "enablewebp")
        .await
        .ok()
        .flatten()
        .map(|v| v != "0")
        .unwrap_or(true);

    // Same "captured before the write loop" pattern as `enablewebp` above — a tier change can't
    // be applied retroactively by `precompute_one`'s incremental one-way backfill (it only ever
    // widens/narrows the *one* archive it's called for), so it needs its own full rebuild,
    // exactly like flipping `enablewebp` needs a full thumbnail regen rather than only affecting
    // thumbnails generated from here on.
    let new_recommend_precision = fields
        .get("recommendprecision")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    let previous_recommend_precision: Option<String> = conn
        .hget(CONFIG_KEY, "recommendprecision")
        .await
        .ok()
        .flatten();

    // Same "captured before the write loop" pattern again — `artist_backfill.rs`'s per-archive
    // ingest hook only ever reaches archives ingested *after* an LLM key exists; a key added days
    // into using the app leaves every already-ingested archive permanently unbackfilled unless
    // this transition itself triggers a retroactive full-library sweep (see
    // `artist_backfill::spawn_full_backfill_job`'s own docs for why this can't just piggyback on
    // the recommend-cache's own startup-only backfill). Only the unset→set transition matters —
    // a set→different-set key change (rotating keys) or set→unset (removing the key) has no
    // reason to re-sweep the whole library.
    let new_llm_api_key = fields
        .get("llm_api_key")
        .and_then(Value::as_str)
        .map(|s| s.to_string());
    let had_llm_api_key_before = conn
        .hget::<_, _, Option<String>>(CONFIG_KEY, "llm_api_key")
        .await
        .ok()
        .flatten()
        .is_some_and(|s| !s.trim().is_empty());

    // Snapshotted before the write loop consumes `fields` — never includes `password`/
    // `session_secret` (both `continue`d past below without being written at all here, so there's
    // nothing meaningful to record for them anyway; `password` changes go through
    // `change_password`'s own separate audit entry instead). Only the *names* of changed fields
    // are recorded, not their values — settings cover everything from a `motd` string to API
    // token lifetimes, and logging arbitrary field values here would risk incidentally recording
    // something sensitive a future field addition didn't anticipate.
    let changed_fields: Vec<String> = fields
        .keys()
        .filter(|k| *k != "password" && *k != "session_secret")
        .cloned()
        .collect();

    for (key, value) in fields {
        if key == "password" || key == "session_secret" {
            // `password` has its own endpoint (needs hashing); `session_secret` is internal-only.
            continue;
        }
        let stored = match &value {
            Value::String(s) => s.clone(),
            Value::Bool(b) => if *b { "1" } else { "0" }.to_string(),
            Value::Number(n) => n.to_string(),
            _ => continue,
        };
        let _: () = match conn.hset(CONFIG_KEY, &key, stored).await {
            Ok(v) => v,
            Err(e) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "put_settings",
                    e.to_string(),
                )
            }
        };
    }

    if !changed_fields.is_empty() {
        crate::activity::record_manual(
            &state,
            auth.as_ref().map(|axum::extract::Extension(a)| a),
            lanrurugi_storage::activity::action_types::SETTINGS_UPDATE,
            lanrurugi_storage::activity::ActivityTarget {
                id: None,
                label: None,
                kind: Some("settings".to_string()),
            },
            None,
            Some(json!({ "changed_fields": changed_fields })),
        )
        .await;
    }

    if new_enablewebp.is_some_and(|v| v != previous_enablewebp) {
        let thumb_settings = lanrurugi_scanner::thumbnail::read_settings(&mut conn).await;
        if let Ok(archives) = state.repos.archives.list_all().await {
            crate::archives::spawn_regen_thumbnails_job(&state, archives, thumb_settings, true)
                .await;
        }
    }

    if new_recommend_precision.is_some_and(|v| Some(&v) != previous_recommend_precision.as_ref()) {
        // Avoid double-queueing if a rebuild (from this same tier change, or a still-running
        // first-time backfill) is already in flight — matches the "check `by_name` before
        // spawning" guard `duplicates.rs`'s own job-creation callers use elsewhere.
        let already_running = state
            .jobs
            .by_name("recommend_precompute")
            .await
            .iter()
            .any(|j| {
                matches!(
                    j.state,
                    lanrurugi_core::jobs::JobState::Queued | lanrurugi_core::jobs::JobState::Active
                )
            });
        if !already_running {
            if let Err(e) = state.recommend_cache.bump_rebuild_generation().await {
                tracing::warn!(error = %e, "failed to bump recommend-cache rebuild generation");
            }
            crate::recommend_precompute::spawn_full_precompute_job(
                &state,
                "precision tier changed",
            )
            .await;
        }
    }

    if !had_llm_api_key_before && new_llm_api_key.is_some_and(|k| !k.trim().is_empty()) {
        // Same "check `by_name` before spawning" double-queue guard as the recommend-precision
        // branch above.
        let already_running = state.jobs.by_name("artist_backfill").await.iter().any(|j| {
            matches!(
                j.state,
                lanrurugi_core::jobs::JobState::Queued | lanrurugi_core::jobs::JobState::Active
            )
        });
        if !already_running {
            crate::artist_backfill::spawn_full_backfill_job(&state, "LLM key configured").await;
        }
    }

    axum::Json(json!({ "operation": "put_settings", "success": 1 })).into_response()
}

#[derive(Deserialize)]
struct ChangePasswordForm {
    password: String,
}

/// Sets a new admin password, hashed the same way legacy stores one (`{CRYPT}$2a$...` — see
/// `lanrurugi_core::password`) so a legacy instance sharing this Redis keeps accepting it too. An
/// empty string clears password protection's practical effect the same way legacy's own
/// config-page "leave blank to keep current password" convention does *not* work here — this
/// endpoint always sets whatever was submitted, so the frontend must not submit an empty field
/// unless the user explicitly means to set an empty password.
async fn change_password(
    State(state): State<AppState>,
    auth: Option<axum::extract::Extension<crate::auth_context::AuthContext>>,
    axum::Form(form): axum::Form<ChangePasswordForm>,
) -> Response {
    let hashed = match password::hash_password(&form.password) {
        Ok(h) => h,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "change_password",
                e.to_string(),
            )
        }
    };

    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "change_password",
                e.to_string(),
            )
        }
    };
    let result: Result<(), _> = conn.hset(CONFIG_KEY, "password", hashed).await;
    match result {
        Ok(()) => {
            // Never records the password itself, before or after — only that a change happened.
            crate::activity::record_manual(
                &state,
                auth.as_ref().map(|axum::extract::Extension(a)| a),
                lanrurugi_storage::activity::action_types::SETTINGS_PASSWORD_CHANGE,
                lanrurugi_storage::activity::ActivityTarget {
                    id: None,
                    label: None,
                    kind: Some("settings".to_string()),
                },
                None,
                None,
            )
            .await;
            axum::Json(json!({ "operation": "change_password", "success": 1 })).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "change_password",
            e.to_string(),
        ),
    }
}
