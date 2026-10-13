//! Manages the `lanrurugi-gpu-worker` subprocess and the `tarpc` connection to it.
//!
//! GPU EP integration plan v9 (issue #103): every CUDA session (`TextRecognizer`, `Inpainter`,
//! `BubbleSegmenter`) used to live inside `lanrurugi-server` itself. A real hardware spike found
//! dropping a CUDA session triggers a reproducible ~35-60% crash rate rooted in NVIDIA's own
//! driver userspace component (`libnvidia-gpucomp.so`), not this project's code/`ort`/
//! onnxruntime — see `.debug-scratch/GPU_EP_FINAL_SUMMARY.md`. Leaking those sessions
//! (`Box::leak`, never dropping) avoids the crash but means their VRAM is held for the whole
//! server process's lifetime, with no way to reclaim it short of restarting the entire server —
//! taking down every unrelated feature (search, downloads, plugins, everything) along with it.
//!
//! The fix verified safe (n=20, zero crashes/driver corruption) is `SIGKILL`ing the process that
//! holds a CUDA session rather than ever calling `Drop` on it in-process. That process can't be
//! `lanrurugi-server` itself, so GPU inference now runs in a separate `lanrurugi-gpu-worker`
//! subprocess this module spawns, talks to over a Unix socket (`tarpc`), and `SIGKILL`s after its
//! own [`WorkerKind::idle_timeout`] of no requests — reclaiming 100% of its VRAM, at the cost of
//! re-paying a real subprocess-spawn-plus-model-load cold start (see [`CALL_TIMEOUT`]'s own doc
//! comment — this cold-start latency has not actually been independently measured end-to-end
//! against this exact subprocess architecture) on the next request after an idle period.
//!
//! **Two worker processes, not one** (added 2026-09-13, same issue #103): all three models resident
//! in one worker at once left essentially no VRAM free on an 8GB card, causing a spawn-OOM-kill-
//! respawn crash loop — see [`WorkerKind`]'s own doc comment for the real incident and why
//! `Recognize`/`Image` are split.

use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lanrurugi_gpu_ipc::{DetectedBubbleWire, GpuWorkerClient as RpcClient, RawMask, RawRgbImage};
use tarpc::client;
use tarpc::tokio_serde::formats::Bincode;
use tokio::io::{AsyncBufReadExt, AsyncRead};
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Which model group a `GpuWorkerClient` instance's own `lanrurugi-gpu-worker` subprocess loads —
/// mirrors `lanrurugi-gpu-worker`'s own `ModelGroup` (that crate can't share this type directly;
/// see `lanrurugi-gpu-ipc`'s own doc comment on why worker/client each define their own wire-
/// adjacent types rather than sharing a dependency).
///
/// Two `GpuWorkerClient`s, one per variant, are what `TranslationRuntime::load` actually
/// constructs (see that function) — never a single client toggling between groups. Splitting *why*:
/// a real incident (2026-09-13, issue #103) found all three models (recognizer 384MiB, bubble
/// segmenter 768MiB, inpainter 4GiB) resident in one worker process at once left essentially no
/// VRAM free on an 8GB card — the very next inference after all three finished loading failed with
/// "Available memory of 0", killing that worker, which respawned and immediately hit the same wall
/// reloading the same three models, a crash loop that silently dropped most of a page's OCR
/// regions (`lanrurugi_ocr::batch::run_batch` skips a region on recognition failure rather than
/// failing the whole batch, so the loop's damage was invisible as anything other than "translation
/// missed most of the dialogue"). `Recognize` and `Image` never share a CUDA context, so at most
/// one of {384MiB} or {768MiB + 4GiB} is ever resident at a time, not both summed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WorkerKind {
    /// `TextRecognizer` only (~384MiB) — cheap enough to keep the normal, longer idle timeout.
    Recognize,
    /// `BubbleSegmenter` + `Inpainter` together (~4.75GiB) — always called back-to-back for the
    /// same page in `translation_pipeline::composite_and_cache`, so there's no benefit to a third
    /// split; reclaimed on a much shorter idle timeout since holding ~4.75GiB idle is expensive.
    Image,
}

impl WorkerKind {
    /// Value `lanrurugi-gpu-worker`'s own `ModelGroup::from_env` expects on `LANRURUGI_GPU_WORKER_MODELS`.
    fn env_value(self) -> &'static str {
        match self {
            WorkerKind::Recognize => "recognize",
            WorkerKind::Image => "image",
        }
    }

    /// Distinct socket path per kind — two worker processes can't share one Unix socket, and a
    /// fixed suffix (rather than e.g. a PID) keeps `spawn_and_connect`'s stale-socket-file cleanup
    /// working the same way it always has for a single worker.
    fn socket_suffix(self) -> &'static str {
        match self {
            WorkerKind::Recognize => "recognize",
            WorkerKind::Image => "image",
        }
    }

    /// How long a connection can sit unused before the worker holding it is `SIGKILL`'d to reclaim
    /// its VRAM.
    fn idle_timeout(self) -> Duration {
        match self {
            // Middle ground between typical reading-session page intervals (long enough that a
            // reader flipping pages doesn't repeatedly pay the reload cost) and not holding GPU
            // memory indefinitely during real idle periods — same value this project used before
            // the `Recognize`/`Image` split, when there was only one worker/one timeout.
            WorkerKind::Recognize => Duration::from_secs(5 * 60),
            // Shorter than `Recognize`: ~4.75GiB idle is expensive to sit on, and `erase_page`/
            // `segment_bubbles` are only ever called together for one page at a time (never a
            // standing background look-ahead the way `recognize` is), so there's little
            // cross-request reuse to lose by reclaiming aggressively.
            //
            // Raised from an original 30s (2026-09-14, a real live incident): `translate_page` now
            // calls this worker twice per page — once for bubble segmentation up front, once for
            // `erase_page` afterward (see that function's own doc comment on why both calls were
            // consolidated into one worker call each, replacing two independent segmentation calls)
            // — with the LLM translation round trip (`translate_batch`, no `Image`-worker call at
            // all) sitting in between. `run_idle_reaper` polls every 10s and kills anything idle
            // past this timeout regardless of whether a second call is still coming — a real
            // request's own LLM leg alone (`LLM_REQUEST_TIMEOUT` allows up to 90s) reliably
            // outlasted the old 30s, killing the worker mid-request and making the *second* call
            // (`erase_page`) fail with "the connection to the server was already shutdown" — the
            // page's own original lettering was left undrawn-over on top of the translation instead
            // of erased.
            //
            // Raised again from 120s to 180s (2026-09-17, a second real live incident): under
            // genuine host memory pressure (this same session's own container restarted repeatedly
            // under it — see `CLAUDE.md`'s own guardrail notes), the real gap between
            // `segment_bubbles` finishing and `erase_page` starting measured 148s in server logs —
            // past even the once-already-raised 120s, since `LLM_REQUEST_TIMEOUT`'s own 90s ceiling
            // is a *cap*, not a typical latency, and real request latency (both the LLM round trip
            // itself and this worker's own idle-reaper polling granularity) grows under the same
            // resource contention that makes this scenario likely to matter in the first place.
            // 180s matches `ERASE_PAGE_TIMEOUT`'s own value (that constant's own doc comment covers
            // why it was chosen) — consistent headroom across the whole pipeline's own worst-case
            // timing rather than two independently-guessed numbers — while still reclaiming this
            // worker's VRAM well before `Recognize`'s own 5-minute ceiling during genuine idle
            // periods between page requests.
            WorkerKind::Image => Duration::from_secs(180),
        }
    }
}

/// Every RPC call gets this long to complete before the caller gives up on the current worker.
///
/// **Not yet independently re-verified against this actual subprocess architecture** — a real
/// first run hit `tarpc::context::current()`'s own 10s *default* deadline (a completely separate
/// timeout from this one, tracked inside the `Context` passed to each call rather than this
/// `tokio::time::timeout` wrapper — see [`rpc_context`]) before this constant's 30s ever had a
/// chance to matter, so 30s has not actually been exercised end-to-end yet against a real cold
/// start (spawn the worker + it loads three models, each building a CUDA session, before the
/// first `erase_page`/`recognize` call can even begin running). The in-process, same-model-cache,
/// no-subprocess-startup timing this project measured before this subprocess split
/// (`.debug-scratch/GPU_EP_FINAL_SUMMARY.md`, ~6s) is not the same measurement as this cold path
/// and shouldn't be treated as validating this number. If this value turns out to still be too
/// tight in practice, the right fix is measuring the real cold-start-to-first-response latency
/// end-to-end, not guessing a fourth number.
const CALL_TIMEOUT: Duration = Duration::from_secs(60);

