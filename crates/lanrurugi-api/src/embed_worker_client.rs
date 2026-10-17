//! Manages the `lanrurugi-embed-worker` subprocess and the `tarpc` connection to it.
//!
//! Real reported incident (2026-09-17): a user's own stated standard ("主进程内存必须峰值不超过
//! 256MB" — the main server process must never peak above 256MB resident) found `lanrurugi-server`
//! idling at ~732MB RSS with zero requests served, almost entirely attributable to the
//! recommendation embedding model's ONNX Runtime session (loaded process-lifetime, directly in
//! `lanrurugi-server`'s own startup task — see `recommend.rs`'s pre-refactor doc comment). This
//! module moves that model into its own subprocess, mirroring `gpu_worker_client.rs`'s own
//! established shape for the exact same class of problem (a real incident, 2026-09-13, that moved
//! `TextRecognizer`/`Inpainter`/`BubbleSegmenter` out for a different reason — CUDA's own
//! crash-on-drop risk).
//!
//! Deliberately simpler than `gpu_worker_client.rs` in three ways, all because this worker has no
//! CUDA/VRAM involvement at all (`ort::CPUExecutionProvider` only):
//! - No GPU vendor detection / `ORT_DYLIB_PATH` selection — always the CPU-only ONNX Runtime build.
//! - No `SIGKILL`-only reclaim / `VRAM_RECLAIM_SETTLE` — a graceful process exit has none of the
//!   GPU worker's own documented driver-crash-on-drop risk, so `discard_session`/the idle reaper
//!   just let the child exit normally (`kill_on_drop` still applies if the parent itself dies, and
//!   a normal `SIGTERM`-then-`SIGKILL`-if-needed teardown is enough otherwise).
//! - No `WorkerKind` split — there is only one model here, not three competing for one GPU's VRAM,
//!   so there is nothing to split across separate processes.

use std::process::Stdio;
use std::sync::Arc;
use std::time::{Duration, Instant};

use lanrurugi_embed_ipc::EmbedWorkerClient as RpcClient;
use tarpc::client;
use tarpc::tokio_serde::formats::Bincode;
use tokio::process::{Child, Command};
use tokio::sync::Mutex;

/// Every RPC call gets this long to complete before the caller gives up on the current worker.
/// A single `embed()` call is one small ONNX inference over a handful of tokens — no equivalent to
/// `gpu_worker_client::ERASE_PAGE_TIMEOUT`'s own whole-page-inpainting cost — so one shared timeout
/// for both `embed` and `ping` is enough, unlike the GPU worker's own per-call split.
const CALL_TIMEOUT: Duration = Duration::from_secs(30);

/// How long the worker process is allowed to take to accept its first connection after being
/// spawned — covers model download (first run only, cached after) + ONNX session construction +
/// tokenizer load. Not yet independently measured end-to-end against this exact subprocess
/// architecture (mirrors `gpu_worker_client::connect_with_retry`'s own historical caveat before its
/// timeout was tuned against a real measurement) — deliberately generous up front rather than
/// guessed tight, given this same session's own repeated real-world lesson that host memory
/// pressure can multiply a cold start's wall-clock time well past what an idle host would show.
const READY_TIMEOUT: Duration = Duration::from_secs(60);

/// How long a connection can sit unused before the worker holding it is asked to exit, reclaiming
/// its ~memory. Longer than any single request's own latency needs, short enough not to hold this
/// model's memory resident during genuine idle periods between recommendation-panel views —
/// recommendations are requested once per reader-session-boundary event, not on every page turn,
/// so idle gaps here are typically much longer than `gpu_worker_client::WorkerKind::Image`'s own
/// segment_bubbles-then-erase_page gap.
const IDLE_TIMEOUT: Duration = Duration::from_secs(5 * 60);

fn rpc_context(timeout: Duration) -> tarpc::context::Context {
    let mut ctx = tarpc::context::current();
    ctx.deadline = std::time::Instant::now() + timeout;
    ctx
}

