//! AI plugin creation wizard (`specs/006-ai-plugin-wizard`) — five additive, stateless endpoints
//! that let a logged-in user generate/trial-run/save login/metadata/download plugins from a
//! natural-language description of a target site. Session-only access (FR-023) is enforced by
//! `route_policy.csv`, not a middleware here — see that file's own comments.

mod analyze_login;
mod fetch;
mod generate;
mod lookup;
mod save;
mod tool_loop;
mod trial_run;

use axum::routing::{get, post};
use axum::Router;

use crate::AppState;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/plugin-wizard/lookup", post(lookup::lookup))
        .route(
            "/plugin-wizard/analyze-login",
            post(analyze_login::analyze_login),
        )
        .route(
            "/plugin-wizard/generate/start",
            post(generate::generate_start),
        )
        .route(
            "/plugin-wizard/generate/stream/{id}",
            get(generate::generate_stream),
        )
        .route("/plugin-wizard/trial-run", post(trial_run::trial_run))
        .route("/plugin-wizard/save", post(save::save))
}
