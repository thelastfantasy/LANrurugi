//! Manga speech-bubble instance segmentation — precise bubble masks (not just bounding boxes) fed
//! into `lanrurugi-translate::composite`'s page-mask merge for
//! `lanrurugi-inpaint::Inpainter::erase_page`'s shape-accurate erasure, rather than falling back
//! to a region's plain rectangular bounding box.
//!
//! Model: `huyvux3005/manga109-segmentation-bubble` (Apache-2.0, YOLO11n-seg fine-tuned on
//! Manga109-s + MangaSegmentation), converted from its only-published `.pt` weights to ONNX via
//! `scripts/fetch-bubble-seg-model.sh` (see that script's own docs on why this conversion, and why
//! `dmMaze/comic-text-detector` — GPL-3.0, license-incompatible — was excluded instead). Single
//! class ("balloon"), fixed 1600x1600 input, letterboxed (not stretched) per the model's own
//! training preprocessing.
//!
//! Decode follows Ultralytics' own YOLOv8/11-seg ONNX export contract: `output0`
//! `[1, 4+nc+nm, num_anchors]` (box already decoded to pixel-space `cx,cy,w,h` inside the graph —
//! not raw DFL logits, this export has no separate anchor-decode step left for callers to do;
//! class score already sigmoid-applied) and `output1` `[1, nm, mask_h, mask_w]` (mask prototypes,
//! mask grid at 1/4 the input resolution). A detection's own final mask is
//! `sigmoid(mask_coeffs · prototypes)`, cropped to that detection's own box (in mask-grid space)
//! and upsampled to the box's real pixel size — not the whole-image mask upsampled first, which
//! would be equivalent but do far more resize work than necessary.

pub mod model_discovery {
    pub use crate::bubble_model_discovery::*;
}

use std::path::Path;
use std::sync::Mutex;

use image::{imageops::FilterType, RgbImage};
use ort::ep;
use ort::ep::ExecutionProvider;
use ort::session::Session;
use ort::value::Tensor;
use thiserror::Error;

use crate::entities::BoundingBox;

/// Model's own fixed input resolution (training `imgsz`, see this module's own doc comment).
const INPUT_SIZE: u32 = 1600;
/// Mask prototype grid stride relative to `INPUT_SIZE` (standard YOLOv8/11-seg architecture).
const MASK_STRIDE: u32 = 4;
const MASK_GRID: u32 = INPUT_SIZE / MASK_STRIDE;
/// Number of mask prototype coefficients this model's head was trained with (Ultralytics default).
const NUM_MASK_COEFFS: usize = 32;
/// Below this confidence a detection is discarded before NMS even runs.
const CONFIDENCE_THRESHOLD: f32 = 0.25;
/// Two detections more overlapping than this are treated as the same bubble; the lower-confidence
/// one is dropped (standard greedy NMS).
const NMS_IOU_THRESHOLD: f32 = 0.45;
/// Mask-probability cutoff for the final binary mask.
const MASK_THRESHOLD: f32 = 0.5;

// GPU EP integration plan v9 (issue #103): per-session CUDA memory cap used to be a fixed
// constant here (768MiB, tuned against one specific 8GB card). Now resolved dynamically at
// startup instead — see `lanrurugi-gpu-worker::vram_budget` (the crate that actually calls
// `BubbleSegmenter::load`) for the hardware-proportional percent/min/max calculation. This
// crate no longer owns that number — `load`'s own `cuda_memory_limit_bytes` parameter is the
// caller's resolved value.

