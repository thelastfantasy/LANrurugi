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

const CONFIG_KEY: &str = "LRR_CONFIG";

pub fn router() -> Router<AppState> {
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
    let total_pages_read: i64 = conn.get("LRR_TOTALPAGESTAT").await.unwrap_or(0);

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
        "has_password": flag("enablepass", "1"),
        "debug_mode": flag("devmode", "0"),
        "nofun_mode": flag("nofunmode", "0"),
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
