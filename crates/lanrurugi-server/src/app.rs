//! Axum app skeleton and router assembly. Endpoint handlers themselves live in `lanrurugi-api`
//! (per `plan.md`'s Project Structure); this module wires that router into the process: shared
//! state, the auth middleware (only applied to `/api/*`, matching legacy's OpenAPI-scoped auth —
//! static assets and the SPA shell are not behind the API key), and tracing.
//!
//! `/api/login` and `/api/logout` are merged in *before* the auth layer is applied, so they alone
//! stay reachable without a valid key/session (see `lanrurugi_api::login`'s docs) while every
//! other `/api/*` route stays behind it.

use std::convert::Infallible;
use std::path::PathBuf;

use axum::extract::Request;
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Router;
use lanrurugi_api::{settings::fetch_theme_for_html_injection, AppState};
use tower::service_fn;
use tower_http::services::ServeDir;
use tower_http::trace::TraceLayer;

use lanrurugi_api::cors::apply_cors;
use lanrurugi_api::procedure::require_api_key;

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
    // Cloned before `.with_state` moves the original — `serve_index`'s own closure below (registered
    // well after `state` would otherwise be gone) needs its own copy of the same shared `AppState`.
    let index_state = state.clone();
    // Also cloned before `.with_state` below moves `state` — `opensearch::router()` needs its own
    // copy, mounted at the bare (non-`/api`-nested) top level (see that module's own docs on why
    // it must be reachable without going through `require_api_key` at all).
    let opensearch_state = state.clone();
    let api = lanrurugi_api::login::router()
        .merge(lanrurugi_api::settings::public_router())
        .merge(protected)
        .with_state(state.clone())
        // Outermost `/api/*` layer, before `require_api_key` — a preflight `OPTIONS` request must
        // never reach the auth check (see `cors::apply_cors`'s own docs).
        .layer(axum::middleware::from_fn_with_state(state, apply_cors));

    let mut router = Router::new()
        .nest("/api", api)
        .merge(lanrurugi_api::opensearch::router().with_state(opensearch_state));

    if let Some(dir) = docs_dir {
        router = router.nest_service("/docs", ServeDir::new(dir));
    }

    if let Some(dir) = static_dir {
        // `ServeDir::fallback` (a method on `ServeDir` itself), not `Router::fallback` — the two
        // look interchangeable but aren't: a `Router` only ever has *one* fallback slot, so a
        // second `.fallback()`/`.fallback_service()` call replaces the first rather than chaining
        // after it (confirmed live: an earlier version of this wired `ServeDir` via
        // `Router::fallback_service` and `serve_index` via a later `Router::fallback`, and the
        // second call silently discarded the first — `ServeDir` never got a chance to serve real
        // assets like `/assets/app.js` at all, they all fell straight through to `serve_index`
        // instead, a real regression caught by `static_frontend_is_served_with_spa_fallback`).
        // `ServeDir::fallback` is the one designed for exactly this: it's `ServeDir`'s own inner
        // fallback, invoked only when *that* `ServeDir` finds no matching file, so it composes
        // into one single `Service` to hand to `Router::fallback_service`.
        //
        // `tower::service_fn` wraps `serve_index` as a plain `tower::Service` — `index_state`/`dir`
        // are captured by the inner closure directly, not via axum's own `State<AppState>`
        // extractor, since by this point `router` (built from `api.with_state(state)` above) is
        // already `Router<()>` (axum's own convention: `Router<S>` means "still missing an `S`", so
        // `.with_state` having already run means there's no `AppState` left for a handler
        // registered from here on to extract). `index_state`, cloned from the original `state`
        // before it was moved into `.with_state` above, sidesteps that entirely. `serve_index`
        // replaces `index.html`'s inline anti-flash-of-default-theme script's placeholder with the
        // real, current theme instead of serving the file's static bytes verbatim — see that
        // handler's own docs.
        let index_dir = dir.clone();
        let fallback = service_fn(move |_req: Request| {
            let state = index_state.clone();
            let dir = index_dir.clone();
            async move { Ok::<_, Infallible>(serve_index(state, dir).await) }
        });
        // `append_index_html_on_directories(false)` — without this, `ServeDir`'s own default
        // behavior resolves a directory request (including `/` itself) straight to a literal
        // `index.html` byte-for-byte, *before* ever consulting `.fallback`, so the exact file this
        // whole mechanism exists to template never actually reaches `serve_index` at all — a real
        // regression caught live: a request to `/` was returning the raw `__SERVER_THEME__`
        // placeholder, unsubstituted, while a request to an unmatched SPA route like `/library`
        // (which `ServeDir` has no literal file for) worked correctly. Turning this off makes every
        // directory request 404 out of `ServeDir` the same way a missing file does, so `/` gets the
        // same `serve_index` treatment as any other unmatched route.
        router = router.fallback_service(
            ServeDir::new(dir)
                .append_index_html_on_directories(false)
                .fallback(fallback),
        );
    }

    router.layer(TraceLayer::new_for_http())
}