#[derive(Debug, Error)]
pub enum BubbleSegmentError {
    #[error("failed to build ORT session from {path}: {source}")]
    Session {
        path: String,
        #[source]
        source: ort::Error,
    },
    #[error("failed to run ORT inference: {0}")]
    Inference(#[from] ort::Error),
    #[error("unexpected model output shape: {0}")]
    BadOutput(String),
}

/// One detected speech bubble: its box plus a row-major binary mask exactly `box.w * box.h` long
/// (`mask[y * box.w + x]` — `true` where the bubble's real (non-rectangular) shape covers that
/// page pixel).
#[derive(Clone)]
pub struct DetectedBubble {
    pub bbox: BoundingBox,
    pub confidence: f32,
    pub mask: Vec<bool>,
}

/// Abstracts over "how a page's bubbles actually get detected" — same reasoning as
/// `crate::batch::TextRecognizerHandle`: GPU EP integration plan v9 (issue #103) moved the real
/// `BubbleSegmenter` (and its CUDA session) into a separate `lanrurugi-gpu-worker` subprocess, so
/// callers above this crate can no longer hold a concrete `BubbleSegmenter` directly without also
/// depending on `lanrurugi-api`. Synchronous for the same reason as that trait too — called from
/// inside a `spawn_blocking` closure, never a true async-reactor thread.
pub trait BubbleSegmenterHandle: Send + Sync {
    fn detect(&self, page: &RgbImage) -> Result<Vec<DetectedBubble>, String>;
}

impl BubbleSegmenterHandle for BubbleSegmenter {
    fn detect(&self, page: &RgbImage) -> Result<Vec<DetectedBubble>, String> {
        BubbleSegmenter::detect(self, page).map_err(|e| e.to_string())
    }
}

/// A session, plus whether it's safe to ever drop.
///
/// GPU EP integration plan v9 (issue #103, see `.debug-scratch/GPU_EP_FINAL_SUMMARY.md` §3): a
/// real hardware spike found dropping a CUDA session triggers a reproducible ~35-60% crash rate
/// rooted in NVIDIA's own driver userspace component, not in this project's code, `ort`, or
/// onnxruntime — the fix is a *compile-time* guarantee that a CUDA session's backing memory is
/// never reclaimed (`Box::leak`, not `std::mem::forget`: a leaked `Box` has no owner left to drop
/// at all, whereas `forget`-ing one of several `Arc` clones only holds as long as no other code
/// path ever drains the last strong reference). Only the `LeakedCuda` variant needs this — `Owned`
/// (CPU) sessions drop normally; the spike confirmed CPU-only sessions have none of this problem.
/// This same `ManagedSession`-shaped pattern is duplicated in `lanrurugi-inpaint` rather than
/// shared — the two crates don't share a dependency that would let it live in one place.
enum ManagedSession {
    Owned(Mutex<Session>, bool),
    LeakedCuda(&'static Mutex<Session>),
}

impl ManagedSession {
    fn with_locked<R>(&self, f: impl FnOnce(&mut Session) -> R) -> Result<R, BubbleSegmentError> {
        let mutex: &Mutex<Session> = match self {
            ManagedSession::Owned(mutex, _) => mutex,
            ManagedSession::LeakedCuda(mutex) => mutex,
        };
        let mut guard = mutex.lock().map_err(|_| {
            BubbleSegmentError::BadOutput("bubble segmentation session poisoned".into())
        })?;
        Ok(f(&mut guard))
    }
}

pub struct BubbleSegmenter {
    session: ManagedSession,
}

/// Tries to build and commit a CUDA-registered session in one atomic attempt (GPU EP integration
/// plan v8 §4.3, G1) — registration and `commit_from_file` are both inside the same `?` chain, so
/// a failure at either step (not just registration) triggers the caller's CPU fallback. `?`
/// (unlike `and_then`) auto-converts each step's own `Error<SessionBuilder>` into the plain
/// `ort::Error` (`Error<()>`) the outer `ort::Result<Session>` return type expects, via the real
/// `impl From<Error<SessionBuilder>> for Error<()>` (`error.rs:233`) — no manual `.map_err` needed
/// at each step. `.error_on_failure()` makes registration failure return a real `Err` instead of
/// silently falling back to CPU inside `ort` itself (the crate's own default behavior, `ep/mod.rs`
/// — without it, "CUDA session" and "CPU session" would be indistinguishable from the caller's
/// side).
fn try_cuda_session(
    model_path: &Path,
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
    builder.commit_from_file(model_path)
}

/// See `lanrurugi-ocr::recognize::try_openvino_session`'s identical call for the full rationale
/// (only ever reached after `ep::OpenVINO::default().is_available()` has confirmed the *loaded*
/// library actually has OpenVINO provider code compiled in — registering this EP against a build
/// that lacks it hangs indefinitely in `session.run()`, issue #103's research.md §1).
fn try_openvino_session(model_path: &Path, intra_threads: usize) -> ort::Result<Session> {
    let builder = Session::builder()?
        .with_execution_providers([ep::OpenVINO::default()
            .with_device_type("GPU")
            .build()
            .error_on_failure()])?
        .with_intra_threads(intra_threads.max(1))?;
    let mut builder = builder;
    builder.commit_from_file(model_path)
}

impl BubbleSegmenter {
    /// GPU EP integration plan v8 §4.3: try CUDA first (registration + session creation as one
    /// attempt — see `try_cuda_session`'s own doc comment for why both, not just registration),
    /// fall back to the CPU EP this crate previously always used unconditionally (research.md §1's
    /// OpenVINO hang incident is why CPU stays the unconditional final fallback, not a GPU EP of
    /// any kind, if every CUDA attempt fails).
    pub fn load(
        model_path: &Path,
        intra_threads: usize,
        cuda_memory_limit_bytes: usize,
    ) -> Result<Self, BubbleSegmentError> {
        // Named `path_display`, not `display` — `display` collides with `tracing::field::display`,
        // which the `%field` shorthand below expands to call; a same-named local shadows that
        // resolution and breaks the macro (confirmed the hard way: `cargo check` caught this as a
        // real arity-mismatch error before it ever reached a rebuild).
        let path_display = || model_path.display().to_string();

        // `LANRURUGI_DISABLE_GPU` (GPU EP integration plan v8 §4.6) — deliberately an env var
        // checked existence-only (`var_os`, not `var().is_ok()`, so a non-UTF-8 value still
        // counts as "set" rather than being silently treated as absent), not a clap flag: this is
        // meant as a hidden escape hatch, not a documented `--help` option. Checked here (still
        // library-internal, not application-layer) as the simplest option that doesn't require
        // changing this function's public signature — see `.debug-scratch/
        // GPU_EP_FINAL_SUMMARY.md` for why the disable-switch plumbing landed here rather than
        // threaded down from `TranslationRuntime`.
        let gpu_disabled = std::env::var_os("LANRURUGI_DISABLE_GPU").is_some();
        // `bool` alongside each `Session` is whether it actually ended up on CUDA —
        // `ManagedSession`/`leak_cuda_sessions` need this to know what's safe to leave owned and
        // what must eventually be leaked; `ort::Session` itself exposes no way to ask which EP it
        // ended up on after construction, so it has to be captured here, at the one point that
        // actually knows.
        let gpu_session: Option<Session> = if gpu_disabled {
            tracing::info!("LANRURUGI_DISABLE_GPU is set; using CPU for bubble segmentation");
            None
        } else {
            match ep::CUDA::default().is_available() {
                Ok(true) => {
                    match try_cuda_session(model_path, intra_threads, cuda_memory_limit_bytes) {
                        Ok(session) => {
                            tracing::info!(path = %path_display(), "bubble segmentation model loaded on CUDA");
                            Some(session)
                        }
                        Err(error) => {
                            // `.error_on_failure()` suppresses ort's own registration-failure log line
                            // (plan v8 §4.3, R3) — this is the only place that failure reason gets logged.
                            tracing::warn!(path = %path_display(), %error, "CUDA session build failed for bubble segmentation model, falling back to CPU");
                            None
                        }
                    }
                }
                Ok(false) => {
                    tracing::debug!(
                        "CUDA execution provider not compiled into the loaded ONNX Runtime build; \
                     checking OpenVINO"
                    );
                    // Only reached when CUDA wasn't compiled into the loaded build — never after a
                    // CUDA registration *failure* (that case already fell through to CPU above).
                    // See `crate::recognize::try_openvino_session`'s doc comment for why gating on
                    // `is_available()` is load-bearing here, not just an optimization.
                    match ep::OpenVINO::default().is_available() {
                        Ok(true) => match try_openvino_session(model_path, intra_threads) {
                            Ok(session) => {
                                tracing::info!(path = %path_display(), "bubble segmentation model loaded on OpenVINO");
                                Some(session)
                            }
                            Err(error) => {
                                tracing::warn!(path = %path_display(), %error, "OpenVINO session build failed for bubble segmentation model, falling back to CPU");
                                None
                            }
                        },
                        Ok(false) => {
                            tracing::debug!(
                                "OpenVINO execution provider not compiled into the loaded ONNX \
                             Runtime build; using CPU for bubble segmentation"
                            );
                            None
                        }
                        Err(error) => {
                            tracing::error!(%error, "is_available() reported an internal ONNX Runtime error while probing OpenVINO; falling back to CPU for bubble segmentation");
                            None
                        }
                    }
                }
                Err(error) => {
                    // ort's own doc on `is_available()`: "a serious internal error occurs, in which
                    // case your application should probably just abort" — logged at error level
                    // (not the benign debug! above) but still falls back to CPU rather than aborting,
                    // since the CPU path is independent of whatever made this probe itself fail.
                    tracing::error!(%error, "is_available() reported an internal ONNX Runtime error while probing CUDA; falling back to CPU for bubble segmentation");
                    None
                }
            }
        };

        let is_cuda = gpu_session.is_some();
        let session = match gpu_session {
            Some(session) => session,
            None => {
                let builder = Session::builder().map_err(|source| BubbleSegmentError::Session {
                    path: path_display(),
                    source,
                })?;
                let builder = builder
                    .with_execution_providers([ep::CPU::default().build()])
                    .map_err(|source| BubbleSegmentError::Session {
                        path: path_display(),
                        source: source.into(),
                    })?;
                let mut builder =
                    builder
                        .with_intra_threads(intra_threads.max(1))
                        .map_err(|source| BubbleSegmentError::Session {
                            path: path_display(),
                            source: source.into(),
                        })?;
                builder.commit_from_file(model_path).map_err(|source| {
                    BubbleSegmentError::Session {
                        path: path_display(),
                        source,
                    }
                })?
            }
        };
        Ok(Self {
            session: ManagedSession::Owned(Mutex::new(session), is_cuda),
        })
    }

