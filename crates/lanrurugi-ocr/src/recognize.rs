//! Text **recognition** — transcribing the text inside an already-detected region (research.md §1).
//!
//! This is a custom `ort` integration against kha-white's Apache-2.0 `manga-ocr` weights (a
//! ViT encoder + autoregressive decoder), written independently. Koharu, which solves the same
//! problem, is GPL-3.0/`publish = false` and was consulted for architecture only — no code from it
//! is used here. A manga-specific model is used rather than PP-OCR's generic recogniser because
//! manga lettering is frequently vertical (tategaki), stylised, and furigana-annotated, which
//! document-trained recognisers handle poorly.
//!
//! Session construction mirrors `lanrurugi-recommend::embedding`'s `make_cpu_session` shape: the
//! CPU execution provider is registered **explicitly** as the fallback, because a GPU-flavoured
//! `libonnxruntime.so` can hang indefinitely in `commit_from_file` when no EP is registered at all
//! (research.md §1's OpenVINO incident). GPU EP integration plan v8 (issue #103) adds CUDA as the
//! first attempt, CPU as the always-explicit fallback — see `try_cuda_session`'s own doc comment.

use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use image::{imageops::FilterType, RgbImage};
use ort::ep;
use ort::ep::ExecutionProvider;
use ort::session::Session;
use ort::value::Tensor;
use thiserror::Error;

use crate::model_discovery::ModelPaths;

/// `manga-ocr` consumes 224x224 RGB crops (its ViT encoder's fixed input geometry).
const IMAGE_SIZE: u32 = 224;
/// Channel-wise normalisation constants baked into the model's preprocessing.
const IMAGE_MEAN: f32 = 0.5;
const IMAGE_STD: f32 = 0.5;
/// Hard stop on generated tokens — a single manga text region is short, and this bounds a
/// pathological non-terminating decode.
///
/// This bound alone isn't tight enough on its own: a real 2026-09-15 incident found one region
/// (post-merge.rs-rewrite, exact crop not yet root-caused) whose decoder never emitted `sep`/`pad`
/// and ran all 300 steps — each step is one full `ort::Session::run` with an ever-growing
/// `input_ids`, and that many real GPU round trips totalled close enough to this worker's own 60s
/// RPC-call timeout (`gpu_worker_client::CALL_TIMEOUT`) that the *whole worker process* got killed
/// as a `DeadlineExceeded` casualty — collateral damage to every other in-flight/queued region on
/// the page, not just the one bad crop. See [`REPEATED_TOKEN_STOP`] for the actual fix.
const MAX_DECODE_TOKENS: usize = 300;

/// Consecutive-identical-token early stop, checked every decode step alongside
/// [`MAX_DECODE_TOKENS`]. A healthy greedy decode moving toward `sep` doesn't emit the same token
/// four times in a row (real manga text has genuine repeated characters, e.g. "ああああ", but not
/// runs this long at greedy-argmax confidence) — once self-attention locks onto a token, geometric
/// decoding has no natural mechanism to escape that groove on its own, so a real repeat streak this
/// long is the observable signature of the non-terminating case [`MAX_DECODE_TOKENS`]'s own doc
/// comment describes, not a false positive waiting to happen. Stopping at 4 instead of riding out
/// to 300 turns a several-minute worker-killing hang into a sub-second regional recognition
/// failure (this region alone renders untranslated, same as any other recognition error —
/// `lanrurugi_ocr::batch::run_batch`'s own per-region isolation already handles that).
const REPEATED_TOKEN_STOP: usize = 4;

// GPU EP integration plan v9 (issue #103): per-session CUDA memory cap used to be a fixed
// constant here (384MiB, tuned against one specific 8GB card). Now resolved dynamically at
// startup instead — see `lanrurugi-gpu-worker::vram_budget` (the crate that actually calls
// `TextRecognizer::load`) for the hardware-proportional percent/min/max calculation, and why a
// fixed byte count either starves a smaller card or wastes VRAM a larger one doesn't need to
// reserve at all. This crate no longer owns that number — `load`'s own `cuda_memory_limit_bytes`
// parameter is the caller's resolved value.

