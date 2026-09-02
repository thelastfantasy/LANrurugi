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
    // `/settings/password` ("账号安全类" danger) — no API token, admin-role or not, may change the
    // admin password; only a real session cookie can. Enforced by `require_api_key` itself
    // (issue #91's `route_policy.csv` `deny` rule for `token_admin`/`token_guest` against this
    // exact route) — see `procedure.rs`'s own module docs for why this used to be a separate
    // `route_layer`-gated sub-router and no longer is. `PUT /settings` itself stays outside this
    // rule — see that handler's own docs for why its dangerous fields need a narrower, field-level
    // check instead.
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

/// Same shape as [`fetch_theme`], for the guest-only `guest_theme` field (see its own doc in
/// [`STRING_FIELDS`]).
async fn fetch_guest_theme(state: &AppState) -> Option<String> {
    let mut conn = state.redis.config.get().await.ok()?;
    let theme: Option<String> = conn.hget(CONFIG_KEY, "guest_theme").await.ok()?;
    Some(theme.unwrap_or_else(|| "ex.css".to_string()))
}

/// Used by `lanrurugi-server`'s own `serve_index` handler (the `index.html`-serving SPA fallback),
/// which substitutes this value directly into that file's inline anti-flash-of-default-theme
/// `<script>` body via a plain string replace — not an HTML/JS-aware templating step — so an
/// unconstrained value would be a real reflected-script-injection vector for anyone able to write
/// an arbitrary string to `LRR_CONFIG`'s `theme` field. `PUT /settings` itself now also validates
/// `theme` against this exact same `KNOWN_THEME_FILES` list before ever writing it (issue #65's
/// `validate_setting_field`), so this filter here is defense-in-depth against a value written some
/// other way (directly in Redis, a legacy instance sharing the same database, etc.) rather than the
/// only thing standing between a bad value and this `<script>` tag. `None` whenever `fetch_theme`
/// itself fails *or* the stored value isn't recognized — both already mean "fall back to
/// `serve_index`'s own built-in default", so this doesn't distinguish them either. `path` is the
/// SPA route being served; `/login` bypasses the guest-eligible branch and always reads the admin
/// theme, so the password screen never flashes/settles on the guest theme.
pub async fn fetch_theme_for_html_injection(
    state: &AppState,
    headers: &axum::http::HeaderMap,
    path: &str,
) -> Option<String> {
    // The Login page is an admin surface, not a guest browsing surface — it should never flash or
    // settle on the guest theme, even when guest mode is on. Every other SPA route keeps the
    // existing guest-eligible behavior (`theme` = guest theme for an eligible guest request).
    let theme = if path == "/login" || path == "/login/" {
        fetch_theme(state).await
    } else if is_guest_eligible_request(state, headers).await {
        fetch_guest_theme(state).await
    } else {
        fetch_theme(state).await
    };
    theme.filter(|theme| KNOWN_THEME_FILES.contains(&theme.as_str()))
}

/// Raw `language` field lookup, same shape as [`fetch_theme`] — `None` only on an actual Redis
/// failure, defaulting to `"auto"` (matching `STRING_FIELDS`'s own default) whenever the field
/// itself was never set.
async fn fetch_language(state: &AppState) -> Option<String> {
    let mut conn = state.redis.config.get().await.ok()?;
    let language: Option<String> = conn.hget(CONFIG_KEY, "language").await.ok()?;
    Some(language.unwrap_or_else(|| "auto".to_string()))
}

/// issue #92: this response also carries `language` now, not just `theme` — `useApplyTheme`/
/// `useApplySettingsLanguage` (`apps/frontend/src/theme.ts`/`i18n/index.ts`) both used to read
/// their respective field off the full, auth-gated `GET /settings` (which 401s pre-login, since
/// that response also carries the API key and other genuinely secret fields that can't be made
/// public) — every page rendered before a session exists (the Login page's own bespoke
/// `usePublicTheme` fallback aside) hit that 401, and the *global* 401 handler in `client.ts`
/// force-navigates to `/login` on every single one regardless of whether the calling hook itself
/// already had a fallback ready, defeating issue #92's own "stay on the page, don't redirect"
/// requirement for anything rendered while logged out (the new 404 page very much included —
/// live-reported as a real double-navigation: the 404 content flashes, then `/login` anyway).
/// `theme`/`language` are the only two fields anything needs before a session exists, and neither
/// is remotely secret, so folding `language` into this already-public endpoint is simpler than
/// standing up a whole second public settings surface for one more field. The response also
/// carries `admin_theme` — always the administrator's own `theme`, regardless of guest-eligibility
/// — so the Login page can render with the admin theme even when guest mode is on and the guest
/// theme is a different one.
async fn get_theme(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
    let admin_theme = fetch_theme(&state).await;
    let theme = if is_guest_eligible_request(&state, &headers).await {
        fetch_guest_theme(&state).await
    } else {
        admin_theme.clone()
    };
    let language = fetch_language(&state).await;
    match (theme, admin_theme, language) {
        (Some(theme), Some(admin_theme), Some(language)) => {
            axum::Json(json!({ "theme": theme, "admin_theme": admin_theme, "language": language }))
                .into_response()
        }
        _ => error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "get_theme",
            "failed to reach redis".to_string(),
        ),
    }
}