    /// Leaks this segmenter's session's backing memory if (and only if) it's actually running on
    /// CUDA — a no-op (just returns `self` unchanged) for a CPU session. See `ManagedSession`'s
    /// own doc comment for why this exists at all, and why it's a separate, caller-invoked step
    /// rather than something [`Self::load`] itself does (this crate's own tests would otherwise
    /// leak real CUDA memory, unreclaimable until process exit, on every single `load` call).
    ///
    /// Takes/returns `self` by value rather than `&mut self` — the one real caller
    /// (`lanrurugi-api::translation_pipeline::TranslationRuntime::load`) calls this right after
    /// `load()` and before wrapping the result in an `Arc`, where it already owns the value
    /// outright; consuming it here avoids needing a placeholder value to swap into a `&mut self`
    /// field mid-match (there's no cheap valid "empty" `ManagedSession` the way `Vec::new()` is
    /// for `Inpainter`'s pool).
    #[must_use]
    pub fn leak_cuda_sessions(self) -> Self {
        let session = match self.session {
            ManagedSession::Owned(mutex, true) => {
                ManagedSession::LeakedCuda(Box::leak(Box::new(mutex)))
            }
            other => other,
        };
        Self { session }
    }

    /// Detects every speech bubble on `page`, each with its own precise (non-rectangular) mask in
    /// page-pixel coordinates.
    pub fn detect(&self, page: &RgbImage) -> Result<Vec<DetectedBubble>, BubbleSegmentError> {
        let (page_w, page_h) = page.dimensions();
        let letterbox = Letterbox::compute(page_w, page_h);
        let input_chw = letterbox.preprocess(page);

        let input_tensor = Tensor::from_array((
            vec![1i64, 3, i64::from(INPUT_SIZE), i64::from(INPUT_SIZE)],
            input_chw,
        ))?;

        // Extraction happens fully inside the closure, not after — `SessionOutputs`/its extracted
        // tensor slices borrow from the `&mut Session` `with_locked` hands in, and that borrow
        // can't outlive the closure itself, so this copies the four things actually needed (two
        // shapes + two real owned `Vec<f32>`s of the data) before the closure returns rather than
        // trying to hand back anything still borrowing the session.
        type ExtractedOutputs = (Vec<i64>, Vec<f32>, Vec<i64>, Vec<f32>);
        let (det_shape, det_flat, proto_shape, proto_flat): ExtractedOutputs = self
            .session
            .with_locked(|session| -> Result<ExtractedOutputs, BubbleSegmentError> {
                let outputs = session
                    .run(ort::inputs! {
                        "images" => input_tensor,
                    })
                    .map_err(BubbleSegmentError::from)?;
                let (det_shape, det_flat) = outputs["output0"]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| BubbleSegmentError::BadOutput(e.to_string()))?;
                let (proto_shape, proto_flat) = outputs["output1"]
                    .try_extract_tensor::<f32>()
                    .map_err(|e| BubbleSegmentError::BadOutput(e.to_string()))?;
                Ok((
                    det_shape.to_vec(),
                    det_flat.to_vec(),
                    proto_shape.to_vec(),
                    proto_flat.to_vec(),
                ))
            })??;

