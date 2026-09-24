//! `enablecors` (Settings page's "Enable CORS for the Client API") — verified against legacy's
//! real implementation (`~/LANraragi/lib/LANraragi/Controller/Login.pm::setup_cors`, gated by
//! `Utils/Routing.pm`'s `if ($self->LRR_CONF->enable_cors)` around the whole `/api` route group).
//! This was a real Phase 1 gap (issue #85): the Settings-page checkbox and Redis field existed,
//! but nothing ever mounted a CORS layer, so toggling it on had zero effect.
//!
//! A runtime-toggleable middleware (not `tower_http::cors::CorsLayer`, which bakes its policy in
//! at router-build time) since `enablecors` is read live from Redis on every request, exactly like
//! every other Settings-page value this project already treats as "changes take effect on the
//! very next request, no restart" (see `auth.rs`'s own module docs on the same convention).
//!
//! Applied as the *outermost* layer on `/api/*` (`app.rs`), specifically *before*
//! `procedure::require_api_key` — a real CORS preflight (`OPTIONS`) request never carries
//! credentials a browser would consider safe to attach cross-origin, so it must never reach the
//! auth check at all; legacy's own routing has the identical ordering concern, resolved by
//! `logged_in_api`'s own explicit `return 1 if $self->req->method eq 'OPTIONS'` carve-out.

use axum::extract::{Request, State};
use axum::http::{HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use deadpool_redis::redis::AsyncCommands;

use crate::AppState;
use lanrurugi_storage::keys::CONFIG_KEY;

const ALLOW_ORIGIN: &str = "*";
const ALLOW_METHODS: &str = "GET, OPTIONS, POST, DELETE, PUT";
const ALLOW_HEADERS: &str = "Authorization";

async fn cors_enabled(state: &AppState) -> bool {
    let Ok(mut conn) = state.redis.config.get().await else {
        return false;
    };
    let value: Option<String> = conn.hget(CONFIG_KEY, "enablecors").await.unwrap_or(None);
    value.map(|v| v != "0").unwrap_or(false)
}

pub async fn apply_cors(State(state): State<AppState>, request: Request, next: Next) -> Response {
    if !cors_enabled(&state).await {
        return next.run(request).await;
    }

    // A real preflight never reaches the handler (or `require_api_key`) at all — nothing past
    // this middleware knows how to answer an `OPTIONS` request for any real route, and legacy's
    // own OpenAPI plugin only ever uses it to answer the CORS handshake, never real work.
    if request.method() == Method::OPTIONS {
        return with_cors_headers((StatusCode::NO_CONTENT, ()).into_response());
    }

    with_cors_headers(next.run(request).await)
}

fn with_cors_headers(mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(
        "Access-Control-Allow-Origin",
        HeaderValue::from_static(ALLOW_ORIGIN),
    );
    headers.insert(
        "Access-Control-Allow-Methods",
        HeaderValue::from_static(ALLOW_METHODS),
    );
    headers.insert(
        "Access-Control-Allow-Headers",
        HeaderValue::from_static(ALLOW_HEADERS),
    );
    response
}
