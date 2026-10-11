//! Volume-level font matching for Phase 2's on-page manga translation
//! (`specs/004-ocr-manga-translation`).
//!
//! Exists so the expensive font classifier doesn't have to run on every single text block: a
//! volume's dominant lettering styles are established once (from non-cover pages), locked, and then
//! reused via a cheap per-block router. The three stages are [`voting`], [`routing`], and
//! [`meltdown`] — see research.md §4, and each module's own docs for the design concerns each one
//! resolves.

pub mod classify;
pub mod entities;
pub mod meltdown;
pub mod routing;
pub mod storage;
pub mod voting;

pub use entities::{FontId, VolumeFontPattern};
pub use routing::{route_block, RouteDecision};
pub use storage::{FontPatternRepository, FontPatternStorageError};