        if det_shape.len() != 3 || det_shape[1] as usize != 4 + 1 + NUM_MASK_COEFFS {
            return Err(BubbleSegmentError::BadOutput(format!(
                "expected output0 [1, {}, N], got {det_shape:?}",
                4 + 1 + NUM_MASK_COEFFS
            )));
        }
        if proto_shape.len() != 4 || proto_shape[1] as usize != NUM_MASK_COEFFS {
            return Err(BubbleSegmentError::BadOutput(format!(
                "expected output1 [1, {NUM_MASK_COEFFS}, H, W], got {proto_shape:?}"
            )));
        }

        let num_anchors = det_shape[2] as usize;
        let channels = det_shape[1] as usize;
        let (proto_h, proto_w) = (proto_shape[2] as usize, proto_shape[3] as usize);

        // `output0` is channel-first ([1, channels, num_anchors]): channel 0..4 = box (cx,cy,w,h)
        // in letterboxed-input pixel space, channel 4 = class confidence (already sigmoid'd by the
        // export graph), channels 5..37 = mask coefficients.
        let mut candidates: Vec<(BoundingBox, f32, [f32; NUM_MASK_COEFFS])> = Vec::new();
        for a in 0..num_anchors {
            let conf = det_flat[4 * num_anchors + a];
            if conf < CONFIDENCE_THRESHOLD {
                continue;
            }
            let cx = det_flat[a];
            let cy = det_flat[num_anchors + a];
            let w = det_flat[2 * num_anchors + a];
            let h = det_flat[3 * num_anchors + a];

            let mut coeffs = [0f32; NUM_MASK_COEFFS];
            for (i, c) in coeffs.iter_mut().enumerate() {
                *c = det_flat[(5 + i) * num_anchors + a];
            }

            let Some(bbox) = letterbox.unletterbox_box(cx, cy, w, h, page_w, page_h) else {
                continue;
            };
            candidates.push((bbox, conf, coeffs));
        }