/// Mirrors `procedure::require_api_key`'s own "is this request eligible for `AuthMethod::
/// GuestVisitor`" branch (session invalid, `guestmode` on, at least one archive actually guest-
/// visible), but standalone — `GET /theme` sits in `public_router()`, entirely outside that
/// middleware (it must work with no session at all, e.g. on the Login page itself), so it can't
/// read an `AuthContext` the middleware never ran to produce. A request carrying *any* bearer
/// token is treated as non-guest without validating the token itself: a soon-to-be-authenticated
/// client asking "what theme should I render right now" should see the admin theme it's about to
/// actually use, not flicker through the guest one first, and an invalid token is about to 401 on
/// its very next real request regardless of what this endpoint answers.
async fn is_guest_eligible_request(state: &AppState, headers: &axum::http::HeaderMap) -> bool {
    if headers.contains_key(axum::http::header::AUTHORIZATION) {
        return false;
    }
    let cfg = match crate::auth::load(state).await {
        Ok(cfg) => cfg,
        Err(_) => return false,
    };
    if crate::auth::session_is_valid(&cfg, headers) {
        return false;
    }
    if !cfg.guest_mode_enabled {
        return false;
    }
    crate::search::guest_has_any_visible_archive(state)
        .await
        .unwrap_or(false)
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

/// issue #97: whether placing a stamp on a not-yet-bookmarked page should also bookmark it —
/// read by `stamps.rs::add_stamp`. Same shape as [`read_new_badge_mode`] above.
pub(crate) async fn read_stamp_autobookmark(state: &AppState) -> bool {
    match state.redis.config.get().await {
        Ok(mut conn) => conn
            .hget::<_, _, Option<String>>(CONFIG_KEY, "stampautobookmark")
            .await
            .ok()
            .flatten()
            .map(|v| v != "0")
            .unwrap_or(true),
        Err(_) => true,
    }
}

/// issue #97: whether removing the last stamp on a page should also remove that page's bookmark
/// — read by `stamps.rs::delete_stamp`. Only meaningful when [`read_stamp_autobookmark`] is also
/// true, but kept as an independent field/read rather than nested inside it, matching how the
/// frontend also treats them as two independently-toggleable settings (the sub-option is just
/// disabled, not hidden, when the parent is off).
pub(crate) async fn read_stamp_autounbookmark(state: &AppState) -> bool {
    match state.redis.config.get().await {
        Ok(mut conn) => conn
            .hget::<_, _, Option<String>>(CONFIG_KEY, "stampautounbookmark")
            .await
            .ok()
            .flatten()
            .map(|v| v != "0")
            .unwrap_or(true),
        Err(_) => true,
    }
}

/// `(field, default)` pairs for every `LRR_CONFIG` value the Settings page's Global/Security/
/// Files/Tags sections read or write, verified against `Model/Config.pm`'s `get_redis_conf`
/// calls. Excludes `password` (see module docs) and `theme` (already had its own default before
/// this table existed, kept as a named constant below for that reason).
const STRING_FIELDS: &[(&str, &str)] = &[
    ("theme", "modern.css"),
    // 007-guest-restricted-access follow-up: the theme an unauthenticated `guest_visitor` sees,
    // independent of whatever theme the admin has picked for their own session — `get_theme`
    // resolves which of the two to serve based on whether the current request is actually guest-
    // eligible (`crate::auth::session_is_valid` + `guestmode` + `guest_has_any_visible_archive`),
    // not on the client's own say-so. Defaults to Sad Panda (`ex.css`), not the admin default —
    // deliberately a different visual identity so a guest browsing session is never confusable
    // with an admin one at a glance.
    ("guest_theme", "ex.css"),
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
    // 007-guest-restricted-access: the site-wide guest-mode master switch. On its own it grants
    // nothing — `procedure::require_api_key`'s guest-eligibility branch also requires at least one
    // `Category.visible_to_guest` before treating an unauthenticated request as
    // `AuthMethod::GuestVisitor` (spec FR-005/FR-006). Replaces the removed `enablepass`/
    // `nofunmode` (password login is now unconditional, with no setting able to disable it) and
    // `devmode` (had zero server-side behavior of its own — see `main.rs`'s
    // `--disable-update-check` flag, which replaces the one real thing it controlled).
    ("guestmode", false),
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
    // issue #97: placing a stamp on a page auto-bookmarks it; removing a page's last remaining
    // stamp auto-removes that page's bookmark (only when both this and `stampautounbookmark` are
    // on — see `stamps.rs::add_stamp`/`delete_stamp`). Both default to `true`.
    ("stampautobookmark", true),
    ("stampautounbookmark", true),
    // `Model/Config.pm::can_replacetitles` — gates whether a metadata plugin's returned `title`
    // is actually applied to an archive (vs. only its tags/summary). Rendered on the Plugins page
    // itself (`~/LANraragi/templates/plugins.html.tt2:84-91`), not the Settings page, even though
    // it's stored in the same `LRR_CONFIG` hash — `Controller/Plugins.pm:31,101-102` reads/writes
    // it directly rather than going through `Config.pm`'s settings-page field list.
    ("replacetitles", true),
];

/// Checks a single `PUT /settings` field against the `STRING_FIELDS`/`NUMBER_FIELDS`/
/// `BOOL_FIELDS` allowlist plus the `theme` field's own extra value check, returning the string
/// form to `HSET` into `LRR_CONFIG` on success or a caller-facing error message on failure. Pulled
/// out of `put_settings`'s write loop as a plain, `Redis`-free function so the field/value matrix
/// (unknown key, wrong JSON type, invalid `theme` value) can be unit-tested directly rather than
/// only reachable through a real HTTP request against a live Redis (issue #65).
fn validate_setting_field(key: &str, value: &Value) -> Result<String, String> {
    // `llm_api_key` is deliberately absent from `STRING_FIELDS` — that list doubles as
    // `get_settings`'s own read-loop allowlist, and the real key must never be echoed back to the
    // frontend (only the `llm_api_key_set` boolean is, computed separately above). It still needs
    // its own write-side allowlist entry here, though — omitting it entirely meant every `PUT
    // /settings` call carrying a real key was rejected outright with "Unknown settings field"
    // (issue #65's allowlist rejected anything not in `STRING_FIELDS`/`NUMBER_FIELDS`/
    // `BOOL_FIELDS`, and this field was never added to any of them) — the Settings page's own "保存
    // Key" action has never actually persisted a key since that allowlist was introduced, confirmed
    // live via a direct `PUT /settings` call, 2026-08-26.
    let is_known_field = key == "llm_api_key"
        || STRING_FIELDS.iter().any(|(k, _)| *k == key)
        || NUMBER_FIELDS.iter().any(|(k, _)| *k == key)
        || BOOL_FIELDS.iter().any(|(k, _)| *k == key);
    if !is_known_field {
        return Err(format!("Unknown settings field: \"{key}\"."));
    }
    if key == "theme" || key == "guest_theme" {
        let is_valid_theme = value
            .as_str()
            .is_some_and(|s| KNOWN_THEME_FILES.contains(&s));
        if !is_valid_theme {
            return Err(format!(
                "Invalid theme: {value}. Must be one of {KNOWN_THEME_FILES:?}."
            ));
        }
    }
    match value {
        Value::String(s) => Ok(s.clone()),
        Value::Bool(b) => Ok(if *b { "1" } else { "0" }.to_string()),
        Value::Number(n) => Ok(n.to_string()),
        _ => Err(format!(
            "Field \"{key}\" must be a string, bool, or number."
        )),
    }
}

/// The only `STRING_FIELDS` entries a `guest_visitor` may see — everything else in the full
/// payload (`pagesize`, `access_token_lifetime_secs`, `tagrules`, `llm_api_key_set`, etc.) is
/// admin-session configuration a guest has no business reading, even though `route_policy.csv`
/// allows the *route* itself (guest needs *some* fields off it — `htmltitle`/`timezone` for the
/// Library page it's allowed into — just not the full admin payload). `theme` is deliberately
/// absent here: a guest's `theme` value is `guest_theme`'s, substituted in below, not the literal
/// `STRING_FIELDS` `theme` entry (that's the admin's own pick).
const GUEST_VISIBLE_STRING_FIELDS: &[&str] = &["language", "htmltitle", "timezone"];

async fn get_settings(State(state): State<AppState>, headers: axum::http::HeaderMap) -> Response {
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

    // A `guest_visitor` reaches this route too (`route_policy.csv` allows it — the Library page
    // it's allowed into needs a handful of these fields), but must never see the full admin
    // payload: that's real admin-session configuration (page size, token lifetimes, tag rules,
    // whether an LLM key is set, ...), and — the concrete bug this branch fixes — the admin's own
    // `theme` pick, which a guest seeing even momentarily is a real, live-reproduced state leak
    // (flashes the correct guest theme, then snaps to the admin's, because the guest's `useSettings`
    // query and the admin's share one `["settings"]` cache key once this response answers it).
    if is_guest_eligible_request(&state, &headers).await {
        let mut body = serde_json::Map::new();
        for key in GUEST_VISIBLE_STRING_FIELDS {
            let default = STRING_FIELDS
                .iter()
                .find(|(k, _)| k == key)
                .map(|(_, default)| *default)
                .expect("every GUEST_VISIBLE_STRING_FIELDS entry is a real STRING_FIELDS key");
            body.insert(
                (*key).to_string(),
                json!(fields
                    .get(*key)
                    .cloned()
                    .unwrap_or_else(|| default.to_string())),
            );
        }
        let guest_theme = fields
            .get("guest_theme")
            .cloned()
            .unwrap_or_else(|| "ex.css".to_string());
        body.insert("theme".to_string(), json!(guest_theme));
        return axum::Json(Value::Object(body)).into_response();
    }

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
/// "账号安全类" danger: widening the login-session lifetimes, or (007-guest-restricted-access)
/// turning on site-wide guest mode, are things that let whoever holds a token entrench its own
/// access or expose content to unauthenticated visitors — exactly what a stolen-but-scoped token
/// must not be able to do. A narrower, per-field check rather than a whole-route `require_session`
/// (unlike `/settings/password` or `/database/drop`) because this one endpoint also carries many
/// harmless fields (`theme`/`motd`/`pagesize`/...) an otherwise-trusted admin-role token should
/// still be able to update. `enablepass`/`nofunmode` are gone from this list along with the
/// settings themselves — password login can no longer be disabled by anyone, token or not.
const TOKEN_AUTH_FORBIDDEN_SETTINGS_FIELDS: &[&str] = &[
    "guestmode",
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

    // Only the *names* of changed fields are recorded, not their values — settings cover
    // everything from a `motd` string to API token lifetimes, and logging arbitrary field values
    // here would risk incidentally recording something sensitive a future field addition didn't
    // anticipate. Populated inside the write loop below (not snapshotted from `fields` up front)
    // now that unknown/invalid keys are rejected before ever reaching Redis — see the allowlist
    // check immediately below.
    let mut changed_fields: Vec<String> = Vec::new();

    for (key, value) in fields {
        if key == "password" || key == "session_secret" {
            // `password` has its own endpoint (needs hashing); `session_secret` is internal-only.
            continue;
        }
        // Issue #65: `put_settings` used to write *any* key whose value happened to be a
        // string/bool/number straight into `LRR_CONFIG`, with no check against the known
        // `STRING_FIELDS`/`NUMBER_FIELDS`/`BOOL_FIELDS` sets this same file already declares (and
        // already uses to build `get_settings`'s own response) — an unrecognized key is now
        // rejected outright rather than silently accepted, so the allowlist this file already
        // maintains actually gates writes, not just reads. See `validate_setting_field`'s own
        // tests for the field-type/theme-value matrix this checks.
        let stored = match validate_setting_field(&key, &value) {
            Ok(s) => s,
            Err(msg) => return error(StatusCode::BAD_REQUEST, "put_settings", msg),
        };
        changed_fields.push(key.clone());
        let _: () = match conn.hset(CONFIG_KEY, &key, stored).await {
            Ok(v) => v,
            Err(e) => {
                // Validation already passed for `key` — this is a real attempted write that
                // failed at the Redis `HSET` itself, unlike the unknown-field/bad-type rejections
                // above which never got this far.
                crate::activity::record_manual(
                    &state,
                    auth.as_ref().map(|axum::extract::Extension(a)| a),
                    lanrurugi_storage::activity::action_types::SETTINGS_UPDATE,
                    lanrurugi_storage::activity::ActivityTarget {
                        id: None,
                        label: Some(key.clone()),
                        kind: Some("settings".to_string()),
                    },
                    lanrurugi_storage::activity::Outcome::Failure {
                        reason: e.to_string(),
                    },
                    None,
                    None,
                )
                .await;
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "put_settings",
                    e.to_string(),
                );
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
            lanrurugi_storage::activity::Outcome::Success,
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
                lanrurugi_storage::activity::Outcome::Success,
                None,
                None,
            )
            .await;
            axum::Json(json!({ "operation": "change_password", "success": 1 })).into_response()
        }
        Err(e) => {
            // A password change was genuinely attempted (hashing already succeeded) but the
            // Redis write itself failed — worth recording, especially since this is a
            // security-sensitive action.
            crate::activity::record_manual(
                &state,
                auth.as_ref().map(|axum::extract::Extension(a)| a),
                lanrurugi_storage::activity::action_types::SETTINGS_PASSWORD_CHANGE,
                lanrurugi_storage::activity::ActivityTarget {
                    id: None,
                    label: None,
                    kind: Some("settings".to_string()),
                },
                lanrurugi_storage::activity::Outcome::Failure {
                    reason: e.to_string(),
                },
                None,
                None,
            )
            .await;
            error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "change_password",
                e.to_string(),
            )
        }
    }
}

