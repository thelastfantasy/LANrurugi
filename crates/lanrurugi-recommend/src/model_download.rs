//! Auto-downloads the ONNX embedding model (+ tokenizer) the recommender needs, ported from the
//! proven pattern in `jellyfin-suite`'s `ModelAcquisitionService.cs`:
//!
//! - A lightweight HEAD request on startup compares the remote ETag with a cached `.etag` sidecar;
//!   the file is re-downloaded only when the remote actually changed. When the network is
//!   unreachable the cached model keeps being used.
//! - Downloads go to a `.tmp` file and are atomically renamed into place, so a crashed download
//!   never leaves a half-written model the ORT session would silently load.
//! - A user-placed file at the final path skips all download logic (offline/air-gapped installs).
//! - The source URL can be overridden via env var (mirror/proxy deployments).
//!
//! Model: `Xenova/paraphrase-multilingual-MiniLM-L12-v2` (the transformers.js ONNX export of
//! `sentence-transformers/paraphrase-multilingual-MiniLM-L12-v2` — includes `tokenizer.json`,
//! which the Rust `tokenizers` crate consumes directly). Only the int8-quantized ONNX and the
//! tokenizer are needed; the pooling config (mean pooling + L2 normalize) is the standard
//! sentence-transformers setting for this model and is hardcoded in `embedding.rs`.

use std::path::{Path, PathBuf};

use thiserror::Error;
use tokio::io::AsyncWriteExt;

/// One downloadable artifact: remote URL, final file name, and the env var that can override the
/// URL (mirror deployments).
struct Artifact {
    name: &'static str,
    url: &'static str,
    file_name: &'static str,
    url_env_var: &'static str,
}

const MODEL_ARTIFACTS: &[Artifact] = &[
    Artifact {
        name: "embedding model",
        url: "https://huggingface.co/Xenova/paraphrase-multilingual-MiniLM-L12-v2/resolve/main/onnx/model_quantized.onnx",
        file_name: "paraphrase-multilingual-MiniLM-L12-v2_quantized.onnx",
        url_env_var: "LANRURUGI_EMBEDDING_MODEL_URL",
    },
    Artifact {
        name: "tokenizer",
        url: "https://huggingface.co/Xenova/paraphrase-multilingual-MiniLM-L12-v2/resolve/main/tokenizer.json",
        file_name: "tokenizer.json",
        url_env_var: "LANRURUGI_EMBEDDING_TOKENIZER_URL",
    },
];