/// `erase_page`'s own timeout, longer than [`CALL_TIMEOUT`] — measured live (2026-09-15) against
/// this actual subprocess architecture, the exact end-to-end measurement `CALL_TIMEOUT`'s own doc
/// comment said was still missing: a real cold start (worker spawn + all `Image`-group models
/// loaded, "models loaded, listening" logged at `11:41:12.375`) followed immediately by a real
/// `erase_page` call still hadn't returned by `11:42:05.850` (>53s and counting) before the then-
/// 60s `CALL_TIMEOUT` killed it — visible client-side as `whole-page inpainting failed; falling
/// back to flat-fill backdrops only`, which skips real background-color estimation entirely
/// (`bg_color` stays `None`), which is what left several regions' original Japanese text
/// undrawn-over beneath the translation on a real device. `erase_page`'s ONNX inference itself is
/// capped by `MAX_INPAINT_SIZE`, but a cold worker's first CUDA/cuDNN call also pays algorithm-
/// search and BFC-arena-growth overhead (seen in this same incident's logs as repeated "Extending
/// BFCArena for Cuda"/"Extended allocation by ... bytes" lines) on top of that — overhead
/// `recognize` calls essentially never pay for real because they run on the smaller, separate
/// `Recognize` worker, and `segment_bubbles` calls pay it too but are cheap enough on top of it
/// that they haven't been observed to approach `CALL_TIMEOUT` the way `erase_page` has (both run
/// on the same `Image` worker, `segment_bubbles` always first per `translate_page`'s own doc
/// comment, so by the time `erase_page` runs the worker is already warm — yet it was *this* call,
/// not `segment_bubbles`, that was observed exceeding 53s). 180s leaves comfortable headroom above
/// the ~53s+ observed without masking a truly hung worker for multiple minutes.
const ERASE_PAGE_TIMEOUT: Duration = Duration::from_secs(180);

/// Builds a [`tarpc::context::Context`] with `deadline` as its own deadline — never bare
/// `tarpc::context::current()`, whose *default* deadline is a hardcoded 10 seconds
/// (`tarpc::context::current_deadline`) entirely independent of, and shorter than, this module's
/// own `tokio::time::timeout(deadline, ...)` wrapper around each call — real behavior confirmed
/// live: a first real cold-start call hit `DeadlineExceeded` from `tarpc`'s own in-flight request
/// tracking at ~10s, well before `CALL_TIMEOUT` (then 30s) could ever have fired. Using
/// `context::current()` anywhere in this module would silently reintroduce that same 10s ceiling.
fn rpc_context(timeout: Duration) -> tarpc::context::Context {
    let mut ctx = tarpc::context::current();
    ctx.deadline = std::time::Instant::now() + timeout;
    ctx
}

/// Which path an erase call takes, decided from the page's dimensions alone.
///
/// Kept as a pure function over two `u32`s — rather than inline in the async call — so the decision
/// that governs whether an already-working page keeps its existing code path can be tested without
/// a worker, a model, a GPU, or a socket.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ErasePlan {
    /// The full-resolution request provably fits within the boundary's limit: send the page whole,
    /// exactly as this code did before the reduced path existed.
    FullResolution,
    /// The full-resolution request does not provably fit, so the page and its model mask are
    /// reduced to these dimensions before being sent.
    Reduced { width: u32, height: u32 },
    /// The request does not provably fit and cannot be reduced, because the page is already at the
    /// working resolution. Unreachable for any real page — a page at the working resolution is far
    /// under the limit — but reported rather than attempted, since attempting it is the silent
    /// failure this decision exists to prevent.
    CannotBound { attempted_bytes: u64 },
}

fn erase_plan(width: u32, height: u32) -> ErasePlan {
    if lanrurugi_gpu_ipc::erase_request_provably_fits(width, height) {
        return ErasePlan::FullResolution;
    }
    // `lanrurugi_inpaint::work_dims`, not a copy of it: the worker reduces by this same rule, and a
    // divergence would hand the model a differently-shaped page and mask without ever erroring.
    let (work_width, work_height) = lanrurugi_inpaint::work_dims(width, height);
    if (work_width, work_height) == (width, height) {
        ErasePlan::CannotBound {
            attempted_bytes: lanrurugi_gpu_ipc::erase_request_bytes(width, height),
        }
    } else {
        ErasePlan::Reduced {
            width: work_width,
            height: work_height,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum GpuWorkerError {
    #[error("failed to spawn lanrurugi-gpu-worker: {0}")]
    Spawn(std::io::Error),
    #[error("failed to connect to lanrurugi-gpu-worker: {0}")]
    Connect(std::io::Error),
    #[error("lanrurugi-gpu-worker did not become ready within {0:?}")]
    NotReady(Duration),
    #[error("RPC call to lanrurugi-gpu-worker failed: {0}")]
    Rpc(#[from] tarpc::client::RpcError),
    #[error("RPC call to lanrurugi-gpu-worker timed out after {0:?}")]
    Timeout(Duration),
    /// A [`lanrurugi_gpu_ipc::WorkerError::Config`] response — the worker rejected this specific
    /// request (no model loaded, malformed input, a model-level verdict on the content) while its
    /// own session stayed healthy. Kept separate from [`Self::WorkerFatal`] so callers can tell a
    /// reproducible per-request verdict from a broken-session failure; issue #101 needs exactly
    /// that distinction to decide whether a degraded OCR result is safe to cache.
    #[error("lanrurugi-gpu-worker reported an error: {0}")]
    Worker(String),
    /// A [`lanrurugi_gpu_ipc::WorkerError::Fatal`] response — the worker's CUDA session was
    /// (or may have been) broken and the worker process exits right after sending this. Always
    /// transient from the caller's point of view: the next call spawns a fresh worker.
    #[error("lanrurugi-gpu-worker reported an error: {0}")]
    WorkerFatal(String),
    /// The full-resolution erase request would exceed the worker boundary's message limit and could
    /// not be reduced to fit. Carries both numbers, and is distinct by *variant* rather than by
    /// message text, so a size problem can never be read as a model-side verdict — exactly the
    /// confusion that let this defect go unnoticed until the boundary was measured.
    #[error(
        "erase_page request of {attempted_bytes} bytes exceeds the worker boundary's {limit_bytes} \
         byte message limit and cannot be reduced to fit"
    )]
    PayloadTooLarge {
        attempted_bytes: u64,
        limit_bytes: u64,
    },
    #[error("could not locate the lanrurugi-gpu-worker binary next to the running executable")]
    BinaryNotFound,
}

impl GpuWorkerError {
    /// Maps this error onto the infrastructure-vs-content split
    /// [`lanrurugi_ocr::batch::TextRecognizerHandle`] callers need (issue #101).
    ///
    /// Everything except [`Self::Worker`] is infrastructure: a spawn/connect/ready/RPC/timeout
    /// failure means the model never judged the crop at all, and `WorkerFatal` means it was
    /// judging it on a session that then died — both are transient and a retry could well
    /// succeed, so a result missing that region must not be cached as final. Only `Worker`
    /// (a `WorkerError::Config` response — the worker answered about *this crop* with its own
    /// session intact) is a reproducible content verdict.
    fn classify_for_recognition(&self) -> lanrurugi_ocr::batch::RecognizeHandleError {
        match self {
            GpuWorkerError::Worker(msg) => {
                lanrurugi_ocr::batch::RecognizeHandleError::Content(msg.clone())
            }
            other => lanrurugi_ocr::batch::RecognizeHandleError::Infrastructure(other.to_string()),
        }
    }
}

/// Drains one of a spawned worker's output streams, re-emitting each line through this process's
/// own `tracing` subscriber — see `spawn_and_connect`'s own comment for the real incident this
/// exists to prevent. The continuous read is the actual fix (the worker can never block or fail
/// on a full pipe); the re-emission is what keeps those lines visible in the server's own
/// `general.log` rather than losing them.
async fn forward_worker_output<R: AsyncRead + Unpin>(reader: R, stream: &'static str) {
    let mut lines = tokio::io::BufReader::new(reader).lines();
    loop {
        match lines.next_line().await {
            Ok(Some(line)) => {
                tracing::info!(target: "lanrurugi_gpu_worker", stream, "{line}")
            }
            Ok(None) => break,
            Err(error) => {
                tracing::debug!(
                    target: "lanrurugi_gpu_worker",
                    stream,
                    %error,
                    "worker output stream ended with an error"
                );
                break;
            }
        }
    }
}

struct Session {
    child: Child,
    client: RpcClient,
    last_used: Instant,
}

/// Which GPU vendor's ONNX Runtime build a worker should load, detected once at spawn time —
/// NVIDIA takes priority over Intel when both are present (this project's own CUDA path is the
/// one actually hardware-verified; see `lanrurugi-inpaint`/`lanrurugi-ocr`'s own `CUDA_SESSION_
/// MEMORY_LIMIT_BYTES` doc comments for the real spike history), Intel next, CPU as the always-
/// available fallback when neither GPU vendor is present or the caller explicitly overrides via
/// `LANRURUGI_GPU_WORKER_ORT_DYLIB_PATH`.
///
/// AMD is deliberately not included — see this crate's own module doc on why: no ONNX Runtime
/// execution provider for AMD has a verified-safe Linux integration path in this project the way
/// CUDA and OpenVINO do (MIGraphX exists upstream but hasn't been evaluated here).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum GpuVendor {
    Nvidia,
    Intel,
}

impl GpuVendor {
    /// The `ort-{gpu,openvino}` directory `Dockerfile.dev` installs this vendor's runtime build
    /// into — see that file's own ORT-setup comments for why there are separate directories at
    /// all (a `load-dynamic` build only ever `dlopen`s one `.so`, so mixing builds in one directory
    /// would make whichever copied there last silently win for every consumer, GPU worker included).
    fn ort_dylib_path(self) -> &'static str {
        match self {
            GpuVendor::Nvidia => "/usr/local/lib/ort-gpu/libonnxruntime.so",
            GpuVendor::Intel => "/usr/local/lib/ort-openvino/libonnxruntime.so",
        }
    }
}

