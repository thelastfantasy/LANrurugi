//! Locates the LaMa inpainting ONNX model on disk — same search-path convention as
//! `lanrurugi-ocr::model_discovery` (env override, next-to-binary, system path, container path).
//!
//! A missing model is a normal, recoverable condition, not a startup failure: callers surface it
//! as "inpainting unavailable" and fall back to the existing flat-fill/outline-stroke compositing
//! (`lanrurugi-translate::composite`), the same graceful-degradation shape already established for
//! the OCR recognition model (FR-019).

use std::path::{Path, PathBuf};
use thiserror::Error;

/// Overrides the whole search path when set.
pub const MODEL_DIR_ENV: &str = "LANRURUGI_INPAINT_MODEL_DIR";

/// Production install path (Debian package / bare-metal deployment).
const SYSTEM_MODEL_DIR: &str = "/var/lib/lanrurugi/models/inpaint";
/// Container path written by the `Dockerfile`'s inpaint-model stage.
const CONTAINER_MODEL_DIR: &str = "/app/models/inpaint";

/// The one file a directory needs before it counts as a real model dir.
pub const MODEL_FILE: &str = "lama-manga-dynamic.onnx";

#[derive(Debug, Error)]
pub enum ModelDiscoveryError {
    #[error(
        "no LaMa inpainting model found (searched: {searched}). Run \
         scripts/fetch-inpaint-model.sh, or set {env} to a directory containing {file}."
    )]
    NotFound {
        searched: String,
        env: &'static str,
        file: &'static str,
    },
}

fn is_complete_model_dir(dir: &Path) -> bool {
    dir.join(MODEL_FILE).is_file()
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
            candidates.push(exe_dir.join("models").join("inpaint"));
        }
    }

    candidates.push(PathBuf::from(SYSTEM_MODEL_DIR));
    candidates.push(PathBuf::from(CONTAINER_MODEL_DIR));
    candidates
}

/// Resolves the directory holding the inpainting model, or reports every path tried.
pub fn find_model_dir() -> Result<PathBuf, ModelDiscoveryError> {
    let candidates = candidate_dirs();

    if let Some(found) = candidates.iter().find(|dir| is_complete_model_dir(dir)) {
        tracing::debug!(dir = %found.display(), "resolved LaMa inpainting model");
        return Ok(found.clone());
    }

    Err(ModelDiscoveryError::NotFound {
        searched: candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
        env: MODEL_DIR_ENV,
        file: MODEL_FILE,
    })
}

/// Path to the model file within an already-resolved directory.
pub fn model_path(dir: &Path) -> PathBuf {
    dir.join(MODEL_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn candidates_always_include_system_and_container_paths() {
        let candidates = candidate_dirs();
        assert!(candidates.contains(&PathBuf::from(SYSTEM_MODEL_DIR)));
        assert!(candidates.contains(&PathBuf::from(CONTAINER_MODEL_DIR)));
    }

    #[test]
    fn incomplete_dir_is_rejected() {
        let dir = std::env::temp_dir().join("lanrurugi-inpaint-test-incomplete");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!is_complete_model_dir(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn complete_dir_is_accepted() {
        let dir = std::env::temp_dir().join("lanrurugi-inpaint-test-complete");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MODEL_FILE), b"fake").unwrap();
        assert!(is_complete_model_dir(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
