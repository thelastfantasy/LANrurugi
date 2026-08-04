//! Text embedding via ONNX Runtime for `sentence-transformers/paraphrase-multilingual-MiniLM-
//! L12-v2` (Xenova ONNX export) — the model the reader recommender uses for series recognition.
//!
//! Standard sentence-transformers pipeline for this model: BERT WordPiece tokenize (the Rust
//! `tokenizers` crate, loading `tokenizer.json`), ONNX inference (inputs `input_ids` /
//! `attention_mask` / `token_type_ids`, output `last_hidden_state`), **mean pooling** over the
//! token dimension masked by `attention_mask`, then **L2 normalization** — so cosine similarity
//! between two embedded titles is a plain dot product. The pooling config is the model's own
//! documented `1_Pooling` setting, hardcoded here (a config.json read would only re-confirm it).
//!
//! Session construction mirrors `frame-forge`'s `make_cpu_session` (`~/jellyfin-suite/crates/
//! frame-forge/src/dl_match.rs`): the execution provider is registered explicitly, because a
//! GPU-flavored `libonnxruntime.so` build hangs indefinitely in `commit_from_file` when no EP is
//! registered. We run CPU-only anyway; the explicit registration is what keeps the same binary
//! safe on either runtime flavor.

use std::path::Path;
use std::sync::Mutex;

use ndarray::Dimension;
use ort::ep;
use ort::session::Session;
use ort::value::Tensor;
use thiserror::Error;
use tokenizers::Tokenizer;

/// MiniLM-L12's context window (the Xenova export's own `max_position_embeddings`).
const MAX_SEQ_LEN: usize = 512;
/// Embedding dimensionality (MiniLM-L12 hidden size).
pub const EMBEDDING_DIM: usize = 384;