#[derive(Debug, Error)]
pub enum RecognitionError {
    #[error("failed to build ORT session from {path}: {source}")]
    Session {
        path: String,
        #[source]
        source: ort::Error,
    },
    #[error("failed to load vocabulary from {path}: {source}")]
    Vocab {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to run ORT inference: {0}")]
    Inference(#[from] ort::Error),
    #[error("unexpected model output: {0}")]
    BadOutput(String),
}

/// The special-token ids `manga-ocr`'s vocabulary reserves. Resolved from `vocab.txt` by name
/// rather than hardcoded, so a vocabulary revision can't silently shift them.
#[derive(Debug, Clone, Copy)]
struct SpecialTokens {
    cls: i64,
    sep: i64,
    pad: i64,
}

struct Sessions {
    encoder: Session,
    decoder: Session,
}

/// The encoder/decoder pair, plus whether it's safe to ever drop.
///
/// GPU EP integration plan v9 (issue #103, see `.debug-scratch/GPU_EP_FINAL_SUMMARY.md` §3): a
/// real hardware spike found dropping a CUDA session triggers a reproducible ~35-60% crash rate
/// rooted in NVIDIA's own driver userspace component, not in this project's code, `ort`, or
/// onnxruntime — the fix is a *compile-time* guarantee that a CUDA session's backing memory is
/// never reclaimed (`Box::leak`). Both sessions in `Sessions` share one `Mutex` (see
/// `TextRecognizer`'s own doc comment for why), so leaking is all-or-nothing at the `Sessions`
/// level, not per-field — if *either* encoder or decoder ended up on CUDA, the whole `Mutex` must
/// be leaked, since it's one allocation backing both. This same `ManagedSessions`-shaped pattern
/// is duplicated in `lanrurugi-inpaint`/`bubble_segment` rather than shared — these crates don't
/// share a dependency that would let it live in one place.
enum ManagedSessions {
    /// Normal ownership. The two `bool`s are whether the encoder/decoder respectively actually
    /// ended up on CUDA — recorded at construction time by `build_session_with_gpu_fallback`,
    /// since `ort::Session` itself exposes no way to ask which EP it ended up on after the fact.
    Owned(Mutex<Sessions>, bool, bool),
    /// At least one of encoder/decoder ended up on CUDA, deliberately leaked at
    /// [`TextRecognizer::leak_cuda_sessions`] time.
    LeakedCuda(&'static Mutex<Sessions>),
}

impl ManagedSessions {
    fn with_locked<R>(&self, f: impl FnOnce(&mut Sessions) -> R) -> Result<R, RecognitionError> {
        let mutex: &Mutex<Sessions> = match self {
            ManagedSessions::Owned(mutex, ..) => mutex,
            ManagedSessions::LeakedCuda(mutex) => mutex,
        };
        let mut guard = mutex
            .lock()
            .map_err(|_| RecognitionError::BadOutput("recognition session poisoned".into()))?;
        Ok(f(&mut guard))
    }
}

/// A loaded `manga-ocr` recogniser.
///
/// Both sessions sit behind one `Mutex` because `ort`'s `run` takes `&mut self` and a decode
/// interleaves encoder and decoder calls — locking them together keeps a single region's decode
/// loop atomic instead of letting two concurrent recognitions interleave their decoder steps.
pub struct TextRecognizer {
    sessions: ManagedSessions,
    vocab: Vec<String>,
    special: SpecialTokens,
}

impl std::fmt::Debug for TextRecognizer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TextRecognizer")
            .field("vocab_size", &self.vocab.len())
            .finish_non_exhaustive()
    }
}

impl TextRecognizer {
    /// Loads the encoder/decoder sessions and vocabulary.
    ///
    /// `intra_threads` caps ONNX Runtime's *within-one-inference* thread count — the same knob
    /// `lanrurugi-recommend::embedding` documents: callers get real parallelism from batching at
    /// the rayon level ([`crate::batch`]), not from concurrent calls here, which serialise on the
    /// session lock.
    ///
    /// GPU EP integration plan v8 (issue #103): the encoder and decoder each try CUDA
    /// independently and fall back to CPU on their own — deliberately not coordinated with each
    /// other, same reasoning as `lanrurugi-inpaint::Inpainter::load`'s own doc comment (dropping
    /// an already-successful CUDA session to keep both on the same EP is exactly the operation
    /// with a reproducible ~35-60% crash rate on this stack; see `.debug-scratch/
    /// GPU_EP_FINAL_SUMMARY.md`). Encoder-on-GPU/decoder-on-CPU (or vice versa) is a real possible
    /// outcome — a performance asymmetry, not a correctness problem.
    pub fn load(
        paths: &ModelPaths,
        intra_threads: usize,
        cuda_memory_limit_bytes: usize,
    ) -> Result<Self, RecognitionError> {
        let (encoder, encoder_is_cuda) = build_session_with_gpu_fallback(
            &paths.encoder,
            intra_threads,
            cuda_memory_limit_bytes,
        )?;
        let (decoder, decoder_is_cuda) = build_session_with_gpu_fallback(
            &paths.decoder,
            intra_threads,
            cuda_memory_limit_bytes,
        )?;
        let (vocab, special) = load_vocab(&paths.vocab)?;

        Ok(Self {
            sessions: ManagedSessions::Owned(
                Mutex::new(Sessions { encoder, decoder }),
                encoder_is_cuda,
                decoder_is_cuda,
            ),
            vocab,
            special,
        })
    }