/// PCI vendor ID `lspci -nn`'s bracketed `[vvvv:dddd]` suffix reports for each GPU vendor this
/// module can route to — see `detect_gpu_vendor`'s own doc comment for why PCI enumeration (not
/// `ort`'s own `is_available()`/device-listing APIs) is what runs this detection.
const PCI_VENDOR_NVIDIA: &str = "10de";
const PCI_VENDOR_INTEL: &str = "8086";

/// Detects which GPU vendor (if any) is actually present on the host, by shelling out to `lspci
/// -nn` and matching PCI vendor IDs on VGA/3D/display-class controllers — same shell-out-to-a-
/// system-tool discipline `~/jellyfin-suite/crates/frame-forge::gpu_compat` uses for its own
/// `nvidia-smi`-based hardware checks, for the same reason: this has to run **before** any ORT
/// library is ever loaded (to pick *which* `ORT_DYLIB_PATH` the worker process should even start
/// with), so `ort`'s own `Environment::devices()`/`is_available()` APIs — which only report on
/// whatever build happens to already be loaded — aren't usable here at all.
///
/// Returns `None` on any failure (`lspci` missing, no output, no recognized vendor) — the caller
/// treats that identically to "no GPU detected" and falls back to `GpuVendor::ort_dylib_path`'s
/// CPU-only default, never a hard error; a host with no GPU at all is an expected, ordinary case,
/// not a misconfiguration. NVIDIA is checked first and returned immediately if found — see this
/// type's own doc comment for the priority rationale.
fn detect_gpu_vendor() -> Option<GpuVendor> {
    let output = std::process::Command::new("lspci")
        .arg("-nn")
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let controller_lines = text
        .lines()
        .filter(|line| {
            let lower = line.to_ascii_lowercase();
            lower.contains("vga compatible controller")
                || lower.contains("3d controller")
                || lower.contains("display controller")
        })
        .collect::<Vec<_>>();

    let has_vendor = |vendor_id: &str| {
        controller_lines
            .iter()
            .any(|line| line.contains(&format!("[{vendor_id}:")))
    };

    if has_vendor(PCI_VENDOR_NVIDIA) {
        Some(GpuVendor::Nvidia)
    } else if has_vendor(PCI_VENDOR_INTEL) {
        Some(GpuVendor::Intel)
    } else {
        None
    }
}

/// `Child::start_kill`'s own docs warn that on Unix a killed-but-never-`wait`ed-for child becomes
/// a zombie process — `wait()` itself is async (reaping requires polling `SIGCHLD`), so this
/// spawns a detached task to actually reap it rather than making the caller (holding the session
/// lock) await that here.
/// Kills `child` and waits for the OS to actually reap it (and, per [`VRAM_RECLAIM_SETTLE`]'s own
/// doc comment, for the driver to actually release its VRAM) before returning — *not* a
/// fire-and-forget `tokio::spawn`. A real 2026-09-14 incident, reproduced live: `discard_session`
/// used to detach the wait, so the very next `ensure_session` call (a near-certainty under
/// concurrent load — several readers/tabs polling the same in-flight translation) could spawn a
/// *replacement* worker while the old one's CUDA context was still tearing down and still holding
/// its VRAM allocation, and the new worker's own session-init `cudaMalloc` calls raced that
/// teardown for the same physical memory — `Available memory of N` failures with `N` well under
/// what `nvidia-smi` reports free a few seconds later, confirmed against real logs from that
/// incident. `discard_session` awaiting this directly, guard still held, is what actually
/// serializes "old worker's VRAM is gone" before "new worker asks for VRAM" can happen at all.
async fn kill_and_reap(mut child: Child) {
    if let Err(e) = child.start_kill() {
        tracing::warn!(error = %e, "failed to send SIGKILL to lanrurugi-gpu-worker (already exited?)");
        return;
    }
    let _ = child.wait().await;
    // `wait()` returning only means the OS has reaped the zombie process — it says nothing about
    // whether the NVIDIA driver has finished reclaiming that process's VRAM allocations yet (that
    // reclaim happens asynchronously to process exit, not synchronously within it). This settle
    // window is the cheapest way to bound that race without polling `nvidia-smi` from inside the
    // server itself; a real reported 2026-09-14 incident needed it before the very next worker's
    // own session-init VRAM allocation stopped intermittently failing with `Available memory of 0`
    // (or similar small-but-nonzero, `nvidia-smi` disagreeing) despite the killed process no longer
    // appearing in `nvidia-smi`'s own process list by the time the next spawn's log line appeared.
    tokio::time::sleep(VRAM_RECLAIM_SETTLE).await;
}

/// See [`kill_and_reap`]'s own doc comment for the incident this exists to prevent. Not tuned
/// against a measured worst-case reclaim latency (no such measurement exists yet) — a placeholder
/// long enough to clear what was observed live, short enough not to meaningfully lengthen a page
/// translation's own already-multi-second critical path when this path is actually taken (a
/// session discard, not the common case of reusing a warm session).
const VRAM_RECLAIM_SETTLE: Duration = Duration::from_millis(500);

