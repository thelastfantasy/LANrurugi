//! Axum app skeleton and router assembly. Endpoint handlers themselves live in `lanrurugi-api`
//! (per `plan.md`'s Project Structure); this module wires that router into the process: shared
//! state, the auth middleware (only applied to `/api/*`, matching legacy's OpenAPI-scoped auth —
//! static assets and the SPA shell are not behind the API key), and tracing.
//!
//! `/api/login` and `/api/logout` are merged in *before* the auth layer is applied, so they alone
//! stay reachable without a valid key/session (see `lanrurugi_api::login`'s docs) while every
//! other `/api/*` route stays behind it.

use std::path::PathBuf;

use axum::Router;
use lanrurugi_api::AppState;
use tower_http::services::{ServeDir, ServeFile};
use tower_http::trace::TraceLayer;

use crate::middleware::auth::require_api_key;

/// `static_dir`, when present, is the built frontend (`frontend/dist` — the Docker image's
/// `LANRURUGI_STATIC_DIR`) served for every route `/api` doesn't claim, with unmatched paths
/// falling back to `index.html` (standard SPA pattern: a client-side route like `/library`
/// otherwise 404s at the file-server layer since no literal `library` file exists). `None` (the
/// local-dev default, when running the API against Vite's own separate dev server instead) means
/// this binary serves API-only — a design decision, not a missing feature.
///
/// `docs_dir`, when present, is the pre-generated plugin-authoring SDK reference (`deno doc
/// --html` output — the Docker image's `LANRURUGI_DOCS_DIR`, built fresh from
/// `crates/lanrurugi-plugin/dispatcher/{plugin-sdk,dispatcher}.ts` at image-build time by the
/// `Dockerfile`'s own `docs-builder` stage; see `mise run plugin-sdk-docs` for the same generation
/// step run locally) served under `/docs`, `nest`-ed (not the catch-all `static_dir` fallback)
/// so it's matched *before* the SPA fallback ever sees it.
pub fn build_app(
    state: AppState,
    static_dir: Option<PathBuf>,
    docs_dir: Option<PathBuf>,
) -> Router {
    let protected = lanrurugi_api::router().layer(axum::middleware::from_fn_with_state(
        state.clone(),
        require_api_key,
    ));
    let api = lanrurugi_api::login::router()
        .merge(lanrurugi_api::settings::public_router())
        .merge(protected)
        .with_state(state);

    let mut router = Router::new().nest("/api", api);

    if let Some(dir) = docs_dir {
        router = router.nest_service("/docs", ServeDir::new(dir));
    }

    if let Some(dir) = static_dir {
        // Plain `.fallback`, not `.not_found_service` — the latter forces every fallback
        // response to `404 Not Found` regardless of the file actually being served, which would
        // make a client-side route like `/library` come back as a 404-with-a-body instead of the
        // `200 OK` a bookmarked/shared SPA URL needs.
        let index_html = ServeFile::new(dir.join("index.html"));
        router = router.fallback_service(ServeDir::new(dir).fallback(index_html));
    }

    router.layer(TraceLayer::new_for_http())
}