    /// Leaks the encoder/decoder pair's shared backing memory if (and only if) at least one of
    /// them is actually running on CUDA — a no-op (both stay owned) if both are on CPU. See
    /// `ManagedSessions`'s own doc comment for why leaking is all-or-nothing here (one shared
    /// `Mutex`, not two independent ones), and why this is a separate, caller-invoked step rather
    /// than something [`Self::load`] itself does (this crate's own tests would otherwise leak
    /// real CUDA memory, unreclaimable until process exit, on every single `load` call).
    ///
    /// Takes/returns `self` by value, same as `lanrurugi-inpaint::Inpainter`'s and
    /// `bubble_segment::BubbleSegmenter`'s identical methods.
    #[must_use]
    pub fn leak_cuda_sessions(self) -> Self {
        let Self {
            sessions,
            vocab,
            special,
        } = self;
        let sessions = match sessions {
            ManagedSessions::Owned(mutex, encoder_is_cuda, decoder_is_cuda)
                if encoder_is_cuda || decoder_is_cuda =>
            {
                ManagedSessions::LeakedCuda(Box::leak(Box::new(mutex)))
            }
            other => other,
        };
        Self {
            sessions,
            vocab,
            special,
        }
    }

    /// Transcribes one already-cropped text region.
    pub fn recognize(&self, crop: &RgbImage) -> Result<String, RecognitionError> {
        let pixels = preprocess(crop);
        let special = self.special;

        // The whole encoder call + decode loop runs inside one `with_locked` closure — its
        // `SessionOutputs`/extracted tensor slices borrow from the `&mut Sessions` `with_locked`
        // hands in, and that borrow can't outlive the closure itself, so this only ever returns
        // `tokens` (a real owned `Vec<i64>`), never anything still borrowing a session.
        let tokens: Vec<i64> = self.sessions.with_locked(
            |sessions| -> Result<Vec<i64>, RecognitionError> {
                // --- Encoder: image -> hidden states -----------------------------------------
                // Scoped so the borrow of `sessions.encoder` (held by `SessionOutputs`) ends
                // before the decode loop borrows `sessions.decoder`.
                let (hidden_shape, hidden_flat) = {
                    let pixel_values = Tensor::from_array((
                        vec![1i64, 3, i64::from(IMAGE_SIZE), i64::from(IMAGE_SIZE)],
                        pixels,
                    ))?;
                    let encoder_out = sessions.encoder.run(ort::inputs! {
                        "pixel_values" => pixel_values,
                    })?;
                    let (shape, flat) = encoder_out["last_hidden_state"]
                        .try_extract_tensor::<f32>()
                        .map_err(|e| RecognitionError::BadOutput(e.to_string()))?;
                    (shape.iter().copied().collect::<Vec<i64>>(), flat.to_vec())
                };

                // --- Decoder: greedy autoregressive decode -----------------------------------
                // Greedy (argmax) rather than beam search: a text region is short, greedy is what
                // `manga-ocr` itself is evaluated with, and beam search would multiply decode
                // cost for a marginal gain on this input length.
                let mut tokens: Vec<i64> = vec![special.cls];

                for _ in 0..MAX_DECODE_TOKENS {
                    // The next token id is computed inside this scope so every borrow of
                    // `sessions.decoder` ends before `tokens` is mutated below.
                    let next = {
                        let input_ids = Tensor::from_array((
                            vec![1i64, tokens.len() as i64],
                            tokens.clone(),
                        ))?;
                        let encoder_hidden =
                            Tensor::from_array((hidden_shape.clone(), hidden_flat.clone()))?;

                        let decoder_out = sessions.decoder.run(ort::inputs! {
                            "input_ids" => input_ids,
                            "encoder_hidden_states" => encoder_hidden,
                        })?;

                        let (logit_shape, logits) = decoder_out["logits"]
                            .try_extract_tensor::<f32>()
                            .map_err(|e| RecognitionError::BadOutput(e.to_string()))?;

                        // [batch, seq, vocab] — only the final position's distribution matters
                        // for the next token.
                        if logit_shape.len() != 3 {
                            return Err(RecognitionError::BadOutput(format!(
                                "decoder logits have shape {logit_shape:?}, expected [batch, seq, vocab]"
                            )));
                        }
                        let vocab_size = logit_shape[2] as usize;
                        let seq_len = logit_shape[1] as usize;
                        if vocab_size == 0 || seq_len == 0 {
                            return Err(RecognitionError::BadOutput("empty decoder output".into()));
                        }
                        let last_offset = (seq_len - 1) * vocab_size;

                        logits[last_offset..last_offset + vocab_size]
                            .iter()
                            .enumerate()
                            .max_by(|(_, a), (_, b)| a.total_cmp(b))
                            .map(|(i, _)| i as i64)
                            .ok_or_else(|| RecognitionError::BadOutput("empty logit row".into()))?
                    };

                    if next == special.sep || next == special.pad {
                        break;
                    }
                    tokens.push(next);

                    // `tokens[0]` is always `special.cls`, never part of a real repeat streak —
                    // see `REPEATED_TOKEN_STOP`'s own doc comment for why this check exists at all.
                    if ends_in_repeat_streak(&tokens[1..], REPEATED_TOKEN_STOP) {
                        break;
                    }
                }

                Ok(tokens)
            },
        )??;

        Ok(decode_tokens(&self.vocab, &tokens[1..]))
    }
}

/// Maps generated token ids back to text.
///
/// `manga-ocr`'s vocabulary is WordPiece: `##`-prefixed pieces continue the previous token rather
/// than starting a new one. Japanese output is not space-delimited, so pieces are concatenated
/// directly — inserting spaces between them would corrupt the text.
///
/// Whether `generated`'s last `streak` tokens are all identical — the free-function half of
/// [`REPEATED_TOKEN_STOP`]'s own doc comment, split out so it stays testable without constructing
/// (or faking) the ORT sessions a [`TextRecognizer`] otherwise owns.
fn ends_in_repeat_streak(generated: &[i64], streak: usize) -> bool {
    generated.len() >= streak
        && generated[generated.len() - streak..]
            .windows(2)
            .all(|w| w[0] == w[1])
}

/// A free function rather than a method so it stays testable without constructing (or faking) the
/// ORT sessions a [`TextRecognizer`] otherwise owns.
fn decode_tokens(vocab: &[String], tokens: &[i64]) -> String {
    let mut out = String::new();
    for &id in tokens {
        let Some(piece) = vocab.get(id as usize) else {
            continue;
        };
        if piece.starts_with('[') && piece.ends_with(']') {
            continue; // Any remaining special token.
        }
        out.push_str(piece.strip_prefix("##").unwrap_or(piece));
    }
    out.trim().to_string()
}

/// Rejects a `recognize()` result that doesn't look like real Japanese text — the sanity check
/// `manga-ocr`'s own architecture has no equivalent of (it's greedy-decoded with no per-token
/// confidence exposed here, research.md's decode loop always emits *some* string). Highly
/// stylised/deformed SFX lettering (a common real case: hand-drawn onomatopoeia stretched, warped,
/// or blended into the artwork) is exactly the input this model was never trained to transcribe
/// reliably, and a failed transcription doesn't fail loud — it silently returns plausible-looking
/// garbage (e.g. `"N-b!»7/26:8"` for a hand-drawn "ロド" SFX, confirmed live, 2026-09-07) that then
/// gets "translated" verbatim (the LLM has no way to tell garbled OCR output from real source text
/// either) and composited over the original artwork, replacing legible lettering with a garbled
/// block — worse than leaving the region untouched. A legitimate Japanese OCR result is dominated
/// by Hiragana/Katakana/Kanji/full-width punctuation; requiring at least half the non-whitespace
/// characters to be one of those is a coarse but effective filter for this failure mode without
/// needing model-level confidence scores.
pub fn looks_like_japanese(text: &str) -> bool {
    let mut total = 0usize;
    let mut japanese = 0usize;
    for c in text.chars() {
        if c.is_whitespace() {
            continue;
        }
        total += 1;
        let cp = c as u32;
        let is_japanese_script = matches!(cp,
            0x3040..=0x309F // Hiragana
            | 0x30A0..=0x30FF // Katakana
            | 0x4E00..=0x9FFF // CJK Unified Ideographs (Kanji)
            | 0x3000..=0x303F // CJK punctuation (、。「」etc.)
            | 0xFF00..=0xFFEF // Fullwidth forms (！？ｎ etc., common in SFX lettering)
        );
        if is_japanese_script {
            japanese += 1;
        }
    }
    total > 0 && japanese * 2 >= total
}

/// Builds one CPU-only ORT session.
///
/// `ort::Error` is generic over the builder stage it came from, so each step maps its own error
/// type rather than sharing one closure.
/// Tries to build and commit a CUDA-registered session in one atomic attempt — registration and
/// `commit_from_file` are both inside the same `?` chain, so a failure at either step (not just
/// registration) triggers the caller's CPU fallback. `.error_on_failure()` makes registration
/// failure return a real `Err` instead of silently falling back to CPU inside `ort` itself.
fn try_cuda_session(
    path: &Path,
    intra_threads: usize,
    cuda_memory_limit_bytes: usize,
) -> ort::Result<Session> {
    let builder = Session::builder()?
        .with_execution_providers([ep::CUDA::default()
            .with_memory_limit(cuda_memory_limit_bytes)
            // See `lanrurugi-inpaint::try_cuda_session`'s identical call for the full rationale —
            // `ort`'s CUDA EP defaults to an unbounded-workspace exhaustive conv-algorithm search
            // regardless of `with_memory_limit`; this caps that separately.
            .with_conv_algorithm_search(ep::cuda::ConvAlgorithmSearch::Heuristic)
            .with_conv_max_workspace(false)
            .build()
            .error_on_failure()])?
        .with_intra_threads(intra_threads.max(1))?;
    let mut builder = builder;
    builder.commit_from_file(path)
}

/// Tries to build and commit an OpenVINO-registered session (Intel CPUs/GPUs/NPUs) in one atomic
/// attempt, same shape as `try_cuda_session` above. Only ever reached when `ep::OpenVINO::
/// default().is_available()` has already confirmed the *loaded* `libonnxruntime.so` actually has
/// OpenVINO provider code compiled in (see `build_session_with_gpu_fallback`) — registering this
/// EP against a build that lacks it is what issue #103's research.md §1 documents hanging
/// indefinitely inside `session.run()` rather than failing cleanly, the one EP-safety exception
/// this project's own `try`/fallback pattern doesn't protect against on its own.
///
/// `with_device_type("GPU")`, not left at the OpenVINO EP's own default (which auto-selects, may
/// land on CPU) — an explicit `GPU` request means a genuine registration failure here (no Intel
/// GPU present, or the specific one present isn't OpenVINO-GPU-capable) surfaces as an `Err` the
/// caller can fall back from, rather than silently succeeding on OpenVINO-CPU and defeating the
/// entire point of preferring this path over the plain CPU EP.
fn try_openvino_session(path: &Path, intra_threads: usize) -> ort::Result<Session> {
    let builder = Session::builder()?
        .with_execution_providers([ep::OpenVINO::default()
            .with_device_type("GPU")
            .build()
            .error_on_failure()])?
        .with_intra_threads(intra_threads.max(1))?;
    let mut builder = builder;
    builder.commit_from_file(path)
}

/// Builds one ORT session, CUDA first, then OpenVINO, falling back to CPU if neither is available
/// or both fail (see `TextRecognizer::load`'s own doc comment for why encoder/decoder decide this
/// independently). The returned `bool` is whether the session actually ended up on *some* GPU EP
/// (CUDA or OpenVINO) — `ManagedSessions`/`TextRecognizer::leak_cuda_sessions` need this to know
/// whether the pair is safe to leave owned or must eventually be leaked; `ort::Session` itself
/// exposes no way to ask this after construction, so it has to be captured here, at the one point
/// that actually knows. Only one of CUDA/OpenVINO is ever attempted per process — see
/// `crate::gpu_worker_client::GpuVendor`'s own doc comment: the *loaded* `libonnxruntime.so` is
/// picked once, before this worker process even starts, based on which GPU vendor `lspci` finds on
/// the host, so `ep::CUDA::is_available()` and `ep::OpenVINO::is_available()` are never both `true`
/// in the same process — whichever vendor's build got loaded is the only one with its EP compiled
/// in at all.
fn build_session_with_gpu_fallback(
    path: &Path,
    intra_threads: usize,
    cuda_memory_limit_bytes: usize,
) -> Result<(Session, bool), RecognitionError> {
    // Named `path_display`, not `display` — collides with `tracing::field::display`, which the
    // `%field` shorthand below expands to call.
    let path_display = || path.display().to_string();

    // `LANRURUGI_DISABLE_GPU` (GPU EP integration plan v8 §4.6) — see
    // `lanrurugi-ocr::bubble_segment`'s identical check for the full rationale.
    if std::env::var_os("LANRURUGI_DISABLE_GPU").is_some() {
        tracing::info!("LANRURUGI_DISABLE_GPU is set; using CPU for recognition");
        return build_cpu_session(path, intra_threads).map(|session| (session, false));
    }

    let cuda_availability = ep::CUDA::default().is_available();
    match cuda_availability {
        Ok(true) => match try_cuda_session(path, intra_threads, cuda_memory_limit_bytes) {
            Ok(session) => {
                tracing::info!(path = %path_display(), "recognition model loaded on CUDA");
                return Ok((session, true));
            }
            Err(error) => {
                tracing::warn!(path = %path_display(), %error, "CUDA session build failed for recognition model, falling back to CPU");
            }
        },
        Ok(false) => {
            tracing::debug!(
                "CUDA execution provider not compiled into the loaded ONNX Runtime build; \
                 checking OpenVINO"
            );
        }
        Err(error) => {
            tracing::error!(%error, "is_available() reported an internal ONNX Runtime error while probing CUDA; checking OpenVINO");
        }
    }

    // Only reached when CUDA wasn't compiled in (never after a CUDA registration *failure* —
    // that case already fell through to CPU above, matching this crate's pre-existing behavior).
    // See `try_openvino_session`'s own doc comment for why `is_available()` gating this call is
    // load-bearing, not just an optimization.
    let openvino_availability = ep::OpenVINO::default().is_available();
    match openvino_availability {
        Ok(true) => match try_openvino_session(path, intra_threads) {
            Ok(session) => {
                tracing::info!(path = %path_display(), "recognition model loaded on OpenVINO");
                return Ok((session, true));
            }
            Err(error) => {
                tracing::warn!(path = %path_display(), %error, "OpenVINO session build failed for recognition model, falling back to CPU");
            }
        },
        Ok(false) => {
            tracing::debug!(
                "OpenVINO execution provider not compiled into the loaded ONNX Runtime build; \
                 using CPU for recognition"
            );
        }
        Err(error) => {
            tracing::error!(%error, "is_available() reported an internal ONNX Runtime error while probing OpenVINO; falling back to CPU for recognition");
        }
    }

    build_cpu_session(path, intra_threads).map(|session| (session, false))
}

/// Builds one CPU-only ORT session — explicit registration, never omitted (research.md §1's
/// OpenVINO hang incident is why a GPU-flavoured `libonnxruntime.so` is never trusted with an
/// unregistered EP list — CUDA is only ever attempted explicitly, above, not implicitly here).
fn build_cpu_session(path: &Path, intra_threads: usize) -> Result<Session, RecognitionError> {
    let path_display = || path.display().to_string();

    let builder = Session::builder().map_err(|source| RecognitionError::Session {
        path: path_display(),
        source,
    })?;

    let builder = builder
        .with_execution_providers([ep::CPU::default().build()])
        .map_err(|source| RecognitionError::Session {
            path: path_display(),
            source: source.into(),
        })?;

    let mut builder = builder
        .with_intra_threads(intra_threads.max(1))
        .map_err(|source| RecognitionError::Session {
            path: path_display(),
            source: source.into(),
        })?;

    builder
        .commit_from_file(path)
        .map_err(|source| RecognitionError::Session {
            path: path_display(),
            source,
        })
}

/// Reads `vocab.txt` (one token per line, id = line number) and locates the special tokens.
fn load_vocab(path: &Path) -> Result<(Vec<String>, SpecialTokens), RecognitionError> {
    let text = std::fs::read_to_string(path).map_err(|source| RecognitionError::Vocab {
        path: path.display().to_string(),
        source,
    })?;

    let vocab: Vec<String> = text.lines().map(|l| l.trim_end().to_string()).collect();
    let index: HashMap<&str, i64> = vocab
        .iter()
        .enumerate()
        .map(|(i, t)| (t.as_str(), i as i64))
        .collect();

    // Fall back to the conventional BERT ids only if a name is genuinely absent, so a vocabulary
    // that orders them differently still works.
    let special = SpecialTokens {
        cls: index.get("[CLS]").copied().unwrap_or(2),
        sep: index.get("[SEP]").copied().unwrap_or(3),
        pad: index.get("[PAD]").copied().unwrap_or(0),
    };

    Ok((vocab, special))
}

/// Resizes to the model's fixed geometry and normalises into CHW float input.
fn preprocess(crop: &RgbImage) -> Vec<f32> {
    let resized = image::imageops::resize(crop, IMAGE_SIZE, IMAGE_SIZE, FilterType::CatmullRom);

    let pixel_count = (IMAGE_SIZE * IMAGE_SIZE) as usize;
    let mut chw = vec![0f32; pixel_count * 3];

    for (i, px) in resized.pixels().enumerate() {
        for c in 0..3 {
            let v = f32::from(px.0[c]) / 255.0;
            chw[c * pixel_count + i] = (v - IMAGE_MEAN) / IMAGE_STD;
        }
    }
    chw
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb as ImageRgb;

    #[test]
    fn preprocess_produces_normalized_chw_layout() {
        let img = RgbImage::from_pixel(10, 10, ImageRgb([255, 255, 255]));
        let out = preprocess(&img);
        assert_eq!(out.len(), (IMAGE_SIZE * IMAGE_SIZE) as usize * 3);
        // White maps to (1.0 - 0.5) / 0.5 == 1.0 in every channel.
        assert!(out.iter().all(|v| (v - 1.0).abs() < 1e-5));
    }

    #[test]
    fn preprocess_maps_black_to_negative_one() {
        let img = RgbImage::from_pixel(8, 8, ImageRgb([0, 0, 0]));
        let out = preprocess(&img);
        assert!(out.iter().all(|v| (v + 1.0).abs() < 1e-5));
    }

    #[test]
    fn vocab_special_tokens_are_resolved_by_name() {
        let dir = std::env::temp_dir().join("lanrurugi-ocr-vocab-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vocab.txt");
        std::fs::write(&path, "[PAD]\n[UNK]\n[CLS]\n[SEP]\nあ\n##い\n").unwrap();

        let (vocab, special) = load_vocab(&path).unwrap();
        assert_eq!(vocab.len(), 6);
        assert_eq!(special.cls, 2);
        assert_eq!(special.sep, 3);
        assert_eq!(special.pad, 0);

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Decoding is the half of recognition that is testable without the real ~460MB model.
    fn test_vocab() -> Vec<String> {
        vec![
            "[PAD]".into(),
            "[UNK]".into(),
            "[CLS]".into(),
            "[SEP]".into(),
            "こん".into(),
            "##にち".into(),
            "##は".into(),
        ]
    }

    #[test]
    fn wordpiece_continuations_join_without_spaces() {
        assert_eq!(decode_tokens(&test_vocab(), &[4, 5, 6]), "こんにちは");
    }

    #[test]
    fn special_tokens_are_dropped_from_decoded_text() {
        assert_eq!(decode_tokens(&test_vocab(), &[4, 3, 5]), "こんにち");
    }

    #[test]
    fn out_of_range_token_ids_are_skipped() {
        assert_eq!(decode_tokens(&test_vocab(), &[4, 9999, 5]), "こんにち");
    }

    #[test]
    fn a_run_of_identical_tokens_at_the_stop_length_is_a_repeat_streak() {
        assert!(ends_in_repeat_streak(
            &[4, 5, 7, 7, 7, 7],
            REPEATED_TOKEN_STOP
        ));
    }

    #[test]
    fn a_shorter_run_than_the_stop_length_is_not_a_repeat_streak() {
        assert!(!ends_in_repeat_streak(
            &[4, 5, 7, 7, 7],
            REPEATED_TOKEN_STOP
        ));
    }

    #[test]
    fn normal_varying_tokens_are_not_a_repeat_streak() {
        assert!(!ends_in_repeat_streak(
            &[4, 5, 6, 4, 5, 6],
            REPEATED_TOKEN_STOP
        ));
    }

    #[test]
    fn a_repeat_streak_earlier_in_the_sequence_does_not_count_once_it_ends() {
        // The streak broke two tokens ago — only the tail matters, not whether one ever occurred.
        assert!(!ends_in_repeat_streak(
            &[7, 7, 7, 7, 5, 6],
            REPEATED_TOKEN_STOP
        ));
    }
}
