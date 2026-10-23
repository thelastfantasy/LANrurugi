//! Embedding inference worker — a subprocess spawned by `lanrurugi-api::embed_worker_client`
//! (never by a user directly) that owns the recommendation embedding model's ONNX Runtime session.
//! See `lanrurugi-embed-ipc`'s own top-level doc comment for why this subprocess boundary exists.
//!
//! Unlike `lanrurugi-gpu-worker`, this process has no CUDA-crash-on-drop reason to avoid a normal
//! graceful shutdown — the embedding model runs on `ort::CPUExecutionProvider` only — so it simply
//! exits when idle past its own timeout (the client-side `run_idle_reaper` sends it a normal kill,
//! no `SIGKILL`-only requirement).

use std::path::PathBuf;
use std::sync::Arc;

use futures::{future, StreamExt};
use lanrurugi_embed_ipc::{EmbedWorker, WorkerError};
use lanrurugi_recommend::embedding::Embedder;
use tarpc::context::Context;
use tarpc::server::{self, Channel};
use tarpc::tokio_serde::formats::Bincode;

/// Same CPU-thread budget knob `lanrurugi-server`'s own startup task used to pass to
/// `Embedder::load` directly — the worker takes over exactly that loading, so it should behave
/// identically resource-wise. Fixed at `1` here (not `precompute_worker_budget()`, which the old
/// in-process startup task used): that budget calculation lived in `lanrurugi-api`, which this
/// worker crate deliberately does not depend on (same reasoning as `lanrurugi-gpu-worker` not
/// depending on `lanrurugi-api` — see this workspace's own dependency direction convention), and
/// the live-request embed path (`recommend.rs`) already only ever passed a small fixed value
/// itself; only the batch precompute job wanted the larger budget, and that job now calls through
/// this same one worker's own request queue rather than needing a second dedicated thread count.
const INTRA_THREADS: usize = 1;

#[derive(Debug, thiserror::Error)]
enum StartupError {
    #[error("LANRURUGI_EMBED_WORKER_MODELS_DIR is not set")]
    MissingModelsDir,
    #[error("LANRURUGI_EMBED_WORKER_SOCKET is not set")]
    MissingSocketPath,
    #[error(transparent)]
    Download(#[from] lanrurugi_recommend::model_download::ModelDownloadError),
    #[error(transparent)]
    Embedding(#[from] lanrurugi_recommend::embedding::EmbeddingError),
}

#[derive(Clone)]
struct Worker {
    embedder: Arc<Embedder>,
}

impl EmbedWorker for Worker {
    // `Embedder::embed` calls `ort::Session::run` synchronously, blocking the calling thread for
    // the full inference duration — same reasoning as `lanrurugi-gpu-worker`'s own `recognize`/
    // `erase_page`/`segment_bubbles` handlers: calling it directly in this `async fn` would starve
    // this runtime's reactor (a real `erase_page` call there was confirmed live to never yield the
    // thread back, going silent on every other in-flight request including `ping` until the
    // client's deadline killed the connection). `spawn_blocking` moves it to tokio's separate
    // blocking-task pool instead.
    async fn embed(self, _: Context, text: String) -> Result<Vec<f32>, WorkerError> {
        let embedder = self.embedder.clone();
        tokio::task::spawn_blocking(move || embedder.embed(&text))
            .await
            .map_err(|e| WorkerError::Fatal(format!("blocking task failed: {e}")))?
            .map_err(|e| WorkerError::Config(e.to_string()))
    }

    async fn ping(self, _: Context) {}
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    tracing_subscriber::fmt::init();

    let socket_path = std::env::var("LANRURUGI_EMBED_WORKER_SOCKET")
        .map_err(|_| StartupError::MissingSocketPath)?;
    let models_dir: PathBuf = std::env::var("LANRURUGI_EMBED_WORKER_MODELS_DIR")
        .map_err(|_| StartupError::MissingModelsDir)?
        .into();

    // Same reasoning as `lanrurugi-gpu-worker::main` — a leftover socket file from a worker that
    // exited uncleanly would otherwise make `listen` below fail with "address already in use";
    // removing it first is safe because this process is only ever started by
    // `embed_worker_client` after it has already confirmed no live worker is listening there.
    let _ = std::fs::remove_file(&socket_path);

    tracing::info!("acquiring embedding model");
    let (model_path, tokenizer_path) =
        lanrurugi_recommend::model_download::acquire_models(&models_dir)
            .await
            .map_err(StartupError::Download)?;
    tracing::info!("loading embedding model");
    let embedder = Embedder::load(&model_path, &tokenizer_path, INTRA_THREADS)
        .map_err(StartupError::Embedding)?;
    tracing::info!("model loaded, listening");
    let worker = Worker {
        embedder: Arc::new(embedder),
    };

    let mut listener = tarpc::serde_transport::unix::listen(&socket_path, Bincode::default).await?;
    listener
        .config_mut()
        .max_frame_length(lanrurugi_embed_ipc::MAX_FRAME_BYTES);
    listener
        .filter_map(|r| future::ready(r.ok()))
        .map(server::BaseChannel::with_defaults)
        .map(|channel| {
            let worker = worker.clone();
            channel.execute(worker.serve()).for_each(spawn)
        })
        // Serve every incoming connection concurrently — same shape `lanrurugi-gpu-worker::main`
        // uses; this worker only ever expects one client (the parent server process) but a fresh
        // connection per pooled client-side connection is still normal, not an error case.
        .buffer_unordered(16)
        .for_each(|()| async {})
        .await;

    Ok(())
}

async fn spawn(fut: impl std::future::Future<Output = ()> + Send + 'static) {
    tokio::spawn(fut);
}