        let kept = greedy_nms(candidates, NMS_IOU_THRESHOLD);

        let mut results = Vec::with_capacity(kept.len());
        for (bbox, confidence, coeffs) in kept {
            let mask = decode_mask(
                &coeffs,
                &proto_flat,
                proto_h,
                proto_w,
                &letterbox,
                &bbox,
                page_w,
                page_h,
            );
            results.push(DetectedBubble {
                bbox,
                confidence,
                mask,
            });
        }

        let _ = channels; // Only used for the shape-validation error message above.
        Ok(results)
    }
}

/// Maps a page image to/from the model's fixed square input via letterboxing (aspect-preserving
/// resize + centred padding) — the model was trained this way, not with a naive stretch-to-square
/// resize, so preprocessing must match or accuracy suffers on any non-square page (manga pages
/// essentially always are).
struct Letterbox {
    scale: f32,
    pad_x: f32,
    pad_y: f32,
    resized_w: u32,
    resized_h: u32,
}

impl Letterbox {
    fn compute(orig_w: u32, orig_h: u32) -> Self {
        let scale = (INPUT_SIZE as f32 / orig_w as f32).min(INPUT_SIZE as f32 / orig_h as f32);
        let resized_w = (orig_w as f32 * scale).round() as u32;
        let resized_h = (orig_h as f32 * scale).round() as u32;
        let pad_x = (INPUT_SIZE - resized_w) as f32 / 2.0;
        let pad_y = (INPUT_SIZE - resized_h) as f32 / 2.0;
        Self {
            scale,
            pad_x,
            pad_y,
            resized_w,
            resized_h,
        }
    }

