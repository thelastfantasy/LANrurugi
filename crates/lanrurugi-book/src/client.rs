//! Synchronous client: a small pool of `lanrurugi-book-worker` subprocesses, plus caches.
//!
//! Every book is parsed and rendered out of process: PDFium and DjVuLibre are C++ reading
//! untrusted files, and the text typesetter holds a font database the server's memory budget has
//! no room for. A malformed book that crashes or hangs an engine takes down one worker, never the
//! server. Each worker is connected over a socketpair (its stdin/stdout), which gives the client
//! real read timeouts; a worker that times out or breaks the protocol is killed, not reused.

use std::collections::HashMap;
use std::io;
use std::os::fd::OwnedFd;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::time::{Duration, Instant, UNIX_EPOCH};

use sha1::{Digest, Sha1};

use crate::format::BookFormat;
use crate::protocol::{self, BookInfo, ErrorKind, PageSource, Request, Response};

/// Width a rendered (non-passthrough) page is drawn at: about 200 DPI on A4/B5, sharp for text.
pub const DEFAULT_RENDER_WIDTH: u32 = 1800;

/// Generous: inspecting a long text book typesets all of it.
const INSPECT_TIMEOUT: Duration = Duration::from_secs(300);
const PAGE_TIMEOUT: Duration = Duration::from_secs(60);
/// Idle workers are reused only this soon after their last request; the worker exits by itself
/// after five idle minutes, and one picked up just before that would die mid-request.
const REUSE_WITHIN: Duration = Duration::from_secs(240);
const REAP_EVERY: Duration = Duration::from_secs(60);
/// Part of every cache key. Bump it whenever the worker would answer differently for the same
/// file (parsing, outline or typesetting changes), so stale pages and outlines are never served.
const CACHE_VERSION: u32 = 2;
/// In-memory inspection results kept per process before the map is dropped and refilled.
const MEMORY_CACHE_ENTRIES: usize = 512;

#[derive(Debug, thiserror::Error)]
pub enum BookError {
    #[error("not an e-book: {0}")]
    NotABook(String),
    #[error("this book is protected by DRM or a password")]
    Encrypted,
    #[error("unreadable book: {0}")]
    Unreadable(String),
    #[error("page {0} does not exist in this book")]
    PageOutOfRange(usize),
    #[error("book worker unavailable: {0}")]
    WorkerUnavailable(String),
    #[error("book worker failed: {0}")]
    Worker(String),
    #[error("I/O error: {0}")]
    Io(#[from] io::Error),
}

#[derive(Debug, Clone)]
pub struct BookConfig {
    pub worker_bin: Option<PathBuf>,
    /// PDFium shared library (file or directory) handed to workers as `LANRURUGI_PDFIUM_LIB`.
    pub pdfium_lib: Option<PathBuf>,
    /// Directory holding DjVuLibre's `djvused`/`ddjvu`, handed to workers as
    /// `LANRURUGI_DJVU_BIN_DIR` (otherwise they are looked up on `PATH`).
    pub djvu_bin_dir: Option<PathBuf>,
    pub cache_dir: PathBuf,
    pub max_workers: usize,
    pub render_width: u32,
}

impl Default for BookConfig {
    fn default() -> Self {
        let cache_dir = std::env::var_os("LANRURUGI_TEMP_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("book");
        Self {
            worker_bin: std::env::var_os("LANRURUGI_BOOK_WORKER_BIN").map(PathBuf::from),
            pdfium_lib: std::env::var_os("LANRURUGI_PDFIUM_LIB").map(PathBuf::from),
            djvu_bin_dir: std::env::var_os("LANRURUGI_DJVU_BIN_DIR").map(PathBuf::from),
            cache_dir,
            max_workers: default_max_workers(),
            render_width: DEFAULT_RENDER_WIDTH,
        }
    }
}

/// ~30% of cores, at least one (the project-wide cap for CPU-bound background work).
fn default_max_workers() -> usize {
    let cores = std::thread::available_parallelism().map_or(1, |n| n.get());
    ((cores as f64 * 0.3).floor() as usize).max(1)
}

static CONFIG: OnceLock<BookConfig> = OnceLock::new();
static POOL: OnceLock<Pool> = OnceLock::new();
static INFO_CACHE: OnceLock<Mutex<HashMap<String, Arc<BookInfo>>>> = OnceLock::new();

/// Overrides the defaults. Only the first call has any effect, and only if made before the
/// first book is touched; returns `false` otherwise.
pub fn configure(config: BookConfig) -> bool {
    CONFIG.set(config).is_ok()
}

fn config() -> &'static BookConfig {
    CONFIG.get_or_init(BookConfig::default)
}

fn pool() -> &'static Pool {
    POOL.get_or_init(|| {
        let _ = std::thread::Builder::new()
            .name("book-pool-reaper".into())
            .spawn(|| loop {
                std::thread::sleep(REAP_EVERY);
                if let Some(pool) = POOL.get() {
                    pool.reap();
                }
            });
        Pool::new(config().max_workers)
    })
}