#[cfg(test)]
mod validate_setting_field_tests {
    use super::*;

    #[test]
    fn rejects_an_unknown_key() {
        let err = validate_setting_field("not_a_real_field", &json!("value")).unwrap_err();
        assert!(err.contains("Unknown settings field"));
    }

    #[test]
    fn accepts_a_known_string_field() {
        let stored = validate_setting_field("motd", &json!("Hello")).unwrap();
        assert_eq!(stored, "Hello");
    }

    /// Regression test for a real bug: `llm_api_key` was never added to `STRING_FIELDS` (kept out
    /// deliberately, since that list doubles as `get_settings`'s own read-loop allowlist and the
    /// real key must never be echoed back to the frontend), but it also needs its own write-side
    /// allowlist entry — omitting it entirely meant every `PUT /settings` call that tried to save a
    /// real key was rejected outright with "Unknown settings field", so the Settings page's own
    /// save-key action never actually persisted anything since issue #65's allowlist was introduced
    /// (confirmed live via a direct `PUT /settings` call, 2026-08-26).
    #[test]
    fn accepts_llm_api_key_despite_it_not_being_in_string_fields() {
        assert!(!STRING_FIELDS.iter().any(|(k, _)| *k == "llm_api_key"));
        let stored = validate_setting_field("llm_api_key", &json!("sk-real-key")).unwrap();
        assert_eq!(stored, "sk-real-key");
    }

