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
/// `tokio_util`'s own default is 8MiB, which one real page can exceed by itself.
///
/// 64MiB was chosen against the assumption that the page crossing this boundary had already been
/// reduced to `Inpainter`'s own working resolution (1024 on its longer edge). That assumption did
/// not hold: the reduction ran *inside* the worker, after the transfer, so a page above roughly
/// 13.4 megapixels produced an over-limit request that `tokio_util` rejected on the **encode** side
/// before sending a byte — and the erase then failed in a way indistinguishable from a model
/// failure. Feature `012-large-page-translation` makes the client reduce first, for exactly those
/// pages; see [`erase_request_provably_fits`]. This limit itself is deliberately unchanged, and is
/// not a measured tight bound.
pub const MAX_FRAME_BYTES: usize = 64 * 1024 * 1024;

/// Fixed per-request bytes a bubble-segmentation request adds: one wire struct (dimensions plus a
/// `Vec` length prefix).
const BUBBLE_REQUEST_OVERHEAD_BYTES: u64 = 16;

/// Bytes a bubble-segmentation request occupies on the wire: the page at 3 bytes/pixel plus the
/// same envelope slack [`erase_request_bytes`] allows.
///
/// This matters because the model itself only ever sees a 1600x1600 letterbox, yet the request
/// sends the page at full resolution. A page over [`MAX_FRAME_BYTES`] therefore cannot be
/// transferred at all — and sending it anyway costs far more than the failed call: the client
/// discards the worker session, so the next page pays a full CUDA model reload for all three
/// models (measured on a 4535x6307 scan, 2026-10-31).
pub fn bubble_request_bytes(width: u32, height: u32) -> u64 {
    BUBBLE_REQUEST_OVERHEAD_BYTES
        + u64::from(width) * u64::from(height) * 3
        + ERASE_REQUEST_FRAMING_SLACK
}

/// Whether a bubble-segmentation request for a `width` x `height` page can be sent at all.
pub fn bubble_request_fits(width: u32, height: u32) -> bool {
    bubble_request_bytes(width, height) <= MAX_FRAME_BYTES as u64
}

/// Fixed per-request bytes an `erase_page` request adds on top of its arguments' own payloads: three
/// wire structs (page, model mask, paste mask), each `4 + 4` bytes of dimensions plus an 8-byte
/// `Vec` length prefix — `3 * 16 = 48`. The RPC envelope is accounted for separately, by
/// [`ERASE_REQUEST_FRAMING_SLACK`].
const ERASE_REQUEST_OVERHEAD_BYTES: u64 = 48;

/// Envelope allowance, kept separate from the exact argument arithmetic above so the two are not
/// confused: the `48 + 5*w*h` term is pinned against real serialization by a test, this term is a
/// deliberate over-estimate. Over-estimating costs a page a trip through the reduced path, which
/// still produces a correct result; under-estimating costs the silent failure this arithmetic
/// exists to prevent. Those costs are not symmetric, so this errs high.
pub const ERASE_REQUEST_FRAMING_SLACK: u64 = 1024;

// The band this slack can misclassify is a fraction of a pixel row at 5 bytes per pixel, not a
// second undocumented threshold. Asserted at compile time so it cannot drift silently.
const _: () = assert!(ERASE_REQUEST_FRAMING_SLACK / 5 < 256);

/// Total bytes an `erase_page` request for a `width` x `height` page occupies on the wire: the page
/// at 3 bytes/pixel plus a model mask and a paste mask at 1 byte/pixel each
/// (`bincode`'s default configuration is fixed-width, so this part is exact), plus framing.
pub fn erase_request_bytes(width: u32, height: u32) -> u64 {
    let pixels = u64::from(width) * u64::from(height);
    ERASE_REQUEST_OVERHEAD_BYTES + pixels * 5 + ERASE_REQUEST_FRAMING_SLACK
}