#[derive(Debug, Error)]
pub enum ModelDownloadError {
    #[error("HTTP error for {artifact}: {source}")]
    Http {
        artifact: &'static str,
        #[source]
        source: reqwest::Error,
    },
    #[error("HTTP status {status} for {artifact}")]
    HttpStatus { artifact: &'static str, status: u16 },
    #[error("I/O error for {artifact} at {path}: {source}")]
    Io {
        artifact: &'static str,
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("no cached model and network unreachable for {artifact}")]
    Offline { artifact: &'static str },
}

/// Resolves `artifact`'s URL: env-var override wins, else the built-in default.
fn resolved_url(artifact: &Artifact) -> String {
    std::env::var(artifact.url_env_var)
        .ok()
        .filter(|u| !u.trim().is_empty())
        .unwrap_or_else(|| artifact.url.to_string())
}

/// Ensures every artifact exists and is current in `models_dir`, downloading as needed. Returns
/// `(model_path, tokenizer_path)`. Callers should run this in a background task (see
/// `spawn_acquire_models`) — downloads are large and must not block server startup.
pub async fn acquire_models(models_dir: &Path) -> Result<(PathBuf, PathBuf), ModelDownloadError> {
    tokio::fs::create_dir_all(models_dir)
        .await
        .map_err(|e| ModelDownloadError::Io {
            artifact: "models dir",
            path: models_dir.to_path_buf(),
            source: e,
        })?;

    let mut model_path = None;
    let mut tokenizer_path = None;
    for artifact in MODEL_ARTIFACTS {
        let path = acquire_one(models_dir, artifact).await?;
        if artifact.file_name.ends_with(".onnx") {
            model_path = Some(path);
        } else {
            tokenizer_path = Some(path);
        }
    }
    Ok((
        model_path.expect("model artifact list always has the onnx"),
        tokenizer_path.expect("model artifact list always has the tokenizer"),
    ))
}

async fn acquire_one(
    models_dir: &Path,
    artifact: &Artifact,
) -> Result<PathBuf, ModelDownloadError> {
    let final_path = models_dir.join(artifact.file_name);

    // 1. A user-placed file at the final path skips all download logic (offline installs).
    if final_path.exists() {
        tracing::debug!(path = %final_path.display(), "using existing {}", artifact.name);
        return Ok(final_path);
    }

    let url = resolved_url(artifact);
    let client = reqwest::Client::new();

    // 2. HEAD request — check whether the remote file changed since the cached ETag.
    let remote_etag = match client.head(&url).send().await {
        Ok(resp) if resp.status().is_success() => resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .or_else(|| {
                resp.headers()
                    .get(reqwest::header::LAST_MODIFIED)
                    .and_then(|v| v.to_str().ok())
                    .map(|s| s.to_string())
            }),
        Ok(resp) => {
            return Err(ModelDownloadError::HttpStatus {
                artifact: artifact.name,
                status: resp.status().as_u16(),
            })
        }
        Err(e) => {
            tracing::warn!(
                artifact = artifact.name,
                error = %e,
                "HEAD failed — using cached model if available"
            );
            None
        }
    };

    let etag_path = models_dir.join(format!("{}.etag", artifact.file_name));
    let cached_etag = match tokio::fs::read_to_string(&etag_path).await {
        Ok(s) => Some(s.trim().to_string()),
        Err(_) => None,
    };

    // 3. Cached file present and (no remote info, or unchanged) → use it.
    if final_path.exists() {
        match (&remote_etag, &cached_etag) {
            (None, _) => return Ok(final_path),
            (Some(r), Some(c)) if r == c => return Ok(final_path),
            (Some(r), _) => {
                tracing::info!(
                    artifact = artifact.name,
                    old = cached_etag.as_deref().unwrap_or("none"),
                    new = r,
                    "remote updated — re-downloading"
                );
            }
        }
    } else if remote_etag.is_none() {
        return Err(ModelDownloadError::Offline {
            artifact: artifact.name,
        });
    }

    // 4. Download (first time or ETag changed) via a temp file + atomic rename.
    download_to(
        models_dir,
        artifact,
        &url,
        &final_path,
        &etag_path,
        remote_etag.as_deref(),
    )
    .await
}

async fn download_to(
    models_dir: &Path,
    artifact: &Artifact,
    url: &str,
    final_path: &Path,
    etag_path: &Path,
    expected_etag: Option<&str>,
) -> Result<PathBuf, ModelDownloadError> {
    let tmp_path = models_dir.join(format!("{}.tmp", artifact.file_name));
    let client = reqwest::Client::new();
    tracing::info!(artifact = artifact.name, url, "downloading");
    let resp = client
        .get(url)
        .send()
        .await
        .map_err(|e| ModelDownloadError::Http {
            artifact: artifact.name,
            source: e,
        })?;
    if !resp.status().is_success() {
        return Err(ModelDownloadError::HttpStatus {
            artifact: artifact.name,
            status: resp.status().as_u16(),
        });
    }
    let etag = resp
        .headers()
        .get(reqwest::header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string())
        .or_else(|| {
            resp.headers()
                .get(reqwest::header::LAST_MODIFIED)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
        })
        .or_else(|| expected_etag.map(|s| s.to_string()));

    let mut file =
        tokio::fs::File::create(&tmp_path)
            .await
            .map_err(|e| ModelDownloadError::Io {
                artifact: artifact.name,
                path: tmp_path.clone(),
                source: e,
            })?;
    let mut stream = resp.bytes_stream();
    use futures_util::StreamExt;
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ModelDownloadError::Http {
            artifact: artifact.name,
            source: e,
        })?;
        file.write_all(&chunk)
            .await
            .map_err(|e| ModelDownloadError::Io {
                artifact: artifact.name,
                path: tmp_path.clone(),
                source: e,
            })?;
    }
    file.flush().await.map_err(|e| ModelDownloadError::Io {
        artifact: artifact.name,
        path: tmp_path.clone(),
        source: e,
    })?;
    drop(file);

    tokio::fs::rename(&tmp_path, final_path)
        .await
        .map_err(|e| ModelDownloadError::Io {
            artifact: artifact.name,
            path: final_path.to_path_buf(),
            source: e,
        })?;
    if let Some(etag) = etag {
        let mut f =
            tokio::fs::File::create(etag_path)
                .await
                .map_err(|e| ModelDownloadError::Io {
                    artifact: artifact.name,
                    path: etag_path.to_path_buf(),
                    source: e,
                })?;
        f.write_all(etag.as_bytes())
            .await
            .map_err(|e| ModelDownloadError::Io {
                artifact: artifact.name,
                path: etag_path.to_path_buf(),
                source: e,
            })?;
    }
    tracing::info!(artifact = artifact.name, path = %final_path.display(), "download complete");
    Ok(final_path.to_path_buf())
}

/// Spawns the background acquisition task; the returned handle resolves to the ready paths (or
/// the error). Server startup calls this once and keeps going — the recommender endpoint reports
/// "model not ready" until the download finishes (the frontend shows a spinner).
pub fn spawn_acquire_models(
    models_dir: PathBuf,
) -> tokio::task::JoinHandle<Result<(PathBuf, PathBuf), ModelDownloadError>> {
    tokio::spawn(async move { acquire_models(&models_dir).await })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_override_wins_over_default_url() {
        // SAFETY of env mutation in tests: each test runs on its own thread; serialized via
        // the env var name being unique to this test.
        let artifact = &MODEL_ARTIFACTS[0];
        std::env::set_var(
            "LANRURUGI_EMBEDDING_MODEL_URL",
            "https://mirror.example/model.onnx",
        );
        let url = resolved_url(artifact);
        std::env::remove_var("LANRURUGI_EMBEDDING_MODEL_URL");
        assert_eq!(url, "https://mirror.example/model.onnx");
        // And the default when unset.
        assert!(resolved_url(artifact).contains("huggingface.co"));
    }
}
