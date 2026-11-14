//! Instance-to-instance sharing. The engine lives in `lanrurugi-share` (023); this module keeps
//! the host-side pieces (the fingerprint preparation worker, the cap readout for the settings
//! page) and re-exports the engine under the path the rest of the API already uses.

pub mod blobs;
pub mod limits;

pub use lanrurugi_share::runtime::*;