/// Whether an `erase_page` request for a `width` x `height` page can be sent at its full resolution
/// without exceeding [`MAX_FRAME_BYTES`].
///
/// Deliberately conservative in one direction only — see [`erase_request_bytes`]. A page for which
/// this returns `false` must be reduced to `lanrurugi_inpaint::work_dims` before being sent, so that
/// the client and the worker agree on the reduced dimensions rather than each deriving them.
pub fn erase_request_provably_fits(width: u32, height: u32) -> bool {
    erase_request_bytes(width, height) <= MAX_FRAME_BYTES as u64
}

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
    /// Transcribes a page's crops in one call, running manga-ocr's encoder once over the whole
    /// batch (`TextRecognizer::recognize_batch`). Results are in input order; one crop's error
    /// does not fail its siblings, so the caller must treat the returned vector as positional.
    async fn recognize_batch(
        crops: Vec<RawRgbImage>,
    ) -> Result<Vec<Result<String, WorkerError>>, WorkerError>;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The real page that exposed this (2026-10-31): a 4535x6307 scan is 81.8 MiB of RGB, so a
    /// bubble-segmentation request for it can never encode — the client must skip it rather than
    /// send it and lose the worker session.
    #[test]
    fn a_very_large_page_does_not_fit_a_bubble_request() {
        assert!(!bubble_request_fits(4535, 6307));
        assert!(!bubble_request_fits(4000, 6000));
        // A normal digital page still goes through untouched.
        assert!(bubble_request_fits(1600, 2300));
    }

    /// The exact byte length of the three values an `erase_page` request carries as arguments,
    /// serialized with `bincode`'s default (fixed-width) configuration — the same library and
    /// configuration `tarpc`'s bincode transport uses. A tuple, not a struct, because that is what
    /// the argument list serializes as: three values back to back with no extra framing.
    fn serialize_arguments(width: u32, height: u32) -> u64 {
        let pixels = (width as usize) * (height as usize);
        let page = RawRgbImage {
            width,
            height,
            rgb: vec![0u8; pixels * 3],
        };
        let mask = RawMask {
            width,
            height,
            mask: vec![false; pixels],
        };
        bincode::serialize(&(page, mask.clone(), mask))
            .expect("serializing erase_page's arguments")
            .len() as u64
    }

    #[test]
    fn erase_request_bytes_matches_real_serialization() {
        // Small sizes only: this asserts the *formula*, and serializing a real 60-megapixel page
        // here would allocate hundreds of megabytes to prove the same arithmetic.
        for (w, h) in [(1u32, 1u32), (3, 5), (17, 23), (1024, 1536)] {
            let arguments = serialize_arguments(w, h);
            assert_eq!(
                arguments,
                ERASE_REQUEST_OVERHEAD_BYTES + u64::from(w) * u64::from(h) * 5,
                "the 48 + 5*w*h term no longer matches bincode's real framing for {w}x{h}"
            );
            assert_eq!(
                erase_request_bytes(w, h),
                arguments + ERASE_REQUEST_FRAMING_SLACK,
                "erase_request_bytes({w}, {h}) drifted from the wire format"
            );
        }
    }

    #[test]
    fn request_size_scales_five_bytes_per_pixel() {
        // Page (3 bytes/pixel) plus two masks (1 each). If a future change adds a field to either
        // wire type, this is what notices.
        let base = erase_request_bytes(100, 100);
        let bigger = erase_request_bytes(200, 100);
        assert_eq!(bigger - base, 100 * 100 * 5);
    }

    #[test]
    fn surveyed_large_pages_do_not_fit_at_full_resolution() {
        // The dimensions this feature was written for: real archives whose erase request exceeded
        // the boundary limit and were therefore never translated at all.
        for (w, h) in [
            (6336u32, 9504u32),
            (5400, 8100),
            (6084, 3943),
            (4535, 6307),
            (4000, 6000),
        ] {
            assert!(
                !erase_request_provably_fits(w, h),
                "{w}x{h} ({:.1} MiB) must be routed through the reduced path",
                erase_request_bytes(w, h) as f64 / (1024.0 * 1024.0)
            );
        }
    }

    #[test]
    fn the_predicate_errs_toward_the_reduced_path() {
        // A page whose *arguments alone* would fit, but only within the framing slack, must still be
        // reported as not provably fitting — the envelope's exact size is not knowable without
        // serializing. Getting this backwards means the client attempts a transfer the codec then
        // rejects on the encode side, which is precisely the silent failure this feature removes.
        //
        // 3800 x 3532: arguments are 48 + 5*13_421_600 = 67_108_048 <= 67_108_864, so without the
        // slack this page would be sent whole; with it, 67_109_064 > 67_108_864, so it is reduced.
        let (w, h) = (3800u32, 3532u32);
        assert!(
            ERASE_REQUEST_OVERHEAD_BYTES + u64::from(w) * u64::from(h) * 5
                <= MAX_FRAME_BYTES as u64,
            "this test's premise: the arguments alone fit"
        );
        assert!(
            !erase_request_provably_fits(w, h),
            "the framing slack must push {w}x{h} onto the reduced path"
        );

        // ...and the next size down is safely inside the fitting range, so the conservative
        // direction costs at most that band and never reclassifies a comfortably-fitting page.
        // (The band's own bound is a compile-time assertion next to `ERASE_REQUEST_FRAMING_SLACK`.)
        assert!(erase_request_provably_fits(3600, 3300));
    }

    #[test]
    fn framing_slack_is_the_only_inexact_term() {
        // Guards the documentation's own claim: exactly one term of erase_request_bytes is an
        // over-estimate, and it does not scale with the page.
        let small = erase_request_bytes(10, 10) - (ERASE_REQUEST_OVERHEAD_BYTES + 10 * 10 * 5);
        let large =
            erase_request_bytes(1000, 1000) - (ERASE_REQUEST_OVERHEAD_BYTES + 1000 * 1000 * 5);
        assert_eq!(small, ERASE_REQUEST_FRAMING_SLACK);
        assert_eq!(large, ERASE_REQUEST_FRAMING_SLACK);
    }
}
