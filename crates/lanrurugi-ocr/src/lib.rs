//! OCR for Phase 2's on-page manga translation (`specs/004-ocr-manga-translation`).
//!
//! Two independently sourced stages (research.md §1): [`detect`] finds *where* text is via
//! `oar-ocr`'s PP-OCR detection, and [`recognize`] transcribes *what it says* via this crate's own
//! CPU-only `ort` integration against kha-white's Apache-2.0 `manga-ocr` weights. [`merge`] then
//! folds raw per-line boxes into the paragraph-level [`DetectedTextRegion`] records translation and
//! font matching operate on, and [`style_estimate`] annotates each with its own colour/boldness.
//!
//! See this crate's `README.md` for model file placement and the discovery search path.

pub mod batch;
mod bubble_model_discovery;
pub mod bubble_segment;
pub mod detect;
pub mod entities;
pub mod merge;
pub mod model_discovery;
pub mod recognize;
pub mod style_estimate;

pub use entities::{BoundingBox, DetectedTextRegion, PageNumber, Rgb};
pub use merge::{merge_lines, MergeConfig};
pub use model_discovery::{find_model_dir, ModelDiscoveryError};