    #[test]
    fn accepts_a_known_number_field() {
        let stored = validate_setting_field("pagesize", &json!(50)).unwrap();
        assert_eq!(stored, "50");
    }

    #[test]
    fn accepts_a_known_bool_field_and_normalizes_to_1_or_0() {
        assert_eq!(
            validate_setting_field("enablecors", &json!(true)).unwrap(),
            "1"
        );
        assert_eq!(
            validate_setting_field("enablecors", &json!(false)).unwrap(),
            "0"
        );
    }

    #[test]
    fn rejects_a_known_field_with_the_wrong_json_type() {
        let err = validate_setting_field("pagesize", &json!({"nested": "object"})).unwrap_err();
        assert!(err.contains("must be a string, bool, or number"));
    }

    #[test]
    fn accepts_every_real_theme_filename() {
        for theme in KNOWN_THEME_FILES {
            assert!(validate_setting_field("theme", &json!(theme)).is_ok());
        }
    }

    #[test]
    fn rejects_a_bogus_theme_value() {
        let err = validate_setting_field("theme", &json!("../../etc/passwd")).unwrap_err();
        assert!(err.contains("Invalid theme"));
    }

    #[test]
    fn rejects_a_non_string_theme_value() {
        let err = validate_setting_field("theme", &json!(42)).unwrap_err();
        assert!(err.contains("Invalid theme"));
    }
}