    /// CHW float32 buffer, `[0, 1]`-normalised (Ultralytics' own default preprocessing — no
    /// further mean/std scaling), padded with the mid-grey `114` Ultralytics pads with by default.
    fn preprocess(&self, page: &RgbImage) -> Vec<f32> {
        let resized =
            image::imageops::resize(page, self.resized_w, self.resized_h, FilterType::CatmullRom);
        let pixel_count = (INPUT_SIZE * INPUT_SIZE) as usize;
        let mut chw = vec![114f32 / 255.0; pixel_count * 3];
        let (pad_x, pad_y) = (self.pad_x.round() as u32, self.pad_y.round() as u32);
        for (rx, ry, px) in resized.enumerate_pixels() {
            let x = pad_x + rx;
            let y = pad_y + ry;
            if x >= INPUT_SIZE || y >= INPUT_SIZE {
                continue;
            }
            let i = (y * INPUT_SIZE + x) as usize;
            for c in 0..3 {
                chw[c * pixel_count + i] = f32::from(px.0[c]) / 255.0;
            }
        }
        chw
    }

    /// Maps one detection's `(cx, cy, w, h)` (letterboxed-input pixel space) back to a
    /// `BoundingBox` in the original page's own pixel space, clamped to the page bounds. `None` if
    /// the box has no positive area left after clamping.
    fn unletterbox_box(
        &self,
        cx: f32,
        cy: f32,
        w: f32,
        h: f32,
        page_w: u32,
        page_h: u32,
    ) -> Option<BoundingBox> {
        let x0 = ((cx - w / 2.0 - self.pad_x) / self.scale).max(0.0);
        let y0 = ((cy - h / 2.0 - self.pad_y) / self.scale).max(0.0);
        let x1 = (((cx + w / 2.0 - self.pad_x) / self.scale).min(page_w as f32)).max(x0);
        let y1 = (((cy + h / 2.0 - self.pad_y) / self.scale).min(page_h as f32)).max(y0);
        let (bx, by) = (x0.round() as u32, y0.round() as u32);
        let (bw, bh) = ((x1 - x0).round() as u32, (y1 - y0).round() as u32);
        if bw == 0 || bh == 0 {
            return None;
        }
        Some(BoundingBox::new(bx, by, bw, bh))
    }

    /// Maps one page-pixel-space coordinate into the `MASK_GRID x MASK_GRID` prototype grid's own
    /// coordinate space (letterbox forward transform, then divide by `MASK_STRIDE`).
    fn to_mask_grid(&self, page_x: f32, page_y: f32) -> (f32, f32) {
        let lx = page_x * self.scale + self.pad_x;
        let ly = page_y * self.scale + self.pad_y;
        (lx / MASK_STRIDE as f32, ly / MASK_STRIDE as f32)
    }
}

/// `mask[y * bbox.w + x]` (page-pixel space, row-major over `bbox`'s own dimensions) — `true`
/// where the combined+thresholded prototype mask covers that pixel.
#[allow(clippy::too_many_arguments)]
fn decode_mask(
    coeffs: &[f32; NUM_MASK_COEFFS],
    prototypes: &[f32],
    proto_h: usize,
    proto_w: usize,
    letterbox: &Letterbox,
    bbox: &BoundingBox,
    page_w: u32,
    page_h: u32,
) -> Vec<bool> {
    let _ = (page_w, page_h, MASK_GRID); // Grid dims come from the real tensor shape, not the constant.
    let proto_pixels = proto_h * proto_w;

    // The box's own region within the mask-prototype grid — only this sub-rectangle is combined,
    // not the whole page's grid, since nothing outside the box is ever used.
    let (gx0, gy0) = letterbox.to_mask_grid(bbox.x as f32, bbox.y as f32);
    let (gx1, gy1) = letterbox.to_mask_grid(bbox.right() as f32, bbox.bottom() as f32);
    let gx0 = (gx0.floor().max(0.0) as usize).min(proto_w.saturating_sub(1));
    let gy0 = (gy0.floor().max(0.0) as usize).min(proto_h.saturating_sub(1));
    let gx1 = (gx1.ceil().max(1.0) as usize).min(proto_w);
    let gy1 = (gy1.ceil().max(1.0) as usize).min(proto_h);
    let (grid_w, grid_h) = (
        gx1.saturating_sub(gx0).max(1),
        gy1.saturating_sub(gy0).max(1),
    );

    let mut grid_mask = vec![0f32; grid_w * grid_h];
    for (gy, row) in grid_mask.chunks_mut(grid_w).enumerate() {
        let py = gy0 + gy;
        for (gx, cell) in row.iter_mut().enumerate() {
            let px = gx0 + gx;
            let mut acc = 0f32;
            for (c, &coeff) in coeffs.iter().enumerate() {
                acc += coeff * prototypes[c * proto_pixels + py * proto_w + px];
            }
            *cell = sigmoid(acc);
        }
    }

    // Upsample just this small crop to the box's own real pixel size — far cheaper than upsampling
    // the whole prototype grid to full page resolution first.
    let grid_img = image::ImageBuffer::<image::Luma<f32>, Vec<f32>>::from_raw(
        grid_w as u32,
        grid_h as u32,
        grid_mask,
    )
    .expect("grid_mask length matches grid_w * grid_h by construction");
    let resized = image::imageops::resize(&grid_img, bbox.w, bbox.h, FilterType::Triangle);

    resized.pixels().map(|p| p.0[0] >= MASK_THRESHOLD).collect()
}

fn sigmoid(x: f32) -> f32 {
    1.0 / (1.0 + (-x).exp())
}

/// Greedy NMS, keeping the highest-confidence box in each overlapping cluster — same primitive
/// (`BoundingBox::iou`) `lanrurugi_ocr::merge` already uses for a different purpose (line merging).
fn greedy_nms(
    mut candidates: Vec<(BoundingBox, f32, [f32; NUM_MASK_COEFFS])>,
    iou_threshold: f32,
) -> Vec<(BoundingBox, f32, [f32; NUM_MASK_COEFFS])> {
    candidates.sort_by(|a, b| b.1.total_cmp(&a.1));
    let mut kept: Vec<(BoundingBox, f32, [f32; NUM_MASK_COEFFS])> = Vec::new();
    'candidate: for candidate in candidates {
        for (kept_bbox, ..) in &kept {
            if kept_bbox.iou(&candidate.0) > iou_threshold {
                continue 'candidate;
            }
        }
        kept.push(candidate);
    }
    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn letterbox_centers_padding_for_a_taller_than_wide_page() {
        let lb = Letterbox::compute(540, 720);
        // Height is the limiting dimension: scale = 1600/720.
        assert!((lb.scale - 1600.0 / 720.0).abs() < 1e-4);
        assert_eq!(lb.resized_h, 1600);
        assert!(lb.resized_w < 1600);
        // Padding split evenly left/right since height already fills the frame.
        assert!(lb.pad_y.abs() < 1e-4);
        assert!(lb.pad_x > 0.0);
    }

