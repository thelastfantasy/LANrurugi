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
pub(crate) const DEFAULT_SIZE_THRESHOLD: i64 = 1000;
pub(crate) const DEFAULT_READER_QUALITY: i64 = 50;
pub(crate) const DEFAULT_WEBP_QUALITY: i64 = 85;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/settings", get(get_settings).put(put_settings))
        .route("/settings/password", post(change_password))
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

/// `(field, default)` pairs for every `LRR_CONFIG` value the Settings page's Global/Security/
/// Files/Tags sections read or write, verified against `Model/Config.pm`'s `get_redis_conf`
/// calls. Excludes `password` (see module docs) and `theme` (already had its own default before
/// this table existed, kept as a named constant below for that reason).
const STRING_FIELDS: &[(&str, &str)] = &[
    ("theme", "modern.css"),
    ("language", "auto"),
    ("htmltitle", "LANrurugi"),
    ("motd", "Welcome to this Library running LANrurugi!"),
    ("apikey", ""),
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
];

const NUMBER_FIELDS: &[(&str, i64)] = &[
    ("pagesize", DEFAULT_PAGE_SIZE),
    ("tempmaxsize", 500),
    ("sizethreshold", DEFAULT_SIZE_THRESHOLD),
    ("readerquality", DEFAULT_READER_QUALITY),
    ("webpquality", DEFAULT_WEBP_QUALITY),
];

const BOOL_FIELDS: &[(&str, bool)] = &[
    ("enablepass", true),
    ("nofunmode", false),
    ("enablecors", false),
    ("localprogress", false),
    ("authprogress", false),
    ("enableresize", false),
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

    axum::Json(Value::Object(body)).into_response()
}

async fn put_settings(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<Value>,
) -> Response {
    let Value::Object(fields) = body else {
        return error(
            StatusCode::BAD_REQUEST,
            "put_settings",
            "Expected a JSON object.",
        );
    };

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

    if new_enablewebp.is_some_and(|v| v != previous_enablewebp) {
        let thumb_settings = lanrurugi_scanner::thumbnail::read_settings(&mut conn).await;
        if let Ok(archives) = state.repos.archives.list_all().await {
            crate::archives::spawn_regen_thumbnails_job(&state, archives, thumb_settings, true)
                .await;
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
            axum::Json(json!({ "operation": "change_password", "success": 1 })).into_response()
        }
        Err(e) => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "change_password",
            e.to_string(),
        ),
    }
}