/// The single shared handle `AppState` hands out for one [`WorkerKind`]. Cloning is cheap (`Arc`
/// internally) — concurrent callers share the same underlying worker process and connection rather
/// than each spawning their own. `TranslationRuntime::load` constructs one of these per
/// `WorkerKind` variant, never a single instance shared across kinds — see that type's own doc
/// comment for why.
#[derive(Clone)]
pub struct GpuWorkerClient {
    inner: Arc<Mutex<Option<Session>>>,
    /// Serializes actual RPC dispatch to this worker — held for the duration of one `call()`,
    /// released before the next queued caller starts *its* [`CALL_TIMEOUT`] clock. Without this,
    /// concurrent callers (e.g. `translate_page` running for the current page and several
    /// look-ahead pages at once, per `TranslationScheduler`) all send their RPC immediately and
    /// race each other inside the same worker process/CUDA session; each call's 60s deadline is
    /// fixed at the moment it's sent, so time spent waiting behind an earlier call (the worker can
    /// only actually run one inference at a time on one GPU) was silently eating into that budget
    /// instead of being excluded from it. Confirmed live, 2026-09-14: several look-ahead pages'
    /// `segment_bubbles` calls landing within the same ~1s window produced repeated
    /// `DeadlineExceeded`/"already shutdown" failures and a worker kill-respawn loop, even though
    /// each individual inference (confirmed via `models loaded, listening` to next request timing)
    /// completed in a few seconds — the failures were queuing artifacts, not real per-call
    /// slowness. This lock makes queued callers wait here (free, no deadline) rather than inside
    /// the RPC's own timeout window.
    call_lock: Arc<Mutex<()>>,
    kind: WorkerKind,
    socket_path: String,
    /// Captured at construction time (always on the real Tokio runtime — `AppState` is built
    /// during server startup, never inside a `rayon` worker). The `TextRecognizerHandle`/
    /// `InpainterHandle`/`BubbleSegmenterHandle` trait impls below bridge their sync methods back
    /// to this type's own `async fn`s via `Handle::block_on` — they used to reach for
    /// `tokio::runtime::Handle::current()` instead, which panics ("there is no reactor running")
    /// when the calling thread is one of `lanrurugi-ocr`'s `rayon` parallel-map workers rather than
    /// a real Tokio worker thread (`lanrurugi_core::concurrency::parallel_map` dispatches onto
    /// rayon's own global pool, which shares no relationship with the Tokio runtime at all — see
    /// issue #104). Storing the handle once here and reusing it sidesteps that: `Handle::block_on`
    /// only needs a valid handle to *a* runtime, not to call from a thread that runtime owns.
    runtime: tokio::runtime::Handle,
}

impl std::fmt::Debug for GpuWorkerClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GpuWorkerClient")
            .field("kind", &self.kind)
            .finish_non_exhaustive()
    }
}

impl GpuWorkerClient {
    /// `LANRURUGI_GPU_WORKER_SOCKET`, if set, is used as a base path with `kind`'s own suffix
    /// appended (rather than as the literal path) — both `WorkerKind` variants must never collide
    /// on one socket, even when a test/dev override is in play.
    pub fn new(kind: WorkerKind) -> Self {
        let base = std::env::var("LANRURUGI_GPU_WORKER_SOCKET")
            .unwrap_or_else(|_| lanrurugi_gpu_ipc::DEFAULT_SOCKET_PATH.to_string());
        let socket_path = format!("{base}.{}", kind.socket_suffix());
        Self {
            inner: Arc::new(Mutex::new(None)),
            call_lock: Arc::new(Mutex::new(())),
            kind,
            socket_path,
            runtime: tokio::runtime::Handle::current(),
        }
    }

    /// Locates `lanrurugi-gpu-worker` next to the currently running binary — same
    /// binary-relative-directory convention `lanrurugi-ocr`/`lanrurugi-inpaint`'s own
    /// `model_discovery` modules already use, so the production image (both binaries `COPY`'d
    /// into the same `/usr/local/bin`) needs no extra configuration.
    fn worker_binary_path() -> Result<std::path::PathBuf, GpuWorkerError> {
        let exe = std::env::current_exe().map_err(|_| GpuWorkerError::BinaryNotFound)?;
        let dir = exe.parent().ok_or(GpuWorkerError::BinaryNotFound)?;
        let candidate = dir.join("lanrurugi-gpu-worker");
        if candidate.is_file() {
            return Ok(candidate);
        }
        // Dev-container/cargo-run convenience: the two binaries land in the same `target/debug`
        // or `target/release` directory as sibling build artifacts, same directory shape as the
        // production image's `/usr/local/bin`, so this is the same check, not a separate case.
        Err(GpuWorkerError::BinaryNotFound)
    }

    /// Spawns a fresh worker process and connects to it, replacing whatever the current session
    /// is (if any — the caller is responsible for having already decided the old one, if it
    /// existed, is unusable). Waits for the worker to answer a real `ping()` before returning, so
    /// callers never race the worker's own model-loading window.
    async fn spawn_and_connect(&self) -> Result<Session, GpuWorkerError> {
        let binary = Self::worker_binary_path()?;
        let _ = std::fs::remove_file(&self.socket_path);

        tracing::info!(path = %self.socket_path, kind = ?self.kind, "spawning lanrurugi-gpu-worker");
        let mut child = Command::new(binary)
            .env("LANRURUGI_GPU_WORKER_SOCKET", &self.socket_path)
            // Tells the worker which model(s) to load — see `WorkerKind`'s own doc comment for why
            // `Recognize`/`Image` are always separate processes, never one worker toggling groups.
            .env("LANRURUGI_GPU_WORKER_MODELS", self.kind.env_value())
            // Explicit override, not inherited from this (the parent, `lanrurugi-server`)
            // process's own environment — the parent's `ORT_DYLIB_PATH` (if set at all) points at
            // the CPU-only build `lanrurugi-recommend` uses; the worker needs a GPU-capable build
            // instead. `LANRURUGI_GPU_WORKER_ORT_DYLIB_PATH` is an explicit escape hatch for tests/
            // manual overrides; absent that, `detect_gpu_vendor()` picks NVIDIA's CUDA13 build,
            // Intel's OpenVINO build, or falls back to CPU — see `GpuVendor`'s own doc comment for
            // the priority order and `Dockerfile.dev`'s ORT-setup comments for why each vendor gets
            // its own directory (a `load-dynamic` build only ever `dlopen`s one `.so`; mixing
            // builds in one directory would make whichever copied there last silently win).
            .env(
                "ORT_DYLIB_PATH",
                std::env::var_os("LANRURUGI_GPU_WORKER_ORT_DYLIB_PATH").unwrap_or_else(|| {
                    detect_gpu_vendor()
                        .map(GpuVendor::ort_dylib_path)
                        .unwrap_or("/usr/local/lib/ort-cpu/libonnxruntime.so")
                        .into()
                }),
            )
            .stdin(Stdio::null())
            // Piped, not inherited: `forward_worker_output` below drains both streams and
            // re-emits them through this process's own `tracing`. A real 2026-09-18 incident
            // showed why inheriting is unsafe — the container's journald-backed stdout pipe
            // filled up under heavy ORT logging, the worker's own `tracing_subscriber` write
            // then returned EAGAIN, and its recovery warning (`eprintln!` to the *same* broken
            // stderr pipe) panicked the process (exit 101) before it ever bound its socket. The
            // client saw every spawn as "did not become ready within 90s" and every page
            // silently fell back to the untranslated original. A pipe with a reader that is
            // always draining cannot fill up, so this class of failure can no longer take down
            // inference. Log volume is unchanged — the same lines still reach the server's logs,
            // just via `general.log` instead of container stdout.
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            // Linux-only: ensures the worker is killed if this parent process itself dies
            // unexpectedly (e.g. OOM-killed), rather than leaking an orphaned worker holding
            // GPU memory that nothing will ever clean up again.
            .kill_on_drop(true)
            .spawn()
            .map_err(GpuWorkerError::Spawn)?;

        if let Some(stdout) = child.stdout.take() {
            tokio::spawn(forward_worker_output(stdout, "stdout"));
        }
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(forward_worker_output(stderr, "stderr"));
        }

        let client = Self::connect_with_retry(&self.socket_path).await?;

