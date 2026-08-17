pub mod activity;
pub mod api_tokens;
pub mod archives;
pub mod artist_backfill;
pub mod auth;
pub mod auth_context;
pub mod authz;
pub mod bench;
pub mod categories;
pub mod common;
pub mod cors;
pub mod database;
pub mod download_manager;
pub mod download_queue;
pub mod duplicates;
pub mod jobs;
pub mod login;
pub mod logs;
pub mod misc;
pub mod opds;
pub mod plugins;
pub mod procedure;
pub mod recommend;
pub mod recommend_llm;
pub mod recommend_precompute;
pub mod scripts;
pub mod search;
pub mod settings;
pub mod shinobu;
pub mod stamps;
pub mod state;
pub mod tag_rules;
pub mod tankoubon_grouping;
pub mod tankoubons;
pub mod upload;

pub use state::{AppState, AuthConfig, LibraryPaths, Repositories};

use axum::Router;

/// Root router for the `lanrurugi-api` crate — merges each endpoint-group's routes. Response
/// shapes/paths below are additive-only over the verified legacy `tools/openapi.yaml` contract
/// (constitution Principle II); see each submodule for the specific paths it covers.
///
/// Does **not** include [`login::router`] or [`settings::public_router`] — those must stay
/// reachable without a valid API key/session (see each module's docs), so the server wires them
/// in separately, unprotected.
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(activity::router())
        .merge(api_tokens::router())
        .merge(archives::router())
        .merge(bench::router())
        .merge(categories::router())
        .merge(tankoubons::router())
        .merge(tankoubon_grouping::router())
        .merge(stamps::router())
        .merge(misc::router())
        .merge(shinobu::router())
        .merge(upload::router())
        .merge(search::router())
        .merge(opds::router())
        .merge(plugins::router())
        .merge(database::router())
        .merge(download_queue::router())
        .merge(duplicates::router())
        .merge(jobs::router())
        .merge(settings::router())
        .merge(scripts::router())
        .merge(logs::router())
        .merge(recommend::router())
}