#[derive(Debug, thiserror::Error)]
pub enum EmbedWorkerError {
    #[error("failed to spawn lanrurugi-embed-worker: {0}")]
    Spawn(std::io::Error),
    #[error("failed to connect to lanrurugi-embed-worker: {0}")]
    Connect(std::io::Error),
    #[error("lanrurugi-embed-worker did not become ready within {0:?}")]
    NotReady(Duration),
    #[error("RPC call to lanrurugi-embed-worker failed: {0}")]
    Rpc(#[from] tarpc::client::RpcError),
    #[error("lanrurugi-embed-worker call timed out after {0:?}")]
    Timeout(Duration),
    #[error("lanrurugi-embed-worker reported an error: {0}")]
    Worker(String),
    #[error("could not locate the lanrurugi-embed-worker binary next to the running executable")]
    BinaryNotFound,
}

struct Session {
    child: Child,
    client: RpcClient,
    last_used: Instant,
}

/// The single shared handle `AppState` hands out. Cloning is cheap (`Arc`-backed) — concurrent
/// callers share the same underlying worker process/connection rather than each spawning their
/// own. Only one instance ever exists per server process (unlike `GpuWorkerClient`, constructed
/// once per `WorkerKind` variant) since there's only one model to load.
#[derive(Clone)]
pub struct EmbedWorkerClient {
    inner: Arc<Mutex<Option<Session>>>,
    /// Serializes actual RPC dispatch — same reasoning as `gpu_worker_client::GpuWorkerClient`'s
    /// own `call_lock` doc comment: without it, concurrent callers (several readers' recommendation
    /// panels loading at once) could each spawn a competing worker, or race each other inside the
    /// same worker's own single ONNX session (already `Mutex`-wrapped inside `Embedder` itself, so
    /// they'd only queue there anyway — this lock just moves that queuing outside each call's own
    /// timeout window rather than eating into it).
    call_lock: Arc<Mutex<()>>,
    socket_path: String,
    models_dir: std::path::PathBuf,
}

impl std::fmt::Debug for EmbedWorkerClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmbedWorkerClient").finish_non_exhaustive()
    }
}

impl EmbedWorkerClient {
    /// `models_dir` mirrors `lanrurugi-server::main`'s own pre-refactor computation
    /// (`thumb_dir`'s parent, joined with `"models"`) — passed in here rather than recomputed, so
    /// this module doesn't need to know about `thumb_dir`/CLI args at all.
    pub fn new(models_dir: std::path::PathBuf) -> Self {
        let socket_path = std::env::var("LANRURUGI_EMBED_WORKER_SOCKET")
            .unwrap_or_else(|_| lanrurugi_embed_ipc::DEFAULT_SOCKET_PATH.to_string());
        Self {
            inner: Arc::new(Mutex::new(None)),
            call_lock: Arc::new(Mutex::new(())),
            socket_path,
            models_dir,
        }
    }

    /// Locates `lanrurugi-embed-worker` next to the currently running binary — same
    /// binary-relative-directory convention `gpu_worker_client.rs`'s own
    /// `worker_binary_path` uses.
    fn worker_binary_path() -> Result<std::path::PathBuf, EmbedWorkerError> {
        let exe = std::env::current_exe().map_err(|_| EmbedWorkerError::BinaryNotFound)?;
        let dir = exe.parent().ok_or(EmbedWorkerError::BinaryNotFound)?;
        let candidate = dir.join("lanrurugi-embed-worker");
        if candidate.is_file() {
            return Ok(candidate);
        }
        Err(EmbedWorkerError::BinaryNotFound)
    }

    async fn spawn_and_connect(&self) -> Result<Session, EmbedWorkerError> {
        let binary = Self::worker_binary_path()?;
        let _ = std::fs::remove_file(&self.socket_path);

        tracing::info!(path = %self.socket_path, "spawning lanrurugi-embed-worker");
        let child = Command::new(binary)
            .env("LANRURUGI_EMBED_WORKER_SOCKET", &self.socket_path)
            .env("LANRURUGI_EMBED_WORKER_MODELS_DIR", &self.models_dir)
            .stdin(Stdio::null())
            // Inherit stdout/stderr — same reasoning as `gpu_worker_client.rs`'s own spawn: no
            // separate log destination configured for this subprocess, its `tracing` output lands
            // in the same place as the parent server's own logs.
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .map_err(EmbedWorkerError::Spawn)?;

        let client = Self::connect_with_retry(&self.socket_path).await?;

        Ok(Session {
            child,
            client,
            last_used: Instant::now(),
        })
    }

    async fn connect_with_retry(socket_path: &str) -> Result<RpcClient, EmbedWorkerError> {
        const RETRY_INTERVAL: Duration = Duration::from_millis(100);
        let deadline = Instant::now() + READY_TIMEOUT;

        loop {
            match Self::try_connect(socket_path).await {
                Ok(client) => return Ok(client),
                Err(_) if Instant::now() < deadline => {
                    tokio::time::sleep(RETRY_INTERVAL).await;
                }
                Err(_) => return Err(EmbedWorkerError::NotReady(READY_TIMEOUT)),
            }
        }
    }

    async fn try_connect(socket_path: &str) -> Result<RpcClient, EmbedWorkerError> {
        let mut transport = tarpc::serde_transport::unix::connect(socket_path, Bincode::default);
        transport
            .config_mut()
            .max_frame_length(lanrurugi_embed_ipc::MAX_FRAME_BYTES);
        let transport = transport.await.map_err(EmbedWorkerError::Connect)?;
        let client = RpcClient::new(client::Config::default(), transport).spawn();
        client
            .ping(rpc_context(CALL_TIMEOUT))
            .await
            .map_err(|_| EmbedWorkerError::Connect(std::io::Error::other("ping failed")))?;
        Ok(client)
    }

