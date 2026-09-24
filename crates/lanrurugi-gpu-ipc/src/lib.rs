//! Wire protocol between `lanrurugi-api` (the long-lived web server, never touches CUDA) and
//! `lanrurugi-gpu-worker` (a short-lived subprocess that owns every CUDA session — recognizer,
//! inpainter, bubble segmenter). See `lanrurugi-api::gpu_worker_client`'s own module doc for why
//! this subprocess boundary exists at all (GPU EP integration plan v9, issue #103): a real NVIDIA
//! driver bug in `libnvidia-gpucomp.so` makes dropping a CUDA session ~35-60% likely to crash the
//! process holding it, and the only way found so far to *fully* reclaim that VRAM is killing the
//! process outright with `SIGKILL` (verified safe, n=20, zero crashes/driver corruption) rather
//! than ever calling `Drop` on the session in-process — which must not be the main server process,
//! since killing that would take down every other unrelated feature with it.
//!
//! Built on `tarpc` (Unix-socket transport, `bincode`-framed) rather than a hand-rolled
//! length-prefixed protocol — an earlier hand-written version of this crate was reviewed and
//! found to have three real gaps a from-scratch implementation would have had to independently
//! re-solve: no request timeout (a hung worker would block the caller forever), no way to
//! distinguish "this connection is still clean" from "a request was cancelled/failed mid-flight
//! and this connection's byte stream may be desynced" before returning it to a pool (a subtler
//! version of a real jellyfin-suite/frame-forge incident — a request-id field that most call
//! sites wrote but never actually verified on the read side, letting a stale response leak into
//! an unrelated new request), and no peer identity check. `tarpc`'s `Context` carries a deadline
//! per call, and its multiplexed transport gives every in-flight call its own tracked completion
//! state internally, so a cancelled/timed-out call can't leave the underlying connection in an
//! ambiguous state the way hand-rolled synchronous request/response framing can.

use serde::{Deserialize, Serialize};

/// Default Unix socket path the worker listens on and the client connects to. Overridable via
/// `LANRURUGI_GPU_WORKER_SOCKET` for tests/multiple instances — see `gpu_worker_client`'s own
/// docs for why a fixed default is fine for production (one worker per server process).
pub const DEFAULT_SOCKET_PATH: &str = "/tmp/lanrurugi-gpu-worker.sock";

/// Both client and server sides must set this on their transport's length-delimited codec —
/// `tokio_util`'s own default is 8MiB, too small for a real page (a 2048px `MAX_INPAINT_SIZE` RGB
/// page alone is ~12.6MB raw, before `erase_page`'s request also carries two same-sized masks
/// alongside it). 64MiB is generous headroom over that, not a measured tight bound.
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// A row-major RGB image, dimensions carried alongside the raw bytes rather than assumed —
/// serializing an `image::RgbImage` directly would pull the `image` crate's own (de)serialization
/// support (not enabled by default) into this protocol crate for no real benefit over three plain
/// fields.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawRgbImage {
    pub width: u32,
    pub height: u32,
    /// Length must be exactly `width * height * 3`.
    pub rgb: Vec<u8>,
}

/// A row-major single-channel mask, `true` = the pixel is covered.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RawMask {
    pub width: u32,
    pub height: u32,
    pub mask: Vec<bool>,
}

/// Wire form of `lanrurugi_ocr::bubble_segment::DetectedBubble` — duplicated rather than shared
/// (this crate deliberately depends on neither `lanrurugi-ocr` nor `lanrurugi-inpaint`, so the
/// worker/client crates each convert to/from their own real domain types at the boundary; see
/// `lanrurugi-gpu-worker`'s and `lanrurugi-api::gpu_worker_client`'s own conversion code).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DetectedBubbleWire {
    pub bbox_x: u32,
    pub bbox_y: u32,
    pub bbox_w: u32,
    pub bbox_h: u32,
    pub confidence: f32,
    pub mask: Vec<bool>,
}

/// Whether a worker-side failure means "this request didn't work, but the session/process is
/// still fine to keep using" ([`Self::Config`]) or "the CUDA session/process itself may be in a
/// broken state and must not be trusted with another call" ([`Self::Fatal`]).
///
/// Added after a real incident (2026-09-13, issue #103): every worker error used to come back as
/// a bare `String`, so `lanrurugi-api::gpu_worker_client`'s `call()` had no way to distinguish a
/// benign "no inpainting model loaded" from a `CUBLAS failure 3: the resource allocation failed`/
/// `CUDNN_STATUS_INTERNAL_ERROR` CUDA-OOM error — both looked identical to the client, so the
/// worker kept getting reused *after* its CUDA session was already unusable, and requests piling
/// up against that broken session eventually made the whole `lanrurugi-server` process stop
/// responding to anything, `/health` included, for minutes at a time until someone manually
/// `kill -9`'d the worker.
///
/// The classification lives on the **worker** side (`lanrurugi-gpu-worker`), not the client —
/// only the worker actually holds the real `ort::Error`/CUDA error text at the point of failure;
/// the client would otherwise have to pattern-match on error strings, which is exactly the kind
/// of fragile coupling this type exists to avoid. See `lanrurugi-gpu-worker/src/main.rs`'s own
/// classification logic for which raw error substrings map to which variant.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkerError {
    /// A request/configuration problem — the model genuinely isn't loaded, a mask's dimensions
    /// don't match the page, a crop was empty, etc. The worker process and its CUDA sessions are
    /// still healthy; the client keeps reusing this session for the next call.
    Config(String),
    /// A GPU/driver/resource-level failure (CUDA out-of-memory, a `CUBLAS`/`CUDNN` internal
    /// error, or literally anything not confidently recognized as [`Self::Config`] — unknown
    /// errors default to `Fatal` deliberately, since the cost of an unnecessary worker restart
    /// (a few seconds) is far smaller than the cost of continuing to reuse a session that's
    /// actually broken, per this incident). The worker calls `std::process::exit` right after
    /// sending this response, so the client's next call finds the connection gone and spawns a
    /// fresh worker rather than needing to inspect this variant itself.
    Fatal(String),
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerError::Config(msg) | WorkerError::Fatal(msg) => write!(f, "{msg}"),
        }
    }
}

/// The RPC surface the worker exposes. Every method returns `Result<_, WorkerError>` — the real
/// error types (`RecognitionError`, `InpaintError`, `BubbleSegmentError`) live in
/// `lanrurugi-ocr`/`lanrurugi-inpaint`, which this crate deliberately does not depend on (see
/// `DetectedBubbleWire`'s own doc comment for the same reasoning); the client side wraps the
/// string payload back into its own error type at the boundary.
#[tarpc::service]
pub trait GpuWorker {
    /// Transcribes one already-cropped text region crop (`lanrurugi_ocr::recognize::TextRecognizer::recognize`).
    async fn recognize(crop: RawRgbImage) -> Result<String, WorkerError>;
    /// Erases every masked pixel across a whole page (`lanrurugi_inpaint::Inpainter::erase_page`).
    async fn erase_page(
        page: RawRgbImage,
        model_mask: RawMask,
        paste_mask: RawMask,
    ) -> Result<RawRgbImage, WorkerError>;
    /// Detects speech bubbles on a page (`lanrurugi_ocr::bubble_segment::BubbleSegmenter::detect`).
    async fn segment_bubbles(page: RawRgbImage) -> Result<Vec<DetectedBubbleWire>, WorkerError>;
    /// Cheap liveness probe — used by the client's health check, does not touch any model.
    async fn ping() -> ();
}