#[derive(Debug, Error)]
pub enum EmbeddingError {
    #[error("failed to load tokenizer from {path}: {source}")]
    Tokenizer {
        path: String,
        #[source]
        source: tokenizers::Error,
    },
    #[error("failed to build ORT session from {path}: {source}")]
    Session {
        path: String,
        #[source]
        source: ort::Error,
    },
    #[error("failed to tokenize: {0}")]
    Tokenize(#[from] tokenizers::Error),
    #[error("failed to run ORT inference: {0}")]
    Inference(#[from] ort::Error),
    #[error("unexpected model output: {0}")]
    BadOutput(String),
}

/// Loaded embedding model, ready to embed titles on demand. Cheap to construct once at startup
/// (~100-200ms session load); `embed` is the hot path. The session is behind a `Mutex` because
/// `ort`'s `run` takes `&mut self` and embedding may be requested from concurrent HTTP handlers.
pub struct Embedder {
    session: Mutex<Session>,
    tokenizer: Tokenizer,
}

impl Embedder {
    pub fn load(model_path: &Path, tokenizer_path: &Path) -> Result<Self, EmbeddingError> {
        let builder = Session::builder().map_err(|e| EmbeddingError::Session {
            path: model_path.display().to_string(),
            source: e,
        })?;
        let mut builder = builder
            .with_execution_providers([ep::CPU::default().build()])
            .map_err(|e| {
                EmbeddingError::BadOutput(format!("failed to register execution provider: {e}"))
            })?;
        let session =
            builder
                .commit_from_file(model_path)
                .map_err(|e| EmbeddingError::Session {
                    path: model_path.display().to_string(),
                    source: e,
                })?;
        let tokenizer =
            Tokenizer::from_file(tokenizer_path).map_err(|e| EmbeddingError::Tokenizer {
                path: tokenizer_path.display().to_string(),
                source: e,
            })?;
        Ok(Self {
            session: Mutex::new(session),
            tokenizer,
        })
    }

    /// Embeds `text` into a normalized `EMBEDDING_DIM`-vector. Truncates to `MAX_SEQ_LEN` (a
    /// library title is a handful of tokens; the cap only guards pathological input).
    pub fn embed(&self, text: &str) -> Result<Vec<f32>, EmbeddingError> {
        let encoding = self.tokenizer.encode(text, true)?;
        let ids: Vec<i64> = encoding
            .get_ids()
            .iter()
            .take(MAX_SEQ_LEN)
            .map(|&id| id as i64)
            .collect();
        let mask: Vec<i64> = encoding
            .get_attention_mask()
            .iter()
            .take(MAX_SEQ_LEN)
            .map(|&m| m as i64)
            .collect();
        let len = ids.len();
        // MiniLM uses token_type_ids; single-sequence input → all zeros.
        let type_ids = vec![0i64; len];

        let shape = vec![1i64, len as i64];
        let input_ids = Tensor::from_array((shape.clone(), ids))?;
        let attention_mask = Tensor::from_array((shape.clone(), mask.clone()))?;
        let token_type_ids = Tensor::from_array((shape, type_ids))?;

        let mut session = self
            .session
            .lock()
            .map_err(|_| EmbeddingError::BadOutput("embedding session poisoned".into()))?;
        let outputs = session.run(ort::inputs! {
            "input_ids" => input_ids,
            "attention_mask" => attention_mask,
            "token_type_ids" => token_type_ids,
        })?;
        let hidden = outputs["last_hidden_state"]
            .try_extract_tensor::<f32>()
            .map_err(|e| EmbeddingError::BadOutput(e.to_string()))?;
        let (shape, flat) = hidden;
        // `flat` is the [1, seq, 384] output flattened; rebuild with the shape the model
        // reported so the indexing below is obviously correct.
        let dims = shape.to_ixdyn();
        if dims.ndim() != 3 || dims[0] != 1 {
            return Err(EmbeddingError::BadOutput(format!(
                "last_hidden_state has shape {dims:?}, expected [1, seq, 384]"
            )));
        }
        let seq = dims[1];
        if seq != len {
            return Err(EmbeddingError::BadOutput(format!(
                "tokenizer produced {len} tokens but the model saw {seq}"
            )));
        }
        let arr = ndarray::ArrayD::from_shape_vec(dims, flat.to_vec())
            .map_err(|e| EmbeddingError::BadOutput(e.to_string()))?;

        // Mean pooling over the sequence dimension, masked by attention_mask. (For a single
        // non-padded sequence every mask entry is 1 — the mask only matters if padding is ever
        // introduced, and keeping the math obviously-correct costs nothing.)
        let mut pooled = vec![0f32; EMBEDDING_DIM];
        let mut denom = 0f32;
        for i in 0..len {
            if mask[i] == 1 {
                for (d, v) in pooled.iter_mut().enumerate() {
                    *v += arr[[0, i, d]];
                }
                denom += 1.0;
            }
        }
        if denom > 0.0 {
            for v in &mut pooled {
                *v /= denom;
            }
        }

        // L2 normalize.
        let norm = pooled.iter().map(|v| v * v).sum::<f32>().sqrt();
        if norm > 0.0 {
            for v in &mut pooled {
                *v /= norm;
            }
        }
        Ok(pooled)
    }
}

/// Cosine similarity between two normalized vectors (a plain dot product once both are L2-
/// normalized — `embed` guarantees that invariant).
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    a.iter().zip(b).map(|(x, y)| x * y).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Real-model smoke test: requires the downloaded model under `data/models/` (see
    /// `model_download.rs` — the dev container's `./data/models` bind mount). Skips when absent
    /// so the crate's unit tests stay runnable on a fresh checkout without the 118MB download.
    #[test]
    fn real_model_embeds_and_normalizes() {
        let models_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/models");
        let model = models_dir.join("paraphrase-multilingual-MiniLM-L12-v2_quantized.onnx");
        let tok = models_dir.join("tokenizer.json");
        if !model.exists() || !tok.exists() {
            eprintln!("skipping: model files not present under data/models/ — run the model download first");
            return;
        }
        let embedder = Embedder::load(&model, &tok).expect("model must load");
        let a = embedder
            .embed("架空アンソロジー 銀花猫 巻の拾参")
            .unwrap();
        let b = embedder
            .embed("[アンソロジー] 架空アンソロジー 銀花猫 巻の弐")
            .unwrap();
        let c = embedder
            .embed("銀花猫 架空アンソロジー Vol.1")
            .unwrap();
        assert_eq!(a.len(), EMBEDDING_DIM);
        let norm_a: f32 = a.iter().map(|v| v * v).sum();
        assert!(
            (norm_a - 1.0).abs() < 1e-3,
            "embedding must be L2-normalized, got {norm_a}"
        );
        // Same-series titles (銀花猫) must be far closer to each other than to the other series.
        let same = cosine_similarity(&a, &b);
        let cross = cosine_similarity(&a, &c);
        assert!(
            same > cross,
            "same-series similarity {same} must exceed cross-series {cross}"
        );
    }
}