/// Serves `{dir}/index.html` with its inline anti-flash-of-default-theme `<script id="theme-init"
/// data-theme="">`'s empty `data-theme` attribute (see that file's own docs) filled in with the
/// real current theme — read fresh on every request (not cached at startup) so a theme change from
/// the Settings page takes effect on the very next page load, matching every other place this app
/// reads live settings rather than a boot-time snapshot. Falls back to serving the file
/// byte-for-byte unmodified (the script's own client-side JS then falls back to its `localStorage`
/// cache, then `modern.css` — see `index.html`) whenever either step can't complete: the file is
/// missing/unreadable, or the Redis lookup fails/returns an unrecognized value. A broken index page
/// would be far worse than one that occasionally still has to flash once.
async fn serve_index(state: AppState, dir: PathBuf) -> Response {
    let path = dir.join("index.html");
    let html = match tokio::fs::read_to_string(&path).await {
        Ok(html) => html,
        Err(_) => return not_found(),
    };
    // Targets the specific, unambiguous `id="theme-init" data-theme=""` attribute value, not a
    // placeholder string anywhere in the file's prose/script body — an earlier version of this
    // used a bare `__SERVER_THEME__` JS-string-literal placeholder instead, and a documentation
    // comment elsewhere in `index.html` happened to also spell out that exact text, which the
    // naive first-match string replace silently rewrote instead of the real assignment (confirmed
    // live via a real curl against a real running server: the script's actual behavior was
    // completely untouched while the response looked correct at a glance). An HTML attribute value
    // has no such ambiguity to begin with — nothing else in a well-formed HTML document could
    // plausibly contain the literal substring `data-theme=""`.
    // `format!` here does no HTML-attribute escaping of its own — the only thing standing between
    // `theme` and a real attribute-breakout injection (a `"` in the value closing the attribute
    // early, e.g. `fetch_theme_for_html_injection`'s own doc comment's `"><script>...` example) is
    // that function's `KNOWN_THEME_FILES` whitelist upstream, which guarantees `theme` is always
    // exactly one of five fixed, quote-free filenames. This function must never be handed an
    // unvalidated theme string — use `fetch_theme_for_html_injection`, never plain `fetch_theme`,
    // for anything that ends up here.
    let placeholder = r#"id="theme-init" data-theme="""#;
    let html = match fetch_theme_for_html_injection(&state).await {
        Some(theme) => html.replacen(
            placeholder,
            &format!(r#"id="theme-init" data-theme="{theme}""#),
            1,
        ),
        None => html,
    };
    (
        StatusCode::OK,
        [(header::CONTENT_TYPE, "text/html; charset=utf-8")],
        html,
    )
        .into_response()
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "index.html not found").into_response()
}