        Ok(Session {
            child,
            client,
            last_used: Instant::now(),
        })
    }

    /// Retries the initial connection while the worker is still starting up and loading models
    /// (`lanrurugi_ocr::model_discovery`/`lanrurugi_inpaint::model_discovery` disk I/O + CUDA
    /// session construction — observed a few seconds cold, see `.debug-scratch/
    /// GPU_EP_FINAL_SUMMARY.md`'s own timing notes) — the socket file may not exist yet the
    /// instant after `spawn()` returns.
    ///
    /// `READY_TIMEOUT` was `30s` until a real reported incident (2026-09-16/17): under genuine
    /// host memory pressure (this same session's own container restarted repeatedly under it —
    /// see `CLAUDE.md`'s own guardrail notes on that), the `Image` worker's cold-start model load
    /// (`lanrurugi-gpu-worker::main::load_worker`, now parallelized across both its models rather
    /// than sequential — see that function's own doc comment on that separate fix) still routinely
    /// took longer than 30s to become reachable at all, at which point every caller degrades to
    /// `translation_pipeline::composite_and_cache`'s own `flat_fill_fallback` — which only ever
    /// fills `bg_color: Some` regions, so a page whose regions are mostly `bg_color: None` (exactly
    /// the case the DenseCRF erase path exists for) got *no* erasure at all, not merely a
    /// lower-quality one. `90s` leaves real headroom for a genuinely slow cold start under host
    /// pressure without masking a truly hung/crashed worker for multiple minutes — still well
    /// under `ERASE_PAGE_TIMEOUT`'s own `180s` (that constant's own doc comment covers why *that*
    /// value was chosen), so a worker that connects within this window still has its own full
    /// `ERASE_PAGE_TIMEOUT` budget for the actual `erase_page` RPC call once connected.
    async fn connect_with_retry(socket_path: &str) -> Result<RpcClient, GpuWorkerError> {
        const READY_TIMEOUT: Duration = Duration::from_secs(90);
        const RETRY_INTERVAL: Duration = Duration::from_millis(100);
        let deadline = Instant::now() + READY_TIMEOUT;

        loop {
            match Self::try_connect(socket_path).await {
                Ok(client) => return Ok(client),
                Err(_) if Instant::now() < deadline => {
                    tokio::time::sleep(RETRY_INTERVAL).await;
                }
                Err(_) => return Err(GpuWorkerError::NotReady(READY_TIMEOUT)),
            }
        }
    }

    async fn try_connect(socket_path: &str) -> Result<RpcClient, GpuWorkerError> {
        let mut transport = tarpc::serde_transport::unix::connect(socket_path, Bincode::default);
        transport
            .config_mut()
            .max_frame_length(lanrurugi_gpu_ipc::MAX_FRAME_BYTES);
        let transport = transport.await.map_err(GpuWorkerError::Connect)?;
        let client = RpcClient::new(client::Config::default(), transport).spawn();
        // A fresh TCP/UDS connection succeeding doesn't mean the worker has finished loading
        // models yet (the OS accepts the connection before the worker's own `main` even gets to
        // `listen`'s stream loop in some races) — an actual `ping()` round-trip is the only real
        // readiness signal. `rpc_context()`, not `context::current()` — see that function's own
        // doc comment for why the latter's 10s default deadline is real trouble here.
        client
            .ping(rpc_context(CALL_TIMEOUT))
            .await
            .map_err(|_| GpuWorkerError::Connect(std::io::Error::other("ping failed")))?;
        Ok(client)
    }

    /// Returns a connected client, spawning a worker first if none is currently running (either
    /// because this is the first call ever, or a previous worker was `SIGKILL`'d for being idle,
    /// or the last call detected the connection was no longer usable).
    async fn ensure_session(&self) -> Result<RpcClient, GpuWorkerError> {
        let mut guard = self.inner.lock().await;
        if let Some(session) = guard.as_mut() {
            session.last_used = Instant::now();
            return Ok(session.client.clone());
        }
        let session = self.spawn_and_connect().await?;
        let client = session.client.clone();
        *guard = Some(session);
        Ok(client)
    }

    /// Marks the current session as unusable, killing the worker outright — used when an RPC call
    /// fails in a way that suggests the connection (or the worker itself) is no longer trustworthy
    /// (a timeout, a transport error). The *next* call to any method here will transparently spawn
    /// a fresh worker; the caller of the failed call still sees that one call's own error.
    async fn discard_session(&self) {
        let mut guard = self.inner.lock().await;
        if let Some(session) = guard.take() {
            tracing::warn!("discarding lanrurugi-gpu-worker session, killing the process");
            // Awaited with `guard` still held (see `kill_and_reap`'s own doc comment for why this
            // matters): the next `ensure_session` call blocks on this same mutex, so it cannot
            // spawn a replacement worker — and race the old one's VRAM teardown — until this
            // function has actually confirmed that VRAM is gone.
            kill_and_reap(session.child).await;
        }
    }

    /// Runs one RPC call against the current (or freshly spawned) worker, applying [`CALL_TIMEOUT`]
    /// and discarding the session on any failure so the next call starts clean rather than
    /// repeatedly retrying against a connection already known to be bad.
    ///
    /// A [`WorkerError::Fatal`] response also discards the session — added after a real incident
    /// (2026-09-13, issue #103) where every worker error used to be an opaque `String`, so a
    /// `CUBLAS`/`CUDNN` CUDA-OOM failure looked identical to a benign "no model loaded" error and
    /// the client kept reusing an already-broken session until `lanrurugi-server` stopped
    /// responding to anything. The worker itself also exits shortly after sending a `Fatal`
    /// response (see `lanrurugi-gpu-worker`'s `exit_after_fatal_response`), so this is belt-and-
    /// suspenders — either side noticing first is enough to stop the broken session being reused.
    async fn call<T, F, Fut>(&self, timeout: Duration, f: F) -> Result<T, GpuWorkerError>
    where
        F: FnOnce(RpcClient) -> Fut,
        Fut: std::future::Future<
            Output = Result<Result<T, lanrurugi_gpu_ipc::WorkerError>, tarpc::client::RpcError>,
        >,
    {
        // Waits here, not against `timeout`, if another caller's RPC is already in flight — see
        // `call_lock`'s own doc comment for the incident this prevents.
        let _call_permit = self.call_lock.lock().await;
        let client = self.ensure_session().await?;
        match tokio::time::timeout(timeout, f(client)).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(lanrurugi_gpu_ipc::WorkerError::Config(worker_error)))) => {
                // A request/configuration problem (e.g. "no inpainting model loaded") — the
                // connection/process are still healthy, so the session is kept, not discarded.
                Err(GpuWorkerError::Worker(worker_error))
            }
            Ok(Ok(Err(lanrurugi_gpu_ipc::WorkerError::Fatal(worker_error)))) => {
                self.discard_session().await;
                Err(GpuWorkerError::WorkerFatal(worker_error))
            }
            Ok(Err(rpc_error)) => {
                self.discard_session().await;
                Err(GpuWorkerError::Rpc(rpc_error))
            }
            Err(_elapsed) => {
                self.discard_session().await;
                Err(GpuWorkerError::Timeout(timeout))
            }
        }
    }

    pub async fn recognize(&self, crop: RawRgbImage) -> Result<String, GpuWorkerError> {
        self.call(CALL_TIMEOUT, |client| async move {
            client.recognize(rpc_context(CALL_TIMEOUT), crop).await
        })
        .await
    }

    /// Batched [`Self::recognize`] — one RPC for a page's crops so the worker runs manga-ocr's
    /// encoder once. Always returns exactly `crops.len()` entries (positional): a transport-level
    /// failure is replicated across every slot rather than returned as a short vector.
    pub async fn recognize_batch(
        &self,
        crops: Vec<RawRgbImage>,
    ) -> Vec<Result<String, GpuWorkerError>> {
        let batch_len = crops.len();
        match self
            .call(CALL_TIMEOUT, |client| async move {
                client
                    .recognize_batch(rpc_context(CALL_TIMEOUT), crops)
                    .await
            })
            .await
        {
            Ok(results) => results
                .into_iter()
                .map(|result| {
                    result.map_err(|error| match error {
                        lanrurugi_gpu_ipc::WorkerError::Config(message) => {
                            GpuWorkerError::Worker(message)
                        }
                        lanrurugi_gpu_ipc::WorkerError::Fatal(message) => {
                            GpuWorkerError::WorkerFatal(message)
                        }
                    })
                })
                .collect(),
            Err(error) => {
                // See the doc comment: keep the vector positional. `WorkerFatal` is the
                // infrastructure bucket `classify_for_recognition` maps to a retryable failure.
                let message = error.to_string();
                (0..batch_len)
                    .map(|_| Err(GpuWorkerError::WorkerFatal(message.clone())))
                    .collect()
            }
        }
    }

    /// Uses [`ERASE_PAGE_TIMEOUT`], not [`CALL_TIMEOUT`] — see that constant's own doc comment for
    /// the real cold-start measurement that showed 60s isn't enough for this specific call.
    ///
    /// Two paths, chosen by [`erase_plan`] from the page's dimensions alone. A page whose
    /// full-resolution request provably fits takes the original path unchanged — that is what makes
    /// "pages that already worked are pixel-identical" true by construction rather than by
    /// argument. A page that does not fit is reduced first (see [`Self::erase_page_reduced`]).
    pub async fn erase_page(
        &self,
        page: RawRgbImage,
        model_mask: RawMask,
        paste_mask: RawMask,
    ) -> Result<RawRgbImage, GpuWorkerError> {
        match erase_plan(page.width, page.height) {
            ErasePlan::FullResolution => {
                self.call(ERASE_PAGE_TIMEOUT, |client| async move {
                    client
                        .erase_page(
                            rpc_context(ERASE_PAGE_TIMEOUT),
                            page,
                            model_mask,
                            paste_mask,
                        )
                        .await
                })
                .await
            }
            ErasePlan::Reduced { width, height } => {
                self.erase_page_reduced(page, model_mask, paste_mask, width, height)
                    .await
            }
            ErasePlan::CannotBound { attempted_bytes } => {
                let limit_bytes = lanrurugi_gpu_ipc::MAX_FRAME_BYTES as u64;
                tracing::error!(
                    width = page.width,
                    height = page.height,
                    attempted_bytes,
                    limit_bytes,
                    "erase_page request exceeds the worker boundary's message limit and cannot be \
                     reduced to fit — this is a size-limit failure, not a model failure"
                );
                Err(GpuWorkerError::PayloadTooLarge {
                    attempted_bytes,
                    limit_bytes,
                })
            }
        }
    }

    /// The over-limit path: reduce the page and its model mask to the working resolution, let the
    /// worker run the whole model call on that, then restore the model's output to the page's
    /// original dimensions and composite there against the **original full-resolution** paste mask,
    /// which never crosses the boundary.
    ///
    /// The reduced request carries an all-covering paste mask, which asks
    /// `Inpainter::erase_page` for the model's output over the whole working image rather than also
    /// compositing it: that function starts from a clone of its input and overwrites only where the
    /// mask is set, so covering everything means exactly "give me the model's result". This is why
    /// the worker, the RPC trait and the wire protocol need no change at all.
    ///
    /// Memory, stated plainly: a full-resolution page necessarily exists in this process either way,
    /// because the caller needs one back and the input already is one. What changes is that the
    /// message assembled for the boundary — previously the page plus two full-resolution masks, at
    /// up to ~287MiB for a 60-megapixel page — becomes bounded by the working resolution, and the
    /// composite writes into the buffer this call already owns rather than a fresh copy. Total
    /// process memory therefore falls by the size of the eliminated message and masks, **not** by
    /// the page's own size.
    async fn erase_page_reduced(
        &self,
        page: RawRgbImage,
        model_mask: RawMask,
        paste_mask: RawMask,
        work_width: u32,
        work_height: u32,
    ) -> Result<RawRgbImage, GpuWorkerError> {
        let (width, height) = (page.width, page.height);
        // The full-resolution path gets this check from the worker; this path composites locally, so
        // it must not index a short mask — and must not spend a worker round trip finding out.
        check_paste_mask_len(width, height, &paste_mask.mask).map_err(GpuWorkerError::Worker)?;

        tracing::info!(
            width,
            height,
            full_request_bytes = lanrurugi_gpu_ipc::erase_request_bytes(width, height),
            work_width,
            work_height,
            "page exceeds the worker boundary's message limit at full resolution; reducing it to \
             the model's working resolution before the erase call"
        );

        let full_page = raw_to_rgb_image(page).map_err(GpuWorkerError::Worker)?;
        let (work_page, work_mask) =
            lanrurugi_inpaint::downscale_to_work(&full_page, &model_mask.mask)
                .map_err(|e| GpuWorkerError::Worker(e.to_string()))?;
        debug_assert_eq!(work_page.dimensions(), (work_width, work_height));

        // `Nearest` reduction leaves exactly 0 or 255, so any mid threshold recovers the same
        // booleans — the same convention `Inpainter::erase_page` uses when it feeds this mask to
        // the model.
        let work_mask: Vec<bool> = work_mask.pixels().map(|p| p.0[0] > 127).collect();

        let work_raw = rgb_image_to_raw(&work_page);
        let work_mask_raw = RawMask {
            width: work_width,
            height: work_height,
            mask: work_mask,
        };
        let all_covering = RawMask {
            width: work_width,
            height: work_height,
            mask: vec![true; (work_width as usize) * (work_height as usize)],
        };
        drop(work_page);

        let erased_small = self
            .call(ERASE_PAGE_TIMEOUT, |client| async move {
                client
                    .erase_page(
                        rpc_context(ERASE_PAGE_TIMEOUT),
                        work_raw,
                        work_mask_raw,
                        all_covering,
                    )
                    .await
            })
            .await?;

        let small = raw_to_rgb_image(erased_small).map_err(GpuWorkerError::Worker)?;

        // Composite at full resolution, into the buffer this call already owns. Only paste-mask
        // pixels may come from the model; every other pixel stays the original page's own — the
        // invariant `finish_composite_page` depends on when it replaces the page wholesale.
        let mut result = full_page;
        composite_restored_output(&mut result, &small, &paste_mask.mask)
            .map_err(GpuWorkerError::Worker)?;

        Ok(RawRgbImage {
            width,
            height,
            rgb: result.into_raw(),
        })
    }

    pub async fn segment_bubbles(
        &self,
        page: RawRgbImage,
    ) -> Result<Vec<DetectedBubbleWire>, GpuWorkerError> {
        self.call(CALL_TIMEOUT, |client| async move {
            client
                .segment_bubbles(rpc_context(CALL_TIMEOUT), page)
                .await
        })
        .await
    }

    /// Spawned once per `GpuWorkerClient` at server startup (`lib.rs`'s own build-app wiring) —
    /// periodically kills the worker if it's been idle past `self.kind.idle_timeout()`, reclaiming
    /// its VRAM. Runs for the whole server process's lifetime; there is deliberately no shutdown
    /// signal wired to stop this loop early, since it does nothing harmful by continuing to run
    /// during process shutdown (the worker, if any, gets `kill_on_drop`'d anyway once the parent
    /// actually exits).
    pub async fn run_idle_reaper(self: Arc<Self>) {
        // A fixed fraction of the shortest possible idle timeout (`WorkerKind::Image`'s 30s),
        // not a fixed 30s poll — polling at the same cadence as the timeout itself would let a
        // worker sit idle for up to one full extra timeout period before this loop notices.
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            let mut guard = self.inner.lock().await;
            let Some(session) = guard.as_ref() else {
                continue;
            };
            if session.last_used.elapsed() >= self.kind.idle_timeout() {
                tracing::info!(
                    kind = ?self.kind,
                    idle_secs = session.last_used.elapsed().as_secs(),
                    "lanrurugi-gpu-worker idle past timeout, killing to reclaim VRAM"
                );
                if let Some(session) = guard.take() {
                    // Awaited with `guard` still held — same reasoning as `discard_session`'s own
                    // call site (see `kill_and_reap`'s own doc comment): a reader flipping to a new
                    // page moments after this idle kill fires must not be able to spawn a
                    // replacement worker before this one's VRAM is actually confirmed gone.
                    kill_and_reap(session.child).await;
                }
            }
        }
    }
}

