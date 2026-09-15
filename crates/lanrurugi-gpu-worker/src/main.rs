//! GPU inference worker — a short-lived subprocess spawned by `lanrurugi-api::gpu_worker_client`
//! (never by a user directly) that owns every CUDA session in the system: `TextRecognizer`,
//! `Inpainter`, `BubbleSegmenter`. See `lanrurugi-gpu-ipc`'s own top-level doc comment for why
//! this subprocess boundary exists at all.
//!
//! This process is expected to be `SIGKILL`'d by its parent after an idle period — never asked to
//! shut down gracefully — so nothing here needs a clean-shutdown path for the CUDA sessions
//! themselves; letting them leak (never `Box::leak`-ing manually here, since the whole process
//! disappearing at once is what actually reclaims the memory) is the point, not an oversight. See
//! `.debug-scratch/GPU_EP_FINAL_SUMMARY.md` for why a graceful `Drop` of any of these sessions is
//! the operation to avoid, and why `SIGKILL` (verified safe, n=20) is the one safe way to reclaim
//! their VRAM.

mod vram_budget;

use std::future::Future;
use std::sync::Arc;

use futures::{future, StreamExt};
use image::RgbImage;
use lanrurugi_gpu_ipc::{DetectedBubbleWire, GpuWorker, RawMask, RawRgbImage, WorkerError};
use lanrurugi_inpaint::Inpainter;
use lanrurugi_ocr::bubble_segment::BubbleSegmenter;
use lanrurugi_ocr::recognize::TextRecognizer;
use tarpc::context::Context;
use tarpc::server::{self, Channel};
use tarpc::tokio_serde::formats::Bincode;

/// Classifies a raw inference error string into [`WorkerError::Config`] (request/setup problem,
/// session still healthy) or [`WorkerError::Fatal`] (GPU/driver-level, session must be assumed
/// broken) — see [`WorkerError`]'s own doc comment for why this lives here rather than on the
/// client. Substrings below are the *exact* error text this project has actually observed live
/// (2026-09-13, issue #103) for CUDA resource-exhaustion failures — `CUBLAS failure`/
/// `CUDNN_STATUS_*`/`bfc_arena.cc`/`Available memory of` all came from real `ort::Error` messages
/// during a real display of this incident, not guessed. Anything not recognized as one of this
/// module's own small set of request-validation errors (mask-length mismatch, missing model,
/// malformed `RawRgbImage`) defaults to `Fatal` deliberately — the cost of an unnecessary worker
/// restart (a few seconds of cold start) is far smaller than the cost of continuing to reuse a
/// session that's actually broken, which is what caused `lanrurugi-server` to stop responding to
/// anything (including unrelated `/health` checks) for minutes at a time in that incident.
fn classify_error(message: &str) -> WorkerError {
    const CONFIG_MARKERS: &[&str] = &[
        "no inpainting model loaded",
        "no bubble segmentation model loaded",
        "mask length",
        "did not match expected",
        "claimed",
        "byte length didn't match",
    ];
    if CONFIG_MARKERS.iter().any(|m| message.contains(m)) {
        WorkerError::Config(message.to_string())
    } else {
        WorkerError::Fatal(message.to_string())
    }
}

/// Ends this process immediately after a [`WorkerError::Fatal`] has already been queued for
/// delivery back to the client. Deliberately blunt (`std::process::exit`, not a graceful
/// shutdown) — this process's own leaked CUDA sessions (see this module's top-level doc comment)
/// can't be trusted after a driver-level error, and the whole point of `SIGKILL`-only recycling is
/// that nothing here ever tries to clean them up in-process. The parent (`GpuWorkerClient`)
/// detects the closed connection on its next call and spawns a fresh worker.
///
/// Spawned as a short delay rather than called synchronously in the request handler so the
/// in-flight tarpc response actually reaches the client before the process disappears — exiting
/// immediately risks the connection closing before the `Fatal` payload is flushed, which would
/// surface to the client as an opaque RPC/transport error instead of the classified `WorkerError`
/// it's supposed to see.
fn exit_after_fatal_response() {
    tokio::spawn(async {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        tracing::error!("exiting after a Fatal worker error — see WorkerError's own doc comment");
        std::process::exit(1);
    });
}

/// Same CPU-thread budget knob `lanrurugi-api::translation_pipeline` passes to each model's own
/// `load()` today — the worker takes over exactly the loading this process used to do inline, so
/// it should behave identically resource-wise, not more or less generous.
const INTRA_THREADS: usize = 1;

