pub mod activity;
pub mod api_tokens;
pub mod archive_split;
pub mod archives;
pub mod artist_backfill;
pub mod auth;
pub mod auth_context;
pub mod authz;
pub mod bench;
pub mod bookmarks;
pub mod categories;
pub mod common;
pub mod cors;
pub mod database;
pub mod device_info;
pub mod download_manager;
pub mod download_queue;
pub mod duplicates;
pub mod embed_worker_client;
pub mod font_pattern;
pub mod geoip;
pub mod gpu_worker_client;
pub mod health;
pub mod jobs;
pub mod llm_prompts;
pub mod login;
pub mod logs;
pub mod misc;
pub mod opds;
pub mod opensearch;
pub mod plugin_wizard;
pub mod plugins;
pub mod procedure;
pub mod recommend;
pub mod recommend_llm;
pub mod recommend_precompute;
pub mod scripts;
pub mod search;
pub mod sessions;
pub mod settings;
pub mod shinobu;
pub mod stamps;
pub mod state;
pub mod tag_rules;
pub mod tankoubon_grouping;
pub mod tankoubons;
pub mod terminology_glossary;
pub mod translation;
pub mod translation_pipeline;
pub mod translation_settings;
pub mod upload;
pub mod version;

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
        .merge(archive_split::router())
        .merge(archives::router())
        .merge(bench::router())
        .merge(bookmarks::router())
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
        .merge(plugin_wizard::router())
        .merge(database::router())
        .merge(download_queue::router())
        .merge(duplicates::router())
        .merge(jobs::router())
        .merge(settings::router())
        .merge(scripts::router())
        .merge(sessions::router())
        .merge(logs::router())
        .merge(recommend::router())
        // `specs/004-ocr-manga-translation` — all new, additive paths (Principle II).
        .merge(translation::router())
        .merge(translation_settings::router())
        .merge(terminology_glossary::router())
        .merge(font_pattern::router())
}