fn rgb_image_to_raw(img: &image::RgbImage) -> RawRgbImage {
    RawRgbImage {
        width: img.width(),
        height: img.height(),
        rgb: img.as_raw().clone(),
    }
}

fn raw_to_rgb_image(raw: RawRgbImage) -> Result<image::RgbImage, String> {
    image::RgbImage::from_raw(raw.width, raw.height, raw.rgb)
        .ok_or_else(|| "worker returned a RawRgbImage whose byte length didn't match its own declared dimensions".to_string())
}

/// One entry per pixel of a `width` x `height` page is a precondition of every composite in this
/// module. Checked in two places for two different reasons: early in the reduced path so a
/// malformed mask never costs a worker round trip, and again inside
/// [`composite_restored_output`] so that function is safe to call on its own.
fn check_paste_mask_len(width: u32, height: u32, paste_mask: &[bool]) -> Result<(), String> {
    let expected = (width as usize) * (height as usize);
    if paste_mask.len() != expected {
        return Err(format!(
            "paste mask length {} does not match page dimensions {width}x{height}",
            paste_mask.len()
        ));
    }
    Ok(())
}

/// Rescales a working-resolution model output back to `full_page`'s dimensions and writes it into
/// `full_page` at exactly the pixels `paste_mask` covers, leaving every other pixel as it was.
///
/// Split out of [`GpuWorkerClient::erase_page_reduced`] so the invariant the whole feature's
/// equivalence argument rests on — that no pixel outside the paste mask is ever taken from the model
/// — is testable without a worker, a model, or a GPU. A model-gated version of that test would have
/// to build a CUDA session inside `cargo test`, and *dropping* one is a documented ~35-60% process
/// crash on this project's hardware, which is why real-model checks live in `examples/` instead.
fn composite_restored_output(
    full_page: &mut image::RgbImage,
    model_output_small: &image::RgbImage,
    paste_mask: &[bool],
) -> Result<(), String> {
    let (width, height) = full_page.dimensions();
    check_paste_mask_len(width, height, paste_mask)?;

    let restored = image::imageops::resize(
        model_output_small,
        width,
        height,
        image::imageops::FilterType::CatmullRom,
    );
    for y in 0..height {
        for x in 0..width {
            let i = (y as usize) * (width as usize) + (x as usize);
            if paste_mask[i] {
                full_page.put_pixel(x, y, *restored.get_pixel(x, y));
            }
        }
    }
    Ok(())
}

