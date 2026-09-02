//! `misc` endpoint group. `GET /info` shape verified against `~/LANraragi/tools/openapi.yaml`'s
//! `ServerInfo` schema (constitution Principle II). Field values read live from the same
//! `LRR_CONFIG` hash legacy itself reads (`Controller/Api/Other.pm::serve_serverinfo`, verified
//! field-by-field), so a value already set through legacy's settings page is reflected correctly
//! here with no migration step (Principle I).

use std::collections::HashMap;

use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use deadpool_redis::redis::AsyncCommands;
use serde_json::json;

use crate::AppState;
use lanrurugi_storage::keys::{CONFIG_KEY, TOTAL_PAGES_STAT_KEY};

pub fn router() -> Router<AppState> {
    Router::new().route("/llm/key-status", get(llm_key_status))
}

/// Deliberately separate from [`router`] and merged unprotected in `lanrurugi-server`'s
/// `build_app` (same pattern as `settings::public_router`/`login::router`) — issue #92:
/// `Footer.tsx` calls `useServerInfo()` and `UpdateBanner.tsx` calls the public `/api/version`
/// endpoint unconditionally on every page, including ones that must render *before* a session
/// exists (a visitor who isn't logged in hitting the new 404 catch-all route, or `/stats`, which
/// legacy itself lets an anonymous visitor view — `Layout.tsx`'s own anonymous nav still links to
/// it). With `/info` behind `require_api_key`, that call 401'd and `client.ts`'s own
/// `redirectToLogin()` force-navigated away from whatever page the visitor was actually looking
/// at — live-reported as "opening an unknown URL while logged out double-navigates: the 404 page
/// flashes, then it jumps to /login anyway", which defeated issue #92's own "stay on the
/// page/URL, don't redirect" requirement. `server_info`'s own payload (library name/MOTD/version/
/// archive count/page-size settings) has no secret in it — no API key, no password hash, nothing
/// session-scoped — so exposing it to an anonymous caller is the same class of decision
/// `settings::public_router`'s own `/theme` already made, not a new precedent. The same reasoning
/// applies to `version::public_router`'s `/api/version`.
pub fn public_router() -> Router<AppState> {
    Router::new().route("/info", get(server_info))
}

async fn server_info(State(state): State<AppState>) -> Response {
    let total_archives = state
        .repos
        .archives
        .list_ids()
        .await
        .map(|ids| ids.len())
        .unwrap_or(0);

    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            return crate::common::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "get_info",
                e.to_string(),
            )
        }
    };
    let fields: HashMap<String, String> = conn.hgetall(CONFIG_KEY).await.unwrap_or_default();
    let total_pages_read: i64 = conn.get(TOTAL_PAGES_STAT_KEY).await.unwrap_or(0);

    let field = |key: &str, default: &str| fields.get(key).cloned().unwrap_or(default.to_string());
    let flag = |key: &str, default: &str| field(key, default) != "0";

    let excluded_namespaces: Vec<String> = field("excludednamespaces", "source, date_added")
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();

    axum::Json(json!({
        "name": field("htmltitle", "LANrurugi"),
        "motd": field("motd", "Welcome to this Library running LANrurugi!"),
        "version": env!("CARGO_PKG_VERSION"),
        "version_name": "",
        // Legacy's own tagline (`~/LANraragi/package.json`'s `description`), verbatim — the
        // footer (`Footer.tsx`) prints this above "Powered by LANraragi." exactly like legacy's
        // own `footer.html.tt2` does with its `descstr`. Purely cosmetic (no version semantics),
        // so reused as-is rather than invented fresh for this rewrite.
        "version_desc": "I'm under Japanese influence and my honor's at stake!",
        // Password protection can no longer be disabled (007-guest-restricted-access) — always
        // `true`, not a Redis-configurable value anymore. `nofun_mode` is removed entirely (its
        // own concept, forcing login even when password protection was otherwise off, no longer
        // applies once password protection can't be off in the first place) — a documented,
        // spec-mandated Constitution Principle II exception (research.md §5), not an oversight.
        "has_password": true,
        // Reflects the deploy-time --disable-update-check / LANRURUGI_DISABLE_UPDATE_CHECK flag
        // (AppState::disable_update_check) instead of the removed `devmode` Settings-page toggle,
        // which had zero server-side behavior of its own — see that field's own docs.
        "debug_mode": state.disable_update_check,
        "archives_per_page": field("pagesize", "100").parse::<u32>().unwrap_or(100),
        "server_resizes_images": flag("enableresize", "0"),
        // Legacy inverts this one: `server_tracks_progress => enable_localprogress ? \0 : \1`.
        "server_tracks_progress": !flag("localprogress", "0"),
        "authenticated_progress": flag("authprogress", "0"),
        "total_pages_read": total_pages_read,
        "total_archives": total_archives,
        "cache_last_cleared": 0,
        "excluded_namespaces": excluded_namespaces,
    }))
    .into_response()
}

/// `GET /api/llm/key-status` — returns `{ configured: true/false }`.
async fn llm_key_status(State(state): State<AppState>) -> Response {
    let mut conn = match state.redis.config.get().await {
        Ok(c) => c,
        Err(e) => {
            return crate::common::error(
                axum::http::StatusCode::INTERNAL_SERVER_ERROR,
                "llm_key_status",
                e.to_string(),
            )
        }
    };
    let fields: std::collections::HashMap<String, String> = conn
        .hgetall(lanrurugi_storage::keys::CONFIG_KEY)
        .await
        .unwrap_or_default();
    let configured = fields
        .get("llm_api_key")
        .map(|v| !v.is_empty())
        .unwrap_or(false);
    axum::Json(serde_json::json!({ "configured": configured })).into_response()
}