/// Which model group this worker process should load — set via `LANRURUGI_GPU_WORKER_MODELS`,
/// always passed explicitly by `gpu_worker_client` (never left to its own default outside tests).
///
/// Splits the three models this worker can hold across two separate processes rather than one
/// (real incident, 2026-09-13, issue #103): `TextRecognizer` (384MiB), `BubbleSegmenter` (768MiB),
/// and `Inpainter` (4GiB) all resident in the same CUDA context at once left an 8GB card with
/// essentially nothing free — the very next inference after all three finished loading failed with
/// "Available memory of 0", killing the worker, which respawned and hit the exact same wall loading
/// the same three models again, a crash loop that silently dropped most of a page's OCR regions
/// (each one skipped rather than failing the whole batch — see `lanrurugi_ocr::batch::run_batch`'s
/// own doc comment). `Recognize` (this worker only loads `TextRecognizer`, ~384MiB, cheap enough to
/// stay resident the normal `IDLE_TIMEOUT`) and `Image` (`BubbleSegmenter` + `Inpainter` together,
/// ~4.75GiB, reclaimed on a much shorter idle timeout — see `gpu_worker_client`'s own two-client
/// wiring) never share a CUDA context, so their combined peak (whichever one is actually resident
/// at a given moment) never approaches the old three-model total.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ModelGroup {
    /// `TextRecognizer` only.
    Recognize,
    /// `Inpainter` + `BubbleSegmenter` — always called back-to-back for the same page in
    /// `translation_pipeline::composite_and_cache`, so there's no benefit to splitting them
    /// further into their own processes; both or neither is the natural unit.
    Image,
}

impl ModelGroup {
    const ENV_VAR: &'static str = "LANRURUGI_GPU_WORKER_MODELS";

    fn from_env() -> Result<Self, String> {
        match std::env::var(Self::ENV_VAR).as_deref() {
            Ok("recognize") => Ok(Self::Recognize),
            Ok("image") => Ok(Self::Image),
            Ok(other) => Err(format!(
                "{} must be \"recognize\" or \"image\", got {other:?}",
                Self::ENV_VAR
            )),
            Err(_) => Err(format!("{} is required but was not set", Self::ENV_VAR)),
        }
    }
}

#[derive(Clone)]
struct Worker {
    /// `None` in an `Image`-group worker (see [`ModelGroup`]) — never attempted, not
    /// loaded-then-discarded.
    recognizer: Option<Arc<TextRecognizer>>,
    inpainter: Option<Arc<Inpainter>>,
    bubble_segmenter: Option<Arc<BubbleSegmenter>>,
}

fn raw_to_rgb_image(raw: RawRgbImage) -> Result<RgbImage, String> {
    RgbImage::from_raw(raw.width, raw.height, raw.rgb).ok_or_else(|| {
        format!(
            "RawRgbImage claimed {}x{} but byte length didn't match",
            raw.width, raw.height
        )
    })
}

fn rgb_image_to_raw(img: &RgbImage) -> RawRgbImage {
    RawRgbImage {
        width: img.width(),
        height: img.height(),
        rgb: img.as_raw().clone(),
    }
}

fn raw_mask_to_vec(raw: RawMask, expected_len: usize) -> Result<Vec<bool>, String> {
    if raw.mask.len() != expected_len {
        return Err(format!(
            "mask length {} did not match expected {expected_len} ({}x{})",
            raw.mask.len(),
            raw.width,
            raw.height
        ));
    }
    Ok(raw.mask)
}