/// Bridges this type's own `async fn` methods (`recognize`/`erase_page`/`segment_bubbles` above)
/// to the synchronous handle traits (`lanrurugi_ocr::batch::TextRecognizerHandle`,
/// `lanrurugi_ocr::bubble_segment::BubbleSegmenterHandle`, `lanrurugi_inpaint::InpainterHandle`)
/// that `lanrurugi-ocr`/`lanrurugi-inpaint`'s own `rayon`-parallel/`spawn_blocking` call sites
/// expect — see those traits' own doc comments for why they're synchronous at all.
///
/// Uses `self.runtime` (captured once at construction time, always on the real Tokio runtime),
/// **not** `tokio::runtime::Handle::current()` — issue #104 found the real call site
/// (`lanrurugi_core::concurrency::parallel_map`, which `lanrurugi-ocr`'s batch recognition uses)
/// dispatches onto `rayon`'s own global thread pool, not a Tokio worker thread, so
/// `Handle::current()` panicked ("there is no reactor running") on every call. `Handle::block_on`
/// only needs a valid handle to *a* runtime — it works from any thread, Tokio-owned or not.
impl lanrurugi_ocr::batch::TextRecognizerHandle for GpuWorkerClient {
    fn recognize(
        &self,
        crop: &image::RgbImage,
    ) -> Result<String, lanrurugi_ocr::batch::RecognizeHandleError> {
        let raw = rgb_image_to_raw(crop);
        self.runtime
            .clone()
            .block_on(GpuWorkerClient::recognize(self, raw))
            .map_err(|e| e.classify_for_recognition())
    }

    fn recognize_batch(
        &self,
        crops: &[image::RgbImage],
    ) -> Vec<Result<String, lanrurugi_ocr::batch::RecognizeHandleError>> {
        let raws = crops.iter().map(rgb_image_to_raw).collect();
        self.runtime
            .clone()
            .block_on(GpuWorkerClient::recognize_batch(self, raws))
            .into_iter()
            .map(|result| result.map_err(|error| error.classify_for_recognition()))
            .collect()
    }
}

impl lanrurugi_inpaint::InpainterHandle for GpuWorkerClient {
    fn erase_page(
        &self,
        page: &image::RgbImage,
        model_mask: &[bool],
        paste_mask: &[bool],
    ) -> Result<image::RgbImage, String> {
        let raw_page = rgb_image_to_raw(page);
        let raw_model_mask = RawMask {
            width: page.width(),
            height: page.height(),
            mask: model_mask.to_vec(),
        };
        let raw_paste_mask = RawMask {
            width: page.width(),
            height: page.height(),
            mask: paste_mask.to_vec(),
        };
        let result = self
            .runtime
            .clone()
            .block_on(GpuWorkerClient::erase_page(
                self,
                raw_page,
                raw_model_mask,
                raw_paste_mask,
            ))
            .map_err(|e| e.to_string())?;
        raw_to_rgb_image(result)
    }
}

