pub mod archives;
pub mod auth;
pub mod bench;
pub mod categories;
pub mod common;
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
pub mod scripts;
pub mod search;
pub mod settings;
pub mod shinobu;
pub mod stamps;
pub mod state;
pub mod tankoubons;
pub mod upload;

pub use state::{AppState, AuthConfig, LibraryPaths, Repositories};

use axum::Router;

/// Root router for the `lanrurugi-api` crate — merges each endpoint-group's routes. Response
/// shapes/paths below are additive-only over the verified legacy `tools/openapi.yaml` contract
/// (constitution Principle II); see each submodule for the specific paths it covers.
///
/// Does **not** include [`login::router`] — that one must stay reachable without a valid
/// API key/session (see that module's docs), so the server wires it in separately, unprotected.
pub fn router() -> Router<AppState> {
    Router::new()
        .merge(archives::router())
        .merge(bench::router())
        .merge(categories::router())
        .merge(tankoubons::router())
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
}
