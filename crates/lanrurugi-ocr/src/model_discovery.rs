//! Locates the `manga-ocr` recognition model files on disk (research.md §1, mirroring
//! `~/jellyfin-suite`'s `find_model()` search-path pattern).
//!
//! First candidate that actually contains every required file wins. A missing model is a normal,
//! recoverable condition — not a startup failure: callers surface it as "translation unavailable"
//! (FR-019) and untranslated reading continues untouched.

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Overrides the whole search path when set.
pub const MODEL_DIR_ENV: &str = "LANRURUGI_MANGA_OCR_MODEL_DIR";

/// Production install path (Debian package / bare-metal deployment).
const SYSTEM_MODEL_DIR: &str = "/var/lib/lanrurugi/models/manga-ocr";
/// Container path written by the `Dockerfile`'s `ocr-model` stage.
const CONTAINER_MODEL_DIR: &str = "/app/models/manga-ocr";

/// Every file the pipeline needs present before a directory counts as a real model dir.
///
/// Includes the detection model: research.md §1 assumed `oar-ocr` would supply its own, but the
/// real 0.9.2 API requires the caller to hand it a `ModelSource`, so detection and recognition
/// models are acquired and discovered together.
pub const REQUIRED_FILES: [&str; 4] = [
    "encoder_model.onnx",
    "decoder_model.onnx",
    "vocab.txt",
    "detection_model.onnx",
];

#[derive(Debug, Error)]
pub enum ModelDiscoveryError {
    #[error(
        "no manga-ocr recognition model found (searched: {searched}). Run \
         scripts/fetch-ocr-model.sh, or set {env} to a directory containing {files}."
    )]
    NotFound {
        searched: String,
        env: &'static str,
        files: String,
    },
}

/// Whether `dir` holds a complete model.
fn is_complete_model_dir(dir: &Path) -> bool {
    REQUIRED_FILES.iter().all(|f| dir.join(f).is_file())
}

/// The ordered candidate list. Split out from [`find_model_dir`] so the error message can report
/// exactly what was searched, and so tests can assert the ordering without touching the filesystem.
fn candidate_dirs() -> Vec<PathBuf> {
    let mut candidates = Vec::new();

    if let Some(dir) = std::env::var_os(MODEL_DIR_ENV) {
        candidates.push(PathBuf::from(dir));
    }

    // Next to the running binary — how a locally built `cargo run` finds a dev-fetched model.
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join("models").join("manga-ocr"));
        }
    }

    candidates.push(PathBuf::from(SYSTEM_MODEL_DIR));
    candidates.push(PathBuf::from(CONTAINER_MODEL_DIR));
    candidates
}

/// Resolves the directory holding the recognition model, or reports every path tried.
pub fn find_model_dir() -> Result<PathBuf, ModelDiscoveryError> {
    let candidates = candidate_dirs();

    if let Some(found) = candidates.iter().find(|dir| is_complete_model_dir(dir)) {
        tracing::debug!(dir = %found.display(), "resolved manga-ocr recognition model");
        return Ok(found.clone());
    }

    Err(ModelDiscoveryError::NotFound {
        searched: candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
        env: MODEL_DIR_ENV,
        files: REQUIRED_FILES.join(", "),
    })
}

/// Paths to the three model files within an already-resolved directory.
#[derive(Debug, Clone)]
pub struct ModelPaths {
    pub encoder: PathBuf,
    pub decoder: PathBuf,
    pub vocab: PathBuf,
    pub detection: PathBuf,
}

impl ModelPaths {
    pub fn in_dir(dir: &Path) -> Self {
        Self {
            encoder: dir.join(REQUIRED_FILES[0]),
            decoder: dir.join(REQUIRED_FILES[1]),
            vocab: dir.join(REQUIRED_FILES[2]),
            detection: dir.join(REQUIRED_FILES[3]),
        }
    }

    /// Convenience: discover the directory and derive the file paths in one step.
    pub fn discover() -> Result<Self, ModelDiscoveryError> {
        find_model_dir().map(|dir| Self::in_dir(&dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn incomplete_dir_is_rejected() {
        let dir = std::env::temp_dir().join("lanrurugi-ocr-model-discovery-test");
        let _ = std::fs::create_dir_all(&dir);
        // Only one of the three required files present.
        let _ = std::fs::write(dir.join("encoder_model.onnx"), b"stub");
        assert!(!is_complete_model_dir(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn complete_dir_is_accepted() {
        let dir = std::env::temp_dir().join("lanrurugi-ocr-model-discovery-complete");
        let _ = std::fs::create_dir_all(&dir);
        for f in REQUIRED_FILES {
            let _ = std::fs::write(dir.join(f), b"stub");
        }
        assert!(is_complete_model_dir(&dir));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn system_and_container_paths_are_always_candidates() {
        let candidates = candidate_dirs();
        assert!(candidates.iter().any(|p| p.ends_with("manga-ocr")));
        assert!(candidates.contains(&PathBuf::from(CONTAINER_MODEL_DIR)));
    }
}