    async fn ensure_session(&self) -> Result<RpcClient, EmbedWorkerError> {
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

    /// Marks the current session as unusable — used when an RPC call fails in a way that suggests
    /// the connection (or the worker itself) is no longer trustworthy. Unlike
    /// `gpu_worker_client::discard_session`, this doesn't need `kill_and_reap`'s own
    /// wait-for-actual-VRAM-reclaim step (there is no VRAM here) — `start_kill` plus a plain
    /// `wait()` to actually reap the process (avoiding a zombie, same `Child::start_kill` doc
    /// warning `gpu_worker_client.rs` cites) is enough.
    async fn discard_session(&self) {
        let mut guard = self.inner.lock().await;
        if let Some(mut session) = guard.take() {
            tracing::warn!("discarding lanrurugi-embed-worker session, killing the process");
            if let Err(e) = session.child.start_kill() {
                tracing::warn!(error = %e, "failed to send kill signal to lanrurugi-embed-worker (already exited?)");
            }
            let _ = session.child.wait().await;
        }
    }

    async fn call<T, F, Fut>(&self, f: F) -> Result<T, EmbedWorkerError>
    where
        F: FnOnce(RpcClient) -> Fut,
        Fut: std::future::Future<
            Output = Result<Result<T, lanrurugi_embed_ipc::WorkerError>, tarpc::client::RpcError>,
        >,
    {
        let _call_permit = self.call_lock.lock().await;
        let client = self.ensure_session().await?;
        match tokio::time::timeout(CALL_TIMEOUT, f(client)).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(lanrurugi_embed_ipc::WorkerError::Config(worker_error)))) => {
                Err(EmbedWorkerError::Worker(worker_error))
            }
            Ok(Ok(Err(lanrurugi_embed_ipc::WorkerError::Fatal(worker_error)))) => {
                self.discard_session().await;
                Err(EmbedWorkerError::Worker(worker_error))
            }
            Ok(Err(rpc_error)) => {
                self.discard_session().await;
                Err(EmbedWorkerError::Rpc(rpc_error))
            }
            Err(_elapsed) => {
                self.discard_session().await;
                Err(EmbedWorkerError::Timeout(CALL_TIMEOUT))
            }
        }
    }

    pub async fn embed(&self, text: &str) -> Result<Vec<f32>, EmbedWorkerError> {
        let text = text.to_string();
        self.call(|client| async move { client.embed(rpc_context(CALL_TIMEOUT), text).await })
            .await
    }

    /// Whether a worker is currently spawned and was, as of its last real use, reachable — used
    /// only for `health.rs`'s own informational reporting (same role
    /// `RecommendService::ready()` used to play against the in-process `Embedder`), never as a
    /// precondition to actually calling `embed` (that call spawns a worker on demand if none
    /// exists — this method must never be used to gate that).
    pub async fn is_connected(&self) -> bool {
        self.inner.lock().await.is_some()
    }

    /// Spawned once per `EmbedWorkerClient` at server startup — periodically exits the worker if
    /// it's been idle past [`IDLE_TIMEOUT`], reclaiming its memory. Mirrors
    /// `gpu_worker_client::GpuWorkerClient::run_idle_reaper`'s own shape, except taking `self` by
    /// value rather than `self: Arc<Self>` — `EmbedWorkerClient` is already `Arc`-backed
    /// internally (see this struct's own doc comment), unlike `GpuWorkerClient`, which its own
    /// call sites additionally wrap in an `Arc` themselves (`translation_pipeline.rs`'s
    /// `Arc::new(GpuWorkerClient::new(...))`); this type never needs that second layer.
    pub async fn run_idle_reaper(self) {
        let mut interval = tokio::time::interval(Duration::from_secs(10));
        loop {
            interval.tick().await;
            let mut guard = self.inner.lock().await;
            let Some(session) = guard.as_ref() else {
                continue;
            };
            if session.last_used.elapsed() >= IDLE_TIMEOUT {
                tracing::info!(
                    idle_secs = session.last_used.elapsed().as_secs(),
                    "lanrurugi-embed-worker idle past timeout, exiting to reclaim memory"
                );
                if let Some(mut session) = guard.take() {
                    if let Err(e) = session.child.start_kill() {
                        tracing::warn!(error = %e, "failed to send kill signal to lanrurugi-embed-worker (already exited?)");
                    }
                    let _ = session.child.wait().await;
                }
            }
        }
    }
}