impl lanrurugi_ocr::bubble_segment::BubbleSegmenterHandle for GpuWorkerClient {
    fn detect(
        &self,
        page: &image::RgbImage,
    ) -> Result<Vec<lanrurugi_ocr::bubble_segment::DetectedBubble>, String> {
        let raw = rgb_image_to_raw(page);
        let wire = self
            .runtime
            .clone()
            .block_on(GpuWorkerClient::segment_bubbles(self, raw))
            .map_err(|e| e.to_string())?;
        Ok(wire
            .into_iter()
            .map(|b| lanrurugi_ocr::bubble_segment::DetectedBubble {
                bbox: lanrurugi_ocr::entities::BoundingBox::new(
                    b.bbox_x, b.bbox_y, b.bbox_w, b.bbox_h,
                ),
                confidence: b.confidence,
                mask: b.mask,
            })
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use lanrurugi_ocr::batch::RecognizeHandleError;

    /// Issue #101: every transport/lifecycle failure must classify as infrastructure, because in
    /// each of these the recognizer never reached a verdict about the crop — caching a page whose
    /// regions are missing for one of these reasons would freeze a degraded result forever.
    #[test]
    fn transport_and_lifecycle_failures_are_infrastructure() {
        let cases: Vec<GpuWorkerError> = vec![
            GpuWorkerError::Timeout(Duration::from_secs(60)),
            GpuWorkerError::NotReady(Duration::from_secs(30)),
            GpuWorkerError::Spawn(std::io::Error::other("spawn failed")),
            GpuWorkerError::Connect(std::io::Error::other("connection refused")),
            GpuWorkerError::BinaryNotFound,
            // The worker's CUDA session broke mid-call and the process exits right after — the
            // next call gets a fresh worker, so this is transient, not a verdict on the crop.
            GpuWorkerError::WorkerFatal("CUBLAS failure 3: the resource allocation failed".into()),
        ];

        for case in cases {
            assert!(
                matches!(
                    case.classify_for_recognition(),
                    RecognizeHandleError::Infrastructure(_)
                ),
                "{case:?} should classify as an infrastructure failure"
            );
        }
    }

    /// The opposite case: the worker answered about this specific crop with its own session
    /// healthy. Re-running would reach the same conclusion, so dropping the region is correct and
    /// the page's result stays cacheable.
    #[test]
    fn a_healthy_worker_verdict_is_a_content_rejection() {
        let err = GpuWorkerError::Worker("decode confidence 0.057 below minimum 0.5".to_string());
        match err.classify_for_recognition() {
            RecognizeHandleError::Content(msg) => {
                assert!(
                    msg.contains("decode confidence"),
                    "message preserved: {msg}"
                );
            }
            other => panic!("expected a content rejection, got {other:?}"),
        }
    }

    /// Real archives whose erase request used to exceed the boundary's message limit and were
    /// therefore never translated. These are the dimensions the feature exists for.
    const SURVEYED_OVER_LIMIT: &[(u32, u32)] = &[
        (6336, 9504),
        (5400, 8100),
        (9918, 4000),
        (6084, 3943),
        (4535, 6307),
        (4000, 6000),
        (3974, 5964),
    ];

    #[test]
    fn over_limit_pages_are_routed_through_the_reduced_path() {
        for &(w, h) in SURVEYED_OVER_LIMIT {
            match erase_plan(w, h) {
                ErasePlan::Reduced { width, height } => {
                    assert_eq!(
                        (width, height),
                        lanrurugi_inpaint::work_dims(w, h),
                        "{w}x{h} must be reduced by the worker's own rule, not a second one"
                    );
                    assert_eq!(
                        width.max(height),
                        lanrurugi_inpaint::MAX_INPAINT_SIZE,
                        "{w}x{h} reduced to {width}x{height}, whose longest edge is not the cap — \
                         the worker would reduce it a second time"
                    );
                }
                other => panic!("{w}x{h} should be reduced, got {other:?}"),
            }
        }
    }

    #[test]
    fn fitting_pages_keep_the_existing_path() {
        // A dense sweep across the whole fitting range, plus the pages that already worked and are
        // closest to the limit. A predicate that were merely *approximately* right would move one
        // of these onto the reduced path and silently change its output.
        for &(w, h) in &[
            (1u32, 1u32),
            (800, 1200),
            (1024, 1536),
            (1024, 1024),
            (2400, 3500),
            (2480, 3508),
            (3000, 4000),
            (1057, 1500),
            (2160, 3064),
        ] {
            assert_eq!(
                erase_plan(w, h),
                ErasePlan::FullResolution,
                "{w}x{h} should keep the pre-existing code path"
            );
        }

        // Sweep the band immediately below the limit, where the framing slack makes the decision
        // conservative. Every one of these must still take the existing path — the slack's cost is
        // a small band of page area, not a reclassification of pages that comfortably fit.
        let cap_pixels = (lanrurugi_gpu_ipc::MAX_FRAME_BYTES as u64
            - lanrurugi_gpu_ipc::ERASE_REQUEST_FRAMING_SLACK
            - 48)
            / 5;
        let side = (cap_pixels as f64).sqrt() as u32;
        assert!(
            side > 64,
            "the sweep assumes a side comfortably larger than its own range"
        );
        for delta in 0..64u32 {
            let (w, h) = (side - delta, side - delta);
            assert_eq!(
                erase_plan(w, h),
                ErasePlan::FullResolution,
                "{w}x{h} is below the limit and must not be reduced"
            );
        }
    }

    #[test]
    fn a_page_at_the_working_resolution_is_never_resampled() {
        // The plan's `CannotBound` arm must be unreachable for a real page, because a page at the
        // working resolution is a small fraction of the limit. If that ever stops being true, the
        // feature would start refusing pages that used to work.
        for &(w, h) in &[(1024u32, 1024u32), (688, 1024), (1, 1024), (1024, 1)] {
            assert_eq!(
                lanrurugi_inpaint::work_dims(w, h),
                (w, h),
                "{w}x{h} is at or below the working resolution and must not be reduced"
            );
            assert_eq!(erase_plan(w, h), ErasePlan::FullResolution);
        }
    }

    #[test]
    fn the_reduced_request_is_bounded_and_an_order_of_magnitude_smaller() {
        // SC-004, and the reason the transport question became moot: whatever the source page's
        // size, what crosses the boundary is bounded by the working resolution.
        for &(w, h) in SURVEYED_OVER_LIMIT {
            let ErasePlan::Reduced { width, height } = erase_plan(w, h) else {
                panic!("{w}x{h} should be reduced");
            };
            let full = lanrurugi_gpu_ipc::erase_request_bytes(w, h);
            let reduced = lanrurugi_gpu_ipc::erase_request_bytes(width, height);

            assert!(
                lanrurugi_gpu_ipc::erase_request_provably_fits(width, height),
                "{w}x{h} reduced to {width}x{height} still does not fit"
            );
            assert!(
                full >= reduced * 10,
                "{w}x{h}: {full} -> {reduced} bytes is not an order of magnitude"
            );
            assert!(
                reduced < 16 * 1024 * 1024,
                "{w}x{h} reduced request is {reduced} bytes, not a small constant"
            );
        }
    }

    #[test]
    fn a_size_limit_failure_is_distinguishable_from_a_model_failure() {
        // FR-013/FR-014: an operator must be able to tell a size problem from a model verdict
        // without reproducing it. Distinct by variant, and the message carries both numbers.
        let err = GpuWorkerError::PayloadTooLarge {
            attempted_bytes: 300_647_080,
            limit_bytes: 67_108_864,
        };
        let rendered = err.to_string();

        assert!(
            rendered.contains("300647080"),
            "message must name the attempted size: {rendered}"
        );
        assert!(
            rendered.contains("67108864"),
            "message must name the limit: {rendered}"
        );
        assert!(
            !matches!(
                err,
                GpuWorkerError::Worker(_) | GpuWorkerError::WorkerFatal(_)
            ),
            "a size failure must not be reported as a model failure"
        );
        assert!(
            !matches!(
                err.classify_for_recognition(),
                RecognizeHandleError::Content(_)
            ),
            "a size failure is not a content verdict, so a retry must not be cached as final"
        );
    }

    #[tokio::test]
    async fn the_reduced_path_rejects_a_mismatched_paste_mask() {
        // This path composites locally, so unlike the worker-side path nothing else would catch a
        // short mask before it is indexed. It must error rather than panic, and it must do so
        // *before* any worker is contacted — which is what makes this testable with no GPU, no
        // model, and no socket. Deliberately small dimensions: the guard runs before any
        // page-sized work, and the routing for real over-limit pages is covered separately by
        // `over_limit_pages_are_routed_through_the_reduced_path`.
        let client = GpuWorkerClient::new(WorkerKind::Image);

        let result = client
            .erase_page_reduced(
                RawRgbImage {
                    width: 64,
                    height: 64,
                    rgb: vec![0; 64 * 64 * 3],
                },
                RawMask {
                    width: 64,
                    height: 64,
                    mask: vec![false; 64 * 64],
                },
                RawMask {
                    width: 64,
                    height: 64,
                    mask: vec![false; 16],
                },
                32,
                32,
            )
            .await;

        match result {
            Err(GpuWorkerError::Worker(msg)) => assert!(
                msg.contains("paste mask length"),
                "expected a mask-length complaint, got: {msg}"
            ),
            other => panic!("expected a mask-length error, got {other:?}"),
        }
    }

    /// The single invariant the whole feature's "pages that already worked are unchanged" argument
    /// rests on: restoring the reduced model output may write *only* the pixels the original
    /// full-resolution paste mask covers. Every other pixel must still be the original page's own.
    ///
    /// Tested here, without a worker or a model, on the half of that invariant this feature actually
    /// authored. The other half — that `Inpainter::erase_page` itself writes only where its own
    /// paste mask is set — is pre-existing behaviour guarded by this crate's `examples/`, not by a
    /// test that would have to build and then drop a CUDA session inside `cargo test`.
    #[test]
    fn restoring_the_reduced_output_touches_only_paste_mask_pixels() {
        let (width, height) = (97u32, 61u32);
        let mut page = image::RgbImage::new(width, height);
        for (x, y, px) in page.enumerate_pixels_mut() {
            *px = image::Rgb([
                (x % 251) as u8,
                (y % 241) as u8,
                ((x * 7 + y * 13) % 239) as u8,
            ]);
        }
        let original = page.clone();

        // A working-resolution stand-in for the model's output, deliberately a different size so the
        // restore step is exercised rather than trivially identity.
        let mut small = image::RgbImage::new(31, 19);
        for (x, y, px) in small.enumerate_pixels_mut() {
            *px = image::Rgb([255, (x % 256) as u8, (y % 256) as u8]);
        }

        // Two disjoint patches, so the test covers both "inside the mask" and "outside it" in one
        // pass rather than relying on the mask being empty or full.
        let mut paste_mask = vec![false; (width as usize) * (height as usize)];
        let mut masked = Vec::new();
        for y in 10..20u32 {
            for x in 5..15u32 {
                paste_mask[(y as usize) * (width as usize) + (x as usize)] = true;
                masked.push((x, y));
            }
        }

        composite_restored_output(&mut page, &small, &paste_mask).unwrap();

        let mut masked_pixels_that_changed = 0usize;
        for y in 0..height {
            for x in 0..width {
                let i = (y as usize) * (width as usize) + (x as usize);
                let before = original.get_pixel(x, y);
                let after = page.get_pixel(x, y);
                if paste_mask[i] {
                    if after != before {
                        masked_pixels_that_changed += 1;
                    }
                } else if after != before {
                    // This is the requirement, stated as an assertion: outside the paste mask, the
                    // returned page must consist of the original page's own pixels.
                    panic!(
                        "pixel ({x}, {y}) is outside the paste mask but changed from {before:?} to \
                         {after:?}"
                    );
                }
            }
        }

        assert_eq!(
            masked_pixels_that_changed,
            masked.len(),
            "every masked pixel should have taken the restored output's value — if none did, the \
             loop above proves nothing about what happens *inside* the mask"
        );
    }

    #[test]
    fn restoring_the_reduced_output_rejects_a_mismatched_mask() {
        let mut page = image::RgbImage::new(16, 16);
        let small = image::RgbImage::new(8, 8);
        let err = composite_restored_output(&mut page, &small, &[false; 10]).unwrap_err();
        assert!(err.contains("paste mask length"), "got: {err}");
    }
}
