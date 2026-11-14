//! The sharing engine, extracted from `lanrurugi-api` (023).
//!
//! Nothing here may know about axum, HTTP routes or `AppState`: the engine talks to the host
//! through [`host::ShareHost`], and the API crate keeps the HTTP handlers. The split exists so the
//! transport can be tested (and later reused) without a web server.
//!
//! **The engine owns** the iroh endpoint and its lifecycle ([`runtime`]: binding, port rotation,
//! interface watch), the share protocol on both ends ([`server`] answers, [`import`] fetches and
//! verifies), knock records and dialling ([`knock`], [`publish`]), the node roles
//! ([`node_service`], [`nodes`], [`relay`]), helpers ([`helper`]), fingerprints ([`blobs`]),
//! upload caps ([`limits`]) and this instance's sharing identity ([`shares`]).
//!
//! **The host provides** storage handles, the library (archives, tankoubons, categories, tags),
//! the download queue an import reports to, activity records, settings and the time zone — see
//! [`host::ShareHost`]. A host also runs its own loops next to the engine's
//! ([`host::ShareHost::start_host_services`]).

pub mod attempts;
pub mod blobs;
pub mod helper;
pub mod host;
pub mod import;
pub mod interfaces;
pub mod knock;
pub mod limits;
pub mod link;
pub mod manifest;
pub mod net;
pub mod node_service;
pub mod nodes;
pub mod proxy;
pub mod publish;
pub mod relay;
pub mod runtime;
pub mod server;
pub mod settings;
pub mod shares;
pub mod status;
pub mod util;