    #[test]
    fn unletterbox_box_round_trips_a_centered_box() {
        let lb = Letterbox::compute(540, 720);
        // A box exactly covering the resized image (no crop) should map back to the full page.
        let full_w = lb.resized_w as f32;
        let full_h = lb.resized_h as f32;
        let bbox = lb
            .unletterbox_box(
                lb.pad_x + full_w / 2.0,
                lb.pad_y + full_h / 2.0,
                full_w,
                full_h,
                540,
                720,
            )
            .unwrap();
        assert_eq!(bbox.x, 0);
        assert_eq!(bbox.y, 0);
        assert!(bbox.w >= 538 && bbox.w <= 540);
        assert!(bbox.h >= 718 && bbox.h <= 720);
    }

    #[test]
    fn nms_drops_the_lower_confidence_overlapping_box() {
        let a = (
            BoundingBox::new(10, 10, 100, 100),
            0.9,
            [0f32; NUM_MASK_COEFFS],
        );
        let b = (
            BoundingBox::new(15, 15, 100, 100),
            0.5,
            [0f32; NUM_MASK_COEFFS],
        );
        let c = (
            BoundingBox::new(500, 500, 50, 50),
            0.8,
            [0f32; NUM_MASK_COEFFS],
        );
        let kept = greedy_nms(vec![a, b, c], 0.45);
        assert_eq!(
            kept.len(),
            2,
            "the overlapping lower-confidence box must be dropped"
        );
        assert!(kept.iter().any(|(bbox, ..)| bbox.x == 10));
        assert!(kept.iter().any(|(bbox, ..)| bbox.x == 500));
    }

    #[test]
    fn sigmoid_is_bounded_and_monotonic() {
        assert!(sigmoid(0.0) - 0.5 < 1e-6);
        assert!(sigmoid(10.0) > sigmoid(0.0));
        assert!(sigmoid(-10.0) < sigmoid(0.0));
        assert!(sigmoid(100.0) <= 1.0);
        assert!(sigmoid(-100.0) >= 0.0);
    }
}