impl GpuWorker for Worker {
    // Each `ort::Session::run` below blocks the calling thread for the full inference duration.
    // Calling it directly in an `async fn` would starve this runtime's reactor — confirmed live:
    // a real `erase_page` call never yielded the thread back, so every other in-flight request
    // (`ping` included) went silent until the client's 60s deadline killed the connection.
    // `spawn_blocking` moves it to tokio's separate blocking-task pool instead.
    async fn recognize(self, _: Context, crop: RawRgbImage) -> Result<String, WorkerError> {
        let Some(recognizer) = self.recognizer.clone() else {
            return Err(WorkerError::Config(
                "no recognition model loaded in this worker".to_string(),
            ));
        };
        let crop = raw_to_rgb_image(crop).map_err(WorkerError::Config)?;
        let result = tokio::task::spawn_blocking(move || {
            recognizer.recognize(&crop).map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| WorkerError::Fatal(format!("recognize task panicked: {e}")))?;
        result.map_err(|e| {
            let classified = classify_error(&e);
            if matches!(classified, WorkerError::Fatal(_)) {
                exit_after_fatal_response();
            }
            classified
        })
    }

    async fn erase_page(
        self,
        _: Context,
        page: RawRgbImage,
        model_mask: RawMask,
        paste_mask: RawMask,
    ) -> Result<RawRgbImage, WorkerError> {
        let Some(inpainter) = self.inpainter.clone() else {
            return Err(WorkerError::Config(
                "no inpainting model loaded in this worker".to_string(),
            ));
        };
        let expected_len = (page.width * page.height) as usize;
        let page_img = raw_to_rgb_image(page).map_err(WorkerError::Config)?;
        let model_mask = raw_mask_to_vec(model_mask, expected_len).map_err(WorkerError::Config)?;
        let paste_mask = raw_mask_to_vec(paste_mask, expected_len).map_err(WorkerError::Config)?;
        let result = tokio::task::spawn_blocking(move || {
            inpainter
                .erase_page(&page_img, &model_mask, &paste_mask)
                .map(|out| rgb_image_to_raw(&out))
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| WorkerError::Fatal(format!("erase_page task panicked: {e}")))?;
        result.map_err(|e| {
            let classified = classify_error(&e);
            if matches!(classified, WorkerError::Fatal(_)) {
                exit_after_fatal_response();
            }
            classified
        })
    }

    async fn segment_bubbles(
        self,
        _: Context,
        page: RawRgbImage,
    ) -> Result<Vec<DetectedBubbleWire>, WorkerError> {
        let Some(bubble_segmenter) = self.bubble_segmenter.clone() else {
            return Err(WorkerError::Config(
                "no bubble segmentation model loaded in this worker".to_string(),
            ));
        };
        let page_img = raw_to_rgb_image(page).map_err(WorkerError::Config)?;
        let result = tokio::task::spawn_blocking(move || {
            bubble_segmenter
                .detect(&page_img)
                .map(|bubbles| {
                    bubbles
                        .into_iter()
                        .map(|b| DetectedBubbleWire {
                            bbox_x: b.bbox.x,
                            bbox_y: b.bbox.y,
                            bbox_w: b.bbox.w,
                            bbox_h: b.bbox.h,
                            confidence: b.confidence,
                            mask: b.mask,
                        })
                        .collect()
                })
                .map_err(|e| e.to_string())
        })
        .await
        .map_err(|e| WorkerError::Fatal(format!("segment_bubbles task panicked: {e}")))?;
        result.map_err(|e| {
            let classified = classify_error(&e);
            if matches!(classified, WorkerError::Fatal(_)) {
                exit_after_fatal_response();
            }
            classified
        })
    }

    async fn ping(self, _: Context) {}
}

/// Loads only the model(s) `group` calls for — see [`ModelGroup`]'s own doc comment for why a
/// worker process never loads all three. `Recognize` requires its one model (the worker can't do
/// anything useful without it); `Image`'s inpainter/bubble segmenter are each optional, mirroring
/// exactly the discovery + graceful-degradation shape
/// `lanrurugi-api::translation_pipeline::TranslationRuntime::load` used before this subprocess
/// boundary existed (a missing inpaint/bubble-seg model directory is not fatal — see each crate's
/// own `model_discovery` module doc). A `Recognize` worker's `inpainter`/`bubble_segmenter` fields
/// are always `None` (never even attempted) rather than loaded-then-discarded — `RawRgbImage`'s own
/// `Worker` fields make `erase_page`/`segment_bubbles` return `WorkerError::Config` for that case
/// regardless of *why* the field is `None`, so a `Recognize` worker correctly refuses those calls
/// the exact same way a worker that legitimately has no inpainting model installed would.
fn load_worker(group: ModelGroup) -> Result<Worker, String> {
    // Resolved once, before any session build — see `vram_budget`'s own module doc for why this
    // has to happen up front (the byte count is an argument to `with_memory_limit`, not something
    // discoverable after the fact) and why hardware-proportional percent/min/max replaces the
    // fixed byte constants each of these crates used to hardcode independently.
    let total_vram_bytes = vram_budget::detect_total_vram_bytes();
    tracing::info!(
        total_vram_mib = total_vram_bytes.map(|b| b / (1024 * 1024)),
        "resolved GPU VRAM budget inputs"
    );

    let recognizer = if group == ModelGroup::Recognize {
        let paths =
            lanrurugi_ocr::model_discovery::ModelPaths::discover().map_err(|e| e.to_string())?;
        let budget = vram_budget::RECOGNIZE_WORKER_BUDGET.resolve(total_vram_bytes);
        Some(Arc::new(
            TextRecognizer::load(&paths, INTRA_THREADS, budget)
                .map_err(|e| e.to_string())?
                .leak_cuda_sessions(),
        ))
    } else {
        None
    };

    // Both models below share one Image-worker-wide budget, split by a fixed ratio — see
    // `vram_budget::split_image_worker_budget`'s own doc comment for why (they're always loaded
    // into, and reclaimed from, the same process as a unit).
    let (bubble_segmenter_budget, inpainter_budget) = vram_budget::split_image_worker_budget(
        vram_budget::IMAGE_WORKER_BUDGET.resolve(total_vram_bytes),
    );

    let inpainter = if group == ModelGroup::Image {
        match lanrurugi_inpaint::model_discovery::find_model_dir() {
            Ok(dir) => {
                let model_path = lanrurugi_inpaint::model_discovery::model_path(&dir);
                match Inpainter::load(&model_path, INTRA_THREADS, inpainter_budget) {
                    Ok(inpainter) => Some(Arc::new(inpainter.leak_cuda_sessions())),
                    Err(e) => {
                        tracing::warn!(error = %e, "found an inpainting model directory but failed to load it");
                        None
                    }
                }
            }
            Err(_) => {
                tracing::info!("no inpainting model installed");
                None
            }
        }
    } else {
        None
    };

    let bubble_segmenter = if group == ModelGroup::Image {
        match lanrurugi_ocr::bubble_segment::model_discovery::find_model_dir() {
            Ok(dir) => {
                let model_path = lanrurugi_ocr::bubble_segment::model_discovery::model_path(&dir);
                match BubbleSegmenter::load(&model_path, INTRA_THREADS, bubble_segmenter_budget) {
                    Ok(seg) => Some(Arc::new(seg.leak_cuda_sessions())),
                    Err(e) => {
                        tracing::warn!(error = %e, "found a bubble segmentation model directory but failed to load it");
                        None
                    }
                }
            }
            Err(_) => {
                tracing::info!("no bubble segmentation model installed");
                None
            }
        }
    } else {
        None
    };

    if group == ModelGroup::Recognize && recognizer.is_none() {
        return Err("Recognize worker failed to load its recognizer".to_string());
    }

    Ok(Worker {
        recognizer,
        inpainter,
        bubble_segmenter,
    })
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // Same default-to-`info` shape `lanrurugi-server`'s own `telemetry.rs` uses — plain
    // `tracing_subscriber::fmt::init()` defaults to `ERROR`-only with no `RUST_LOG` set, which
    // would silently swallow this process's own model-loading progress/timing logs (the exact
    // information needed to diagnose a slow cold start) whenever `RUST_LOG` isn't set, which
    // production/dev containers alike normally don't set explicitly.
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let socket_path = std::env::var("LANRURUGI_GPU_WORKER_SOCKET")
        .unwrap_or_else(|_| lanrurugi_gpu_ipc::DEFAULT_SOCKET_PATH.to_string());
    // A stale socket file from a previous worker that was SIGKILL'd (not gracefully shut down, so
    // it never had a chance to remove its own socket file) would otherwise make `bind` fail with
    // "address already in use" — removing it first is safe precisely because this process is only
    // ever started by `gpu_worker_client` after it has already confirmed no live worker is
    // listening there (see that module's own spawn logic).
    let _ = std::fs::remove_file(&socket_path);

    let group = ModelGroup::from_env().map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    tracing::info!(path = %socket_path, ?group, "loading models");
    let worker = load_worker(group).map_err(|e| -> Box<dyn std::error::Error> { e.into() })?;
    tracing::info!("models loaded, listening");

    let mut listener = tarpc::serde_transport::unix::listen(&socket_path, Bincode::default).await?;
    // Default is 8MiB (`tokio_util::codec::length_delimited`'s own `Builder::default`) — too
    // small for a real page: a 2048px `MAX_INPAINT_SIZE` RGB page alone is already
    // 2048*2048*3 = ~12.6MB raw, before `erase_page`'s request also carries two masks of the same
    // pixel count alongside it. See `lanrurugi_gpu_ipc::MAX_FRAME_BYTES` (the client side sets the
    // same limit on its own connections) for why this value, not an unbounded one.
    listener
        .config_mut()
        .max_frame_length(lanrurugi_gpu_ipc::MAX_FRAME_BYTES);
    listener
        .filter_map(|r| future::ready(r.ok()))
        .map(server::BaseChannel::with_defaults)
        .map(|channel| {
            let worker = worker.clone();
            channel.execute(worker.serve()).for_each(spawn)
        })
        // Serve every incoming connection concurrently, same shape as tarpc's own examples —
        // this worker only ever expects one client (the parent server process) but a fresh
        // connection per pooled client-side connection is still normal, not an error case.
        .buffer_unordered(16)
        .for_each(|()| async {})
        .await;

    Ok(())
}

async fn spawn(fut: impl Future<Output = ()> + Send + 'static) {
    tokio::spawn(fut);
}