/// Page sources, sizes, outline and metadata for `path`, cached by file identity (path, size,
/// mtime) in memory and on disk, so a restart does not re-read every book.
pub fn inspect(path: &Path) -> Result<Arc<BookInfo>, BookError> {
    let key = fingerprint(path)?;
    let memory = INFO_CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(info) = memory.lock().expect("book info cache poisoned").get(&key) {
        return Ok(info.clone());
    }

    let disk = config().cache_dir.join(&key).join("info.json");
    let info = match std::fs::read(&disk)
        .ok()
        .and_then(|b| serde_json::from_slice::<BookInfo>(&b).ok())
    {
        Some(info) => info,
        None => {
            let format = BookFormat::detect(path)
                .ok_or_else(|| BookError::NotABook(path.display().to_string()))?;
            let request = Request::Inspect {
                path: path_string(path)?,
                format,
            };
            let info = match pool().call(&request, INSPECT_TIMEOUT)? {
                (Response::Info { info }, _) => info,
                (other, _) => {
                    return Err(BookError::Worker(format!("unexpected response {other:?}")))
                }
            };
            if let Ok(bytes) = serde_json::to_vec(&info) {
                let _ = write_atomic(&disk, &bytes);
            }
            info
        }
    };

    let info = Arc::new(info);
    let mut map = memory.lock().expect("book info cache poisoned");
    if map.len() >= MEMORY_CACHE_ENTRIES {
        map.clear();
    }
    map.insert(key, info.clone());
    Ok(info)
}

/// The image bytes of page `index` (0-based). Rendered pages are cached on disk; pictures stored
/// in the book are cheap to pull again and are not duplicated into the cache.
pub fn read_page(path: &Path, index: usize) -> Result<Vec<u8>, BookError> {
    let info = inspect(path)?;
    let source = *info
        .pages
        .get(index)
        .ok_or(BookError::PageOutOfRange(index))?;
    let width = config().render_width;

    let cached = (source == PageSource::Render).then(|| {
        config()
            .cache_dir
            .join(fingerprint(path).unwrap_or_default())
            .join(format!("{index}-{width}.webp"))
    });
    if let Some(bytes) = cached.as_ref().and_then(|p| std::fs::read(p).ok()) {
        return Ok(bytes);
    }

    let request = Request::Page {
        path: path_string(path)?,
        format: info.format,
        index: index as u32,
        source,
        target_width: width,
    };
    let bytes = match pool().call(&request, PAGE_TIMEOUT)? {
        (Response::Page { .. }, Some(bytes)) => bytes,
        (other, _) => return Err(BookError::Worker(format!("unexpected response {other:?}"))),
    };
    if let Some(p) = cached {
        let _ = write_atomic(&p, &bytes);
    }
    Ok(bytes)
}

fn path_string(path: &Path) -> Result<String, BookError> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| BookError::Unreadable(format!("non-UTF-8 path {}", path.display())))
}

/// Identity of a file's current contents for cache keys: path, size and mtime (and
/// [`CACHE_VERSION`]).
fn fingerprint(path: &Path) -> Result<String, BookError> {
    let meta = std::fs::metadata(path)?;
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_nanos());
    let mut hasher = Sha1::new();
    hasher.update(CACHE_VERSION.to_le_bytes());
    hasher.update(path.as_os_str().as_encoded_bytes());
    hasher.update(meta.len().to_le_bytes());
    hasher.update(mtime.to_le_bytes());
    Ok(hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::other("no parent dir"))?;
    std::fs::create_dir_all(dir)?;
    let tmp = dir.join(format!(
        ".tmp-{}-{:?}",
        std::process::id(),
        std::thread::current().id()
    ));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

