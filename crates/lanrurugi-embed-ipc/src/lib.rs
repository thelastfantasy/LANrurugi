//! Wire protocol between `lanrurugi-api` (the long-lived web server) and `lanrurugi-embed-worker`
//! (a subprocess that owns the recommendation embedding model's ONNX Runtime session).
//!
//! Real reported incident (2026-09-17): the embedding model (`multilingual-e5-small_quantized.onnx`,
//! 118MB on disk) used to load directly into `lanrurugi-server` itself, at server startup,
//! process-lifetime — same "load once, share via `Arc`" reasoning as the GPU-worker split
//! (`lanrurugi-gpu-ipc`'s own doc comment) minus the CUDA-crash-on-drop reason that split exists
//! for. A user's own stated standard ("主进程内存必须峰值不超过256MB" — the main server process
//! must never peak above 256MB resident) found `lanrurugi-server` idling at ~732MB RSS with zero
//! requests served, almost entirely attributable to this one model's ONNX Runtime session (an
//! ONNX Runtime session's actual resident memory is reliably several times a quantized model's own
//! on-disk byte count — intermediate tensor buffers, the runtime's own arena allocator, and the
//! tokenizer's vocabulary table all add up well past the raw weight bytes). Moving it into its own
//! subprocess — reachable only over this IPC boundary — gets the main process back under budget
//! the same way the GPU-inference split already did for `TextRecognizer`/`Inpainter`/
//! `BubbleSegmenter`.
//!
//! Unlike `lanrurugi-gpu-ipc`, this worker is CPU-only (`ort::CPUExecutionProvider`, no CUDA
//! session, no driver-crash-on-drop risk) — so `lanrurugi-embed-worker` can shut down gracefully
//! on idle rather than needing `SIGKILL`-only reclaim, and there is only ever one model group
//! (no `WorkerKind` split like the GPU worker's `Recognize`/`Image` — a single ~118MB CPU model
//! has no VRAM-contention reason to split across two processes the way the GPU worker's three
//! GPU-resident models did).
//!
//! Built on `tarpc` (Unix-socket transport, `bincode`-framed) for the same reasons
//! `lanrurugi-gpu-ipc`'s own doc comment gives (per-call deadline via `tarpc::context::Context`,
//! multiplexed transport tracking in-flight-call state so a cancelled call can't desync the byte
//! stream) — this crate deliberately mirrors that one's shape rather than inventing a second
//! protocol style for what is, at the wire level, an extremely similar problem (call out to a
//! subprocess holding a loaded ONNX model, get a `Result` back).

use serde::{Deserialize, Serialize};

/// Default Unix socket path the worker listens on and the client connects to. Overridable via
/// `LANRURUGI_EMBED_WORKER_SOCKET` for tests/multiple instances.
pub const DEFAULT_SOCKET_PATH: &str = "/tmp/lanrurugi-embed-worker.sock";

/// Same reasoning as `lanrurugi-gpu-ipc::MAX_FRAME_BYTES` — `tokio_util`'s own length-delimited
/// codec default (8MiB) is already generous for this protocol's own payloads (a title string in,
/// a few hundred `f32`s out), but both sides must still agree on an explicit value rather than
/// relying on a default that could change out from under this crate in a future `tokio_util`
/// upgrade. 1MiB is already wildly more than either message shape could ever need.
pub const MAX_FRAME_BYTES: usize = 1024 * 1024;

/// Whether a worker-side failure means "this request didn't work, but the session/process is
/// still fine to keep using" ([`Self::Config`]) or "the process itself may be in a broken state"
/// ([`Self::Fatal`]) — same split `lanrurugi-gpu-ipc::WorkerError` makes, for the same reason (the
/// client needs to know whether to keep reusing this session or discard it), even though a CPU
/// ONNX session has no equivalent to the GPU worker's own CUDA-crash-on-drop risk motivating the
/// original split; keeping the same two-variant shape here costs nothing and lets
/// `lanrurugi-api::embed_worker_client` reuse the exact same "Config keeps the session, Fatal
/// discards it" call-handling logic `gpu_worker_client.rs` already has, rather than inventing a
/// second one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkerError {
    /// A request/configuration problem (e.g. genuinely malformed input) — the worker process is
    /// still healthy, the client keeps reusing this session for the next call.
    Config(String),
    /// Something unexpected enough that the process's own state shouldn't be trusted further
    /// (a session-poisoning panic recovered at the boundary, an `ort::Error` mid-inference).
    /// Unlike the GPU worker, this process does not need to `std::process::exit` after sending
    /// this — a graceful `Drop` of a CPU-only ONNX session has none of the GPU worker's own
    /// documented crash risk — but the client still discards its session on this variant so a
    /// genuinely wedged worker doesn't keep getting reused.
    Fatal(String),
}

impl std::fmt::Display for WorkerError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WorkerError::Config(msg) | WorkerError::Fatal(msg) => write!(f, "{msg}"),
        }
    }
}

/// The RPC surface the worker exposes.
#[tarpc::service]
pub trait EmbedWorker {
    /// Embeds `text` into a normalized vector (`lanrurugi_recommend::embedding::Embedder::embed`).
    async fn embed(text: String) -> Result<Vec<f32>, WorkerError>;
    /// Cheap liveness probe — used by the client's readiness check, does not touch the model.
    async fn ping() -> ();
}
