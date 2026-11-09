//! The sharing engine, extracted from `lanrurugi-api` (023).
//!
//! Nothing here may know about axum, HTTP routes or `AppState`: the engine talks to the host
//! through [`host::ShareHost`], and the API crate keeps the HTTP handlers. The split exists so the
//! transport can be tested (and later reused) without a web server.

pub mod blobs;
pub mod host;
pub mod interfaces;
pub mod limits;
pub mod link;
pub mod net;
pub mod nodes;
pub mod proxy;
pub mod publish;
pub mod relay;
pub mod settings;
pub mod status;
pub mod util;
