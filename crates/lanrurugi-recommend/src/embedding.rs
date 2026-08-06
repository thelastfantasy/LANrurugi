//! Text embedding via ONNX Runtime — the model the reader recommender uses for series
//! recognition. Model-agnostic within the sentence-transformers BERT-family convention: see
//! `model_download.rs`'s own docs for how the actual model file is selected (default
//! `intfloat/multilingual-e5-small`, env-var overridable to point at a different model file
//! entirely — see that module for the accuracy comparison that picked the default and
//! `crates/lanrurugi-recommend/examples/eval_fixture.rs` for the tool to re-run that comparison
//! against a new candidate before switching).
//!
//! Standard sentence-transformers pipeline: BERT WordPiece tokenize (the Rust `tokenizers` crate,
//! loading `tokenizer.json`), ONNX inference (inputs `input_ids` / `attention_mask` /
//! `token_type_ids`, output `last_hidden_state`), **mean pooling** over the token dimension masked
//! by `attention_mask`, then **L2 normalization** — so cosine similarity between two embedded
//! titles is a plain dot product. The pooling config (mean pooling + L2 normalize) is the
//! standard sentence-transformers `1_Pooling` setting shared by every model this module has been
//! run against so far, hardcoded here (a config.json read would only re-confirm it) — a future
//! swapped-in model using a genuinely different pooling strategy (e.g. CLS-token pooling) would
//! need this module's own pooling logic updated, not just a config change. The embedding
//! dimensionality itself is NOT hardcoded — it's read from the model's own `last_hidden_state`
//! output shape on every call (see `embed()`), so a differently-sized model swapped in via env
//! var doesn't silently corrupt the pooling math the way a hardcoded dimension constant would.
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

/// Context window shared by every BERT-family model this has been run against so far (MiniLM-L12
/// and multilingual-e5-small both report 512 as their `max_position_embeddings`). A library title
/// is a handful of tokens; this cap only guards pathological input, and a future swapped-in model
/// with a smaller window would just get truncated more aggressively, not broken.
const MAX_SEQ_LEN: usize = 512;

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
    /// `intra_threads` caps how many threads ONNX Runtime uses *inside* a single `run()` call to
    /// parallelize one inference's own graph ops — the knob that actually matters for the
    /// precompute backfill job (issue #70): `session` above is `Mutex`-wrapped, so spawning more
    /// concurrent callers of `embed()` never buys real parallelism, they just queue on that lock.
    /// Real throughput on a multi-core box comes only from this session-internal thread count.
    /// The live request path (`recommend.rs`) passes a small fixed value (contending with normal
    /// request handling); the batch precompute job (`recommend_precompute.rs`) passes its own
    /// CPU-budget calculation instead.
    pub fn load(
        model_path: &Path,
        tokenizer_path: &Path,
        intra_threads: usize,
    ) -> Result<Self, EmbeddingError> {
        let builder = Session::builder().map_err(|e| EmbeddingError::Session {
            path: model_path.display().to_string(),
            source: e,
        })?;
        let builder = builder
            .with_execution_providers([ep::CPU::default().build()])
            .map_err(|e| {
                EmbeddingError::BadOutput(format!("failed to register execution provider: {e}"))
            })?;
        let mut builder = builder
            .with_intra_threads(intra_threads.max(1))
            .map_err(|e| {
                EmbeddingError::BadOutput(format!("failed to set intra-op thread count: {e}"))
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

    /// Embeds `text` into a normalized vector — length is whatever the loaded model's own
    /// `last_hidden_state` output reports (see this module's own docs). Truncates to
    /// `MAX_SEQ_LEN` (a library title is a handful of tokens; the cap only guards pathological
    /// input).
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
        // `flat` is the [1, seq, hidden_dim] output flattened; rebuild with the shape the model
        // itself reported so both the indexing below AND the output vector length are correct
        // for whatever model is actually loaded — no hardcoded dimension constant to fall out of
        // sync with a swapped-in model (see this module's own docs).
        let dims = shape.to_ixdyn();
        if dims.ndim() != 3 || dims[0] != 1 {
            return Err(EmbeddingError::BadOutput(format!(
                "last_hidden_state has shape {dims:?}, expected [1, seq, hidden_dim]"
            )));
        }
        let seq = dims[1];
        if seq != len {
            return Err(EmbeddingError::BadOutput(format!(
                "tokenizer produced {len} tokens but the model saw {seq}"
            )));
        }
        let hidden_dim = dims[2];
        let arr = ndarray::ArrayD::from_shape_vec(dims, flat.to_vec())
            .map_err(|e| EmbeddingError::BadOutput(e.to_string()))?;

        // Mean pooling over the sequence dimension, masked by attention_mask. (For a single
        // non-padded sequence every mask entry is 1 — the mask only matters if padding is ever
        // introduced, and keeping the math obviously-correct costs nothing.)
        let mut pooled = vec![0f32; hidden_dim];
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
    /// `model_download.rs` — the dev container's `./data/models` bind mount) AND three real
    /// archive titles supplied via env var (`LANRURUGI_TEST_TITLE_SAME_SERIES_A`/`_B`/
    /// `LANRURUGI_TEST_TITLE_CROSS_SERIES` — copy `.env.example` to `.env.local` and fill them
    /// in; see that file's own comment). Real, copyrighted work titles don't belong hardcoded in
    /// source (same reasoning as `TEST_REAL_DOWNLOAD_URL` elsewhere in this repo). Skips (not
    /// fails) when either the model files or the env vars are absent, so the crate's unit tests
    /// stay runnable on a fresh checkout without the 118MB download or any external secrets.
    #[test]
    fn real_model_embeds_and_normalizes() {
        let models_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../data/models");
        let model = models_dir.join("multilingual-e5-small_quantized.onnx");
        let tok = models_dir.join("e5-tokenizer.json");
        if !model.exists() || !tok.exists() {
            eprintln!("skipping: model files not present under data/models/ — run the model download first");
            return;
        }
        let (Ok(title_a), Ok(title_b), Ok(title_c)) = (
            std::env::var("LANRURUGI_TEST_TITLE_SAME_SERIES_A"),
            std::env::var("LANRURUGI_TEST_TITLE_SAME_SERIES_B"),
            std::env::var("LANRURUGI_TEST_TITLE_CROSS_SERIES"),
        ) else {
            eprintln!(
                "skipping: LANRURUGI_TEST_TITLE_SAME_SERIES_A/_B/LANRURUGI_TEST_TITLE_CROSS_SERIES not set — see .env.example"
            );
            return;
        };
        let embedder = Embedder::load(&model, &tok, 1).expect("model must load");
        let a = embedder.embed(&title_a).unwrap();
        let b = embedder.embed(&title_b).unwrap();
        let c = embedder.embed(&title_c).unwrap();
        assert!(!a.is_empty(), "embedding must have a non-zero dimension");
        assert_eq!(
            a.len(),
            b.len(),
            "the same model must produce a consistent dimension across calls"
        );
        let norm_a: f32 = a.iter().map(|v| v * v).sum();
        assert!(
            (norm_a - 1.0).abs() < 1e-3,
            "embedding must be L2-normalized, got {norm_a}"
        );
        // The two same-series titles (env vars A/B) must be far closer to each other than to the
        // cross-series title (env var C).
        let same = cosine_similarity(&a, &b);
        let cross = cosine_similarity(&a, &c);
        assert!(
            same > cross,
            "same-series similarity {same} must exceed cross-series {cross}"
        );
    }
}