fn worker_binary() -> Result<PathBuf, BookError> {
    if let Some(bin) = &config().worker_bin {
        return Ok(bin.clone());
    }
    // Next to the running binary (the server image); one level up covers test binaries, which
    // run from `target/<profile>/deps/`.
    let exe = std::env::current_exe()?;
    for dir in exe.ancestors().skip(1).take(2) {
        let candidate = dir.join("lanrurugi-book-worker");
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    Err(BookError::WorkerUnavailable(
        "lanrurugi-book-worker binary not found next to this program".into(),
    ))
}

struct Worker {
    child: Child,
    stream: UnixStream,
    /// Workers exit by themselves after a few idle minutes; a reused one may already be gone.
    last_used: Instant,
}

impl Worker {
    fn spawn() -> Result<Self, BookError> {
        let (ours, theirs) = UnixStream::pair()?;
        let theirs: OwnedFd = theirs.into();
        let mut command = Command::new(worker_binary()?);
        if let Some(lib) = &config().pdfium_lib {
            command.env("LANRURUGI_PDFIUM_LIB", lib);
        }
        if let Some(dir) = &config().djvu_bin_dir {
            command.env("LANRURUGI_DJVU_BIN_DIR", dir);
        }
        let child = command
            .stdin(Stdio::from(theirs.try_clone()?))
            .stdout(Stdio::from(theirs))
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| BookError::WorkerUnavailable(e.to_string()))?;
        Ok(Self {
            child,
            stream: ours,
            last_used: Instant::now(),
        })
    }

    fn exchange(
        &mut self,
        request: &Request,
        timeout: Duration,
    ) -> io::Result<(Response, Option<Vec<u8>>)> {
        self.stream.set_write_timeout(Some(timeout))?;
        self.stream.set_read_timeout(Some(timeout))?;
        protocol::write_json(&mut self.stream, request)?;
        let response: Response = protocol::read_json(&mut self.stream)?;
        let payload = match response {
            Response::Page { .. } => Some(protocol::read_frame(&mut self.stream)?),
            _ => None,
        };
        self.last_used = Instant::now();
        Ok((response, payload))
    }
}

impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

struct Pool {
    state: Mutex<PoolState>,
    available: Condvar,
    max: usize,
}

struct PoolState {
    idle: Vec<Worker>,
    live: usize,
}

impl PoolState {
    /// Drops (killing and reaping) idle workers that have exited, or are about to exit on their own
    /// idle timeout and could die mid-request.
    fn retire_stale(&mut self) {
        let before = self.idle.len();
        self.idle.retain_mut(|worker| {
            matches!(worker.child.try_wait(), Ok(None)) && worker.last_used.elapsed() < REUSE_WITHIN
        });
        self.live -= before - self.idle.len();
    }
}

impl Pool {
    fn new(max: usize) -> Self {
        Self {
            state: Mutex::new(PoolState {
                idle: Vec::new(),
                live: 0,
            }),
            available: Condvar::new(),
            max: max.max(1),
        }
    }

    /// An idle worker, a freshly spawned one if under the cap, or the next one released.
    fn acquire(&self) -> Result<Worker, BookError> {
        let mut state = self.state.lock().expect("book pool poisoned");
        loop {
            state.retire_stale();
            if let Some(worker) = state.idle.pop() {
                return Ok(worker);
            }
            if state.live < self.max {
                state.live += 1;
                drop(state);
                return Worker::spawn().inspect_err(|_| self.forget());
            }
            state = self.available.wait(state).expect("book pool poisoned");
        }
    }

    /// Periodic [`PoolState::retire_stale`], so workers that exited on their own idle timeout are
    /// reaped rather than left as zombies until the next request.
    fn reap(&self) {
        let mut state = self.state.lock().expect("book pool poisoned");
        let live = state.live;
        state.retire_stale();
        if state.live < live {
            self.available.notify_all();
        }
    }

    fn release(&self, worker: Worker) {
        self.state
            .lock()
            .expect("book pool poisoned")
            .idle
            .push(worker);
        self.available.notify_one();
    }

    /// A worker was discarded (or never started): free its slot.
    fn forget(&self) {
        self.state.lock().expect("book pool poisoned").live -= 1;
        self.available.notify_one();
    }

    fn call(
        &self,
        request: &Request,
        timeout: Duration,
    ) -> Result<(Response, Option<Vec<u8>>), BookError> {
        let mut retried = false;
        loop {
            let mut worker = self.acquire()?;
            let was_idle = worker.last_used.elapsed() > Duration::from_secs(60);
            match worker.exchange(request, timeout) {
                Ok((Response::Error { kind, message }, _)) => {
                    self.release(worker);
                    return Err(match kind {
                        ErrorKind::Encrypted => BookError::Encrypted,
                        ErrorKind::Unreadable => BookError::Unreadable(message),
                        ErrorKind::PageOutOfRange => BookError::PageOutOfRange(match request {
                            Request::Page { index, .. } => *index as usize,
                            Request::Inspect { .. } => 0,
                        }),
                        ErrorKind::Internal => BookError::Worker(message),
                    });
                }
                Ok(ok) => {
                    self.release(worker);
                    return Ok(ok);
                }
                Err(e) => {
                    drop(worker);
                    self.forget();
                    // A worker that sat idle may simply have exited on its own timeout: retry
                    // once on a fresh one. A timeout or crash mid-request is reported instead.
                    if was_idle
                        && !retried
                        && e.kind() != io::ErrorKind::WouldBlock
                        && e.kind() != io::ErrorKind::TimedOut
                    {
                        retried = true;
                        continue;
                    }
                    tracing::warn!(error = %e, ?request, "book worker failed; discarded");
                    return Err(BookError::Worker(e.to_string()));
                }
            }
        }
    }
}
