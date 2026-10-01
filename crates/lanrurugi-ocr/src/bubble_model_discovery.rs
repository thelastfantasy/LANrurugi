//! Locates the manga speech-bubble segmentation ONNX model on disk — same search-path convention
//! as `model_discovery` (env override, next-to-binary, system path, container path), kept as a
//! separate module (not folded into that one's `REQUIRED_FILES`) because this model is
//! independently optional: OCR/recognition works with or without it, it only feeds
//! `bubble_segment`'s precise-mask path (`lanrurugi-inpaint`'s alternative to plain bbox erasure).

use std::path::{Path, PathBuf};
use thiserror::Error;

pub const MODEL_DIR_ENV: &str = "LANRURUGI_BUBBLE_SEG_MODEL_DIR";
const SYSTEM_MODEL_DIR: &str = "/var/lib/lanrurugi/models/bubble-seg";
const CONTAINER_MODEL_DIR: &str = "/app/models/bubble-seg";
pub const MODEL_FILE: &str = "bubble_seg.onnx";

#[derive(Debug, Error)]
pub enum BubbleModelDiscoveryError {
    #[error(
        "no bubble segmentation model found (searched: {searched}). Run \
         scripts/fetch-bubble-seg-model.sh, or set {env} to a directory containing {file}."
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

fn candidate_dirs() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    if let Some(dir) = std::env::var_os(MODEL_DIR_ENV) {
        candidates.push(PathBuf::from(dir));
    }
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            candidates.push(exe_dir.join("models").join("bubble-seg"));
        }
    }
    candidates.push(PathBuf::from(SYSTEM_MODEL_DIR));
    candidates.push(PathBuf::from(CONTAINER_MODEL_DIR));
    candidates
}

pub fn find_model_dir() -> Result<PathBuf, BubbleModelDiscoveryError> {
    let candidates = candidate_dirs();
    if let Some(found) = candidates.iter().find(|dir| is_complete_model_dir(dir)) {
        tracing::debug!(dir = %found.display(), "resolved bubble segmentation model");
        return Ok(found.clone());
    }
    Err(BubbleModelDiscoveryError::NotFound {
        searched: candidates
            .iter()
            .map(|p| p.display().to_string())
            .collect::<Vec<_>>()
            .join(", "),
        env: MODEL_DIR_ENV,
        file: MODEL_FILE,
    })
}

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
        let dir = std::env::temp_dir().join("lanrurugi-bubble-seg-test-incomplete");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        assert!(!is_complete_model_dir(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn complete_dir_is_accepted() {
        let dir = std::env::temp_dir().join("lanrurugi-bubble-seg-test-complete");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join(MODEL_FILE), b"fake").unwrap();
        assert!(is_complete_model_dir(&dir));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
