//! Deno subprocess worker-pool manager (constitution Principle IV, research.md §7).
//!
//! Each plugin namespace gets its **own** persistent Deno subprocess, started with exactly that
//! plugin's declared permissions (never a shared process across plugins, which would otherwise
//! grant every plugin the union of every other enabled plugin's permissions — a Principle IV
//! violation). Starting a plugin's worker is therefore two-phase:
//! 1. Query `plugin_info` from a throwaway, **zero-permission** subprocess (the host can't know
//!    what to grant until it asks).
//! 2. Spawn the real, persistent worker with exactly the permissions that response declared.
//!
//! A failed or timed-out request kills and drops that one plugin's worker (it's respawned lazily
//! on next use) without touching any other in-flight request or plugin (FR-013 — failure
//! isolation).

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{oneshot, Mutex};
use uuid::Uuid;

use crate::http::{HostHttp, RelayRequest};
use crate::permissions::build_flags;
use crate::protocol::{
    DiscoveryResult, HostHttpCall, HostHttpResult, PluginInfo, PluginIntrospection,
    PluginOptionsResult, Request, Response,
};

/// `dispatcher.ts`'s `canonicalize_source` reply shape.
#[derive(serde::Deserialize)]
struct CanonicalizeSourceResult {
    canonical: Vec<String>,
}

const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, thiserror::Error)]
pub enum PoolError {
    #[error("failed to spawn Deno subprocess: {0}")]
    Spawn(#[from] std::io::Error),
    #[error("plugin worker did not respond within {0:?} (timeout)")]
    Timeout(Duration),
    #[error("plugin returned an error: {}", .0.message)]
    PluginError(crate::protocol::ResponseError),
    #[error("plugin worker exited unexpectedly")]
    WorkerGone,
    #[error("malformed response from plugin worker: {0}")]
    Malformed(#[from] serde_json::Error),
    #[error("no plugin installed under namespace {0:?}")]
    NotFound(String),
}

type Result<T> = std::result::Result<T, PoolError>;

/// `namespace` ultimately gets `format!("{namespace}.ts")`-ed and joined onto `plugins_dir` to
/// locate the plugin's `.ts` file — but `plugin_info`/`execute` take it straight from an
/// unauthenticated-by-namespace HTTP query parameter (`POST /plugins/use`'s `plugin` param,
/// `crates/lanrurugi-api/src/plugins.rs`), not from `discover_namespaces`'s own directory listing.
/// Without this check, `PathBuf::join` treats a leading `/` in `namespace` as an **absolute**
/// path and discards `plugins_dir` entirely (Rust std behavior), and `..` components climb out of
/// it — either way letting a caller point the host at an arbitrary `.ts` file on disk, which would
/// then run with whatever permissions *that file's own* `plugin_info` response declares.
///
/// Namespaces may now contain subdirectory components (`metadata/ehentai`, `custom/foo/bar`) —
/// plugins are organized under `plugins_dir` by category (`metadata/`, `login/`, `download/`,
/// `script/`) plus a `custom/` tree for uploaded ones — so this only rejects the two genuinely
/// unsafe shapes (absolute paths, `..` traversal), not multi-component paths. Every component
/// must still be `Normal` (a plain name, not `.`/`..`/a root), which is exactly what
/// `discover_namespaces`'s own recursive walk (`file_stem()`-based, relative to `plugins_dir`)
/// always produces.
fn is_safe_namespace(namespace: &str) -> bool {
    !namespace.is_empty()
        && Path::new(namespace)
            .components()
            .all(|c| matches!(c, std::path::Component::Normal(_)))
}

/// What one worker may relay to the network: the shared relay plus the hosts *its* plugin declared
/// (`declared_permissions.net`). Bundled rather than threaded as two more arguments, and so an
/// introspection worker — which gets no permission flags at all — visibly gets no hosts either.
#[derive(Clone)]
pub struct RelayAccess {
    pub http: Arc<HostHttp>,
    pub allowed_hosts: Vec<String>,
}

impl RelayAccess {
    /// Introspection (`plugin_info`/`plugin_options`) runs with zero permissions: a plugin has no
    /// business fetching anything just to describe itself, exactly as before this was relayed.
    fn none(http: Arc<HostHttp>) -> Self {
        Self {
            http,
            allowed_hosts: Vec::new(),
        }
    }
}

/// A plugin `.ts` file's current `mtime` — used both to stamp a freshly spawned [`Worker`] and,
/// on every later [`PluginPool::execute`] call, to detect whether the file has changed since that
/// worker's own `import()` last read it.
fn plugin_mtime(plugin_module: &Path) -> Result<SystemTime> {
    Ok(std::fs::metadata(plugin_module)?.modified()?)
}

/// How long a descriptor entry may be reused even when the file still carries the same stamp.
///
/// Purely a memory backstop now: [`PluginStamp`] already sees an in-place replacement (it reads
/// `ctime` and `inode`, neither of which `cp -p`/`rsync -a`/untarring can preserve), so this no
/// longer decides correctness — ten minutes keeps an entry warm across an idle stretch.
const INFO_CACHE_TTL: Duration = Duration::from_secs(600);

/// How many throwaway dispatchers may exist at once — the ones that answer "what does this plugin
/// declare?" and then exit.
///
/// Persistent per-plugin workers are deliberately **not** capped: each one is already shared by
/// every concurrent call into that plugin, and evicting one to make room would mean killing a
/// process that a download may be talking to. Only the transient fan-out needs a gate, and a
/// halved-core number (2–8) is what the machine can afford rather than something to tune: a Deno
/// dispatcher costs ~14 ms to boot and tens of MB resident, so a cold library-wide discover would
/// otherwise start one process per installed plugin at the same moment.
pub fn worker_limit() -> usize {
    let cores = std::thread::available_parallelism()
        .map(|cores| cores.get())
        .unwrap_or(4);
    (cores / 2).clamp(2, 8)
}

/// A plugin's own declaration, cached per namespace (029 US1, `contracts/plugin-descriptor-cache.md`).
///
/// Only the *declaration* is cached: type, parameters, the permissions it declares. Nothing here
/// decides what a real load is granted — that still comes from the freshly fetched entry, and the
/// entry itself is only ever read to hand those flags to `Worker::spawn` (P3/V4).
#[derive(Clone)]
struct CachedPluginInfo {
    /// Stamped at fetch time from the file itself, never from a caller-supplied value (V1).
    stamp: PluginStamp,
    info: PluginInfo,
    /// `Some(None)` means "asked, the plugin exports no `pluginOptions()`"; `None` means this
    /// entry has never been asked yet.
    options: Option<Option<PluginOptionsResult>>,
    fetched_at: Instant,
}

impl CachedPluginInfo {
    /// P1/P6: still describes the same file version, and has not aged out.
    fn is_current(&self, stamp: PluginStamp) -> bool {
        self.stamp == stamp && self.fetched_at.elapsed() < INFO_CACHE_TTL
    }
}

/// A plugin file's identity for cache purposes.
///
/// `mtime`+size alone cannot see an in-place replacement that preserves timestamps (`cp -p`,
/// `rsync -a`, untarring a backup over the file). `ctime` cannot be set from userspace and `inode`
/// changes on a write-then-rename swap, so reading them closes that hole at no extra IO — the same
/// `stat` already in hand returns both.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct PluginStamp {
    modified: SystemTime,
    size: u64,
    /// Unix `ctime` as (seconds, nanoseconds); `(0, 0)` where the platform has no such field.
    changed: (i64, i64),
    inode: u64,
}

impl PluginStamp {
    fn of(meta: &std::fs::Metadata) -> std::io::Result<Self> {
        let modified = meta.modified()?;
        let size = meta.len();
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            Ok(Self {
                modified,
                size,
                changed: (meta.ctime(), meta.ctime_nsec()),
                inode: meta.ino(),
            })
        }
        #[cfg(not(unix))]
        {
            Ok(Self {
                modified,
                size,
                changed: (0, 0),
                inode: 0,
            })
        }
    }
}

struct Worker {
    /// Shared with the reader task, which writes relay answers back to the same stream.
    stdin: Arc<Mutex<ChildStdin>>,
    pending: Arc<Mutex<HashMap<String, oneshot::Sender<Response>>>>,
    _child: Child,
    /// The plugin file's `mtime` when this worker last `import()`ed it — `execute` respawns if
    /// the file has since changed, since a running worker never reloads on its own.
    mtime: std::time::SystemTime,
}

impl Worker {
    /// The OS process this worker is. Exposed so the resource monitor can attribute RSS and CPU to
    /// a plugin namespace (030 FR-018) without reaching into the child handle.
    fn pid(&self) -> Option<u32> {
        self._child.id()
    }
}

impl Worker {
    async fn spawn(
        deno_binary: &str,
        dispatcher_path: &Path,
        plugins_dir: &Path,
        namespace: &str,
        extra_flags: &[String],
        declared_read: bool,
        relay: RelayAccess,
    ) -> Result<Self> {
        // Defence in depth: every current caller validates the namespace before getting here, but
        // `spawn` is where the joins below actually happen, so the guard belongs here too rather
        // than only at the entry points. `..`/absolute namespaces would otherwise escape
        // `plugins_dir` through `Path::join`.
        if !is_safe_namespace(namespace) {
            return Err(PoolError::NotFound(namespace.to_string()));
        }
        let plugin_module = plugins_dir.join(format!("{namespace}.ts"));
        // Read before spawning: worst case is one extra harmless respawn later, never a missed edit.
        let mtime = plugin_mtime(&plugin_module)?;
        // `plugin-sdk.ts` is written out as `dispatcher_path`'s own sibling (every
        // `lanrurugi_plugin::DISPATCHER_SCRIPT` write site has a matching
        // `lanrurugi_plugin::PLUGIN_SDK_SCRIPT` write right next to it — see those sites' own
        // comments), so this is a mechanical path join, not a lookup of anything Deno-side.
        let plugin_sdk_path = dispatcher_path
            .parent()
            .expect("dispatcher_path always has a parent directory")
            .join("plugin-sdk.ts");
        // Deno must read the dispatcher + SDK + plugin `.ts` files to `import()` them at all — a
        // mechanical requirement, not a capability grant — so this baseline is always present,
        // scoped to just those files unless the plugin itself declared broader read access.
        // `plugin-sdk.ts` is read by `dispatcher.ts` itself (a real `import` — see that file's own
        // top-of-file comment for why `PluginErrorException` needs one there but a plugin file
        // itself does not), so this worker process — which *is* the dispatcher process — needs the
        // grant even though the plugin module it later `import()`s never touches that path itself.
        let read_flag = if declared_read {
            "--allow-read".to_string()
        } else {
            format!(
                "--allow-read={},{},{}",
                dispatcher_path.display(),
                plugin_sdk_path.display(),
                plugin_module.display()
            )
        };

        let mut cmd = tokio::process::Command::new(deno_binary);
        cmd.arg("run").arg("--quiet").arg(read_flag);
        for flag in extra_flags {
            cmd.arg(flag);
        }
        // The dispatcher decides between Deno's native fetch and the host relay by reading
        // `LANRURUGI_EGRESS_ACTIVE`, which only this shim sets — hence the two-name env grant.
        cmd.arg("--allow-env=LANRURUGI_EGRESS_ACTIVE,LANRURUGI_EGRESS_IF");
        // Only when an egress interface is chosen: the shim switches the dispatcher to Deno's own
        // fetch, which drops the host relay's per-host concurrency caps, request merging and
        // timeouts. With no interface chosen those protections stay on.
        if let Some(interface) = lanrurugi_core::http_client::outbound_interface() {
            if let Some(library) = crate::egress::library() {
                cmd.env(
                    "LD_PRELOAD",
                    crate::egress::preload_value(
                        &library,
                        std::env::var_os("LD_PRELOAD").as_deref(),
                    ),
                );
                cmd.env("LANRURUGI_EGRESS_IF", interface);
            }
        }
        cmd.arg(dispatcher_path)
            .arg(plugins_dir)
            .arg(namespace)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true);

        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");

        let pending: Arc<Mutex<HashMap<String, oneshot::Sender<Response>>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let pending_for_reader = pending.clone();

        let stdin = Arc::new(Mutex::new(stdin));
        let stdin_for_reader = stdin.clone();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                let Ok(value) = serde_json::from_str::<serde_json::Value>(&line) else {
                    continue;
                };
                // A plugin's own `fetch()` arrives on the same stream as its results; serving it in
                // its own task (not inline) is what keeps a slow upstream request from blocking the
                // plugin's other, unrelated calls.
                if value.get("type").and_then(|t| t.as_str()) == Some("host_http") {
                    let Ok(call) = serde_json::from_value::<HostHttpCall>(value) else {
                        continue;
                    };
                    let http = relay.http.clone();
                    let allowed = relay.allowed_hosts.clone();
                    let stdin = stdin_for_reader.clone();
                    tokio::spawn(async move {
                        let request = RelayRequest {
                            method: call.method,
                            url: call.url,
                            headers: call.headers,
                            body: call.body,
                            redirect_limit: call.redirect_limit,
                        };
                        let result = match http.relay(&allowed, request).await {
                            Ok(response) => HostHttpResult {
                                line_type: "host_http_result",
                                call_id: call.call_id,
                                ok: true,
                                status: Some(response.status),
                                headers: Some(response.headers),
                                url: Some(response.url),
                                body: Some(response.body),
                                error: None,
                            },
                            Err(error) => HostHttpResult {
                                line_type: "host_http_result",
                                call_id: call.call_id,
                                ok: false,
                                status: None,
                                headers: None,
                                url: None,
                                body: None,
                                error: Some(error),
                            },
                        };
                        if let Ok(mut line) = serde_json::to_string(&result) {
                            line.push('\n');
                            let _ = stdin.lock().await.write_all(line.as_bytes()).await;
                        }
                    });
                    continue;
                }
                let Ok(response) = serde_json::from_value::<Response>(value) else {
                    continue;
                };
                if let Some(sender) = pending_for_reader.lock().await.remove(&response.request_id) {
                    let _ = sender.send(response);
                }
            }
            // Reader loop ended (process exited): wake any still-pending callers with nothing —
            // their `oneshot::Receiver` will just observe the sender dropped.
        });

        Ok(Self {
            stdin,
            pending,
            _child: child,
            mtime,
        })
    }

    async fn call(
        &mut self,
        plugin: &str,
        method: &str,
        args: serde_json::Value,
    ) -> Result<Response> {
        let request_id = Uuid::new_v4().to_string();
        let request = Request {
            request_id: request_id.clone(),
            plugin: plugin.to_string(),
            method: method.to_string(),
            args,
        };

        let (tx, rx) = oneshot::channel();
        self.pending.lock().await.insert(request_id.clone(), tx);

        let mut line = serde_json::to_string(&request)?;
        line.push('\n');
        self.stdin.lock().await.write_all(line.as_bytes()).await?;

        match tokio::time::timeout(DEFAULT_TIMEOUT, rx).await {
            Ok(Ok(response)) => Ok(response),
            Ok(Err(_)) => Err(PoolError::WorkerGone),
            Err(_) => {
                self.pending.lock().await.remove(&request_id);
                Err(PoolError::Timeout(DEFAULT_TIMEOUT))
            }
        }
    }
}

/// A throwaway worker (declaration, introspection) and the slot it holds while alive.
///
/// Dropping the wrapper drops the worker — `kill_on_drop` ends the process — and returns the slot,
/// which is what keeps a library-wide discover from booting every plugin at once.
struct TransientWorker {
    worker: Worker,
    _slot: tokio::sync::OwnedSemaphorePermit,
}

/// The persistent worker for one namespace, behind its own lock.
///
/// `worker` is `None` before the first call, after the plugin file changed, and after a failed
/// call — in all three cases the next call spawns a fresh process while holding this slot's lock,
/// so a cold start of one plugin never blocks a call into another.
struct WorkerSlot {
    worker: Option<Worker>,
}

/// Per-namespace call counters, kept outside the slot's lock so the resource monitor can read them
/// while a call is in flight (030: a busy plugin must still report its history, not zeros).
#[derive(Default)]
struct CallCounters {
    calls: AtomicU64,
    failures: AtomicU64,
    /// `0` means "never called" — the wire shape wants `null`, not epoch.
    last_call_ms: AtomicI64,
}

/// One persistent worker as the resource monitor sees it (030 FR-018).
#[derive(Debug, Clone)]
pub struct WorkerProcess {
    pub namespace: String,
    /// `None` exactly when `busy` — see [`PluginPool::worker_processes`].
    pub pid: Option<u32>,
    pub busy: bool,
    pub calls: u64,
    pub failures: u64,
    pub last_call_at: Option<i64>,
}

pub struct PluginPool {
    deno_binary: String,
    dispatcher_path: PathBuf,
    plugins_dir: PathBuf,
    /// One lock per namespace, so a call into plugin A never waits behind plugin B's network I/O
    /// (a metadata fetch can take seconds). Calls into the *same* plugin still serialize — that is
    /// what keeps it at one process — but the map lock itself is only taken to find the slot.
    workers: Arc<Mutex<HashMap<String, Arc<tokio::sync::Mutex<WorkerSlot>>>>>,

    /// Slots for throwaway declaration/introspection workers, so discovering a whole library at
    /// once cannot boot more Deno processes than [`worker_limit`] at a time.
    transient_slots: Arc<tokio::sync::Semaphore>,
    /// Last declaration fetched per namespace, keyed by the file version it came from (029 US1).
    info_cache: Mutex<HashMap<String, CachedPluginInfo>>,
    /// One lock per namespace, so concurrent discovers of the same plugin coalesce into a single
    /// real load instead of N (contract P4): the losers wake up, re-check the cache and read the
    /// winner's entry.
    inflight: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Shared by every worker this pool spawns: one revalidation store and one coalescing map for
    /// the whole process, not one per plugin namespace.
    http: Arc<HostHttp>,

    /// Per-namespace call counters for the resource monitor (030). Separate from `workers` so a
    /// namespace keeps its history even while its slot is locked by an in-flight call.
    counters: Mutex<HashMap<String, Arc<CallCounters>>>,
    /// How many throwaway declaration processes have been started since boot, and how many slots
    /// exist — the two numbers that show the 029 cap doing its job.
    transient_limit: usize,
    transient_spawned: AtomicU64,
    /// Descriptor cache hit/miss counters (029 P6); read by the resource monitor's `caches` section.
    info_hits: AtomicU64,
    info_misses: AtomicU64,
}

impl PluginPool {
    pub fn new(
        deno_binary: impl Into<String>,
        dispatcher_path: PathBuf,
        plugins_dir: PathBuf,
    ) -> Self {
        Self::with_worker_limit(deno_binary, dispatcher_path, plugins_dir, worker_limit())
    }

    /// Same pool with an explicit process cap — only the tests need to pin it.
    fn with_worker_limit(
        deno_binary: impl Into<String>,
        dispatcher_path: PathBuf,
        plugins_dir: PathBuf,
        limit: usize,
    ) -> Self {
        Self {
            deno_binary: deno_binary.into(),
            dispatcher_path,
            plugins_dir,
            workers: Arc::new(Mutex::new(HashMap::new())),
            transient_slots: Arc::new(tokio::sync::Semaphore::new(limit)),
            info_cache: Mutex::new(HashMap::new()),
            inflight: Mutex::new(HashMap::new()),
            http: Arc::new(HostHttp::new()),
            counters: Mutex::new(HashMap::new()),
            transient_limit: limit,
            transient_spawned: AtomicU64::new(0),
            info_hits: AtomicU64::new(0),
            info_misses: AtomicU64::new(0),
        }
    }

    /// Drops every persistent dispatcher, so the next call spawns fresh ones.
    ///
    /// Needed when something the *spawn* reads changes — the plugin egress interface travels as an
    /// environment variable (and the preloaded shim reads it once), so a dispatcher started before
    /// the switch would keep the old binding forever. Dropping a [`Worker`] kills its child
    /// (`kill_on_drop`), which is all a fresh spawn needs.
    pub async fn restart_workers(&self) {
        // 022 US4: clearing the map drops each worker, and `kill_on_drop` then kills a subprocess
        // that may be in the middle of a login, a metadata fetch or `execDownload`. New calls
        // already go to a freshly spawned worker; the old ones only have to be stopped once their
        // current call answers, so an interface change no longer fails work in flight.
        let drained: Vec<Arc<tokio::sync::Mutex<WorkerSlot>>> = {
            let mut map = self.workers.lock().await;
            map.drain().map(|(_, slot)| slot).collect()
        };
        // The counters describe calls into the processes being stopped, so they go with them.
        self.counters.lock().await.clear();
        if drained.is_empty() {
            return;
        }
        tokio::spawn(async move {
            let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
            let mut left = drained;
            while !left.is_empty() && std::time::Instant::now() < deadline {
                tokio::time::sleep(std::time::Duration::from_secs(2)).await;
                // Taking a drained slot's own lock proves no call is in flight on it (a call holds
                // that lock from spawn through answer), so its process can be stopped now.
                left.retain(|slot| match slot.try_lock() {
                    Ok(mut slot) => {
                        slot.worker = None;
                        false
                    }
                    Err(_) => true,
                });
            }
            if !left.is_empty() {
                tracing::warn!(
                    workers = left.len(),
                    "plugin workers still busy after ten minutes; stopping them anyway"
                );
            }
        });
    }

    /// The configured `deno` binary path/name (whatever was passed to `new` — a bare `"deno"`
    /// resolved via `PATH` in most deployments, but always the *same* one this pool spawns every
    /// worker with). Exposed so a caller can shell out to a plain `deno fmt`/`deno check`
    /// subprocess against the exact same binary, without duplicating the "how do we find deno"
    /// logic or risking a different (possibly absent/mismatched-version) `deno` on `PATH`.
    pub fn deno_binary(&self) -> &str {
        &self.deno_binary
    }

    /// The automatic declaration-process cap and how much of it is in use right now (030 FR-018):
    /// `available` is free slots, `in_flight` the ones a throwaway worker holds.
    pub fn transient_counters(&self) -> (u64, u64, u64) {
        let available = self.transient_slots.available_permits() as u64;
        let in_flight = (self.transient_limit as u64).saturating_sub(available);
        (
            available,
            in_flight,
            self.transient_spawned.load(Ordering::Relaxed),
        )
    }

    /// `(hits, misses)` of the descriptor cache since boot.
    pub fn cache_counters(&self) -> (u64, u64) {
        (
            self.info_hits.load(Ordering::Relaxed),
            self.info_misses.load(Ordering::Relaxed),
        )
    }

    /// Every persistent worker this pool holds, with its process id when that is readable without
    /// waiting for an in-flight call.
    ///
    /// A locked slot is reported as `busy` with no pid rather than blocking the snapshot: the whole
    /// point of this view is to be cheap enough to take every second, and a metadata fetch can take
    /// several seconds. The call counters are kept outside the slot lock, so a busy plugin still
    /// shows its history.
    pub async fn worker_processes(&self) -> Vec<WorkerProcess> {
        let slots: Vec<(String, Arc<tokio::sync::Mutex<WorkerSlot>>)> = {
            let workers = self.workers.lock().await;
            workers
                .iter()
                .map(|(namespace, slot)| (namespace.clone(), slot.clone()))
                .collect()
        };
        let counters: HashMap<String, Arc<CallCounters>> = {
            let map = self.counters.lock().await;
            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        let mut out: Vec<WorkerProcess> = slots
            .into_iter()
            .map(|(namespace, slot)| {
                let (pid, busy) = match slot.try_lock() {
                    Ok(guard) => (guard.worker.as_ref().and_then(Worker::pid), false),
                    Err(_) => (None, true),
                };
                let counts = counters.get(&namespace).cloned().unwrap_or_default();
                let last = counts.last_call_ms.load(Ordering::Relaxed);
                WorkerProcess {
                    namespace,
                    pid,
                    busy,
                    calls: counts.calls.load(Ordering::Relaxed),
                    failures: counts.failures.load(Ordering::Relaxed),
                    last_call_at: (last > 0).then_some(last),
                }
            })
            .collect();
        out.sort_by(|a, b| a.namespace.cmp(&b.namespace));
        out
    }

    /// This namespace's call counters, created on first use.
    async fn counters_for(&self, namespace: &str) -> Arc<CallCounters> {
        self.counters
            .lock()
            .await
            .entry(namespace.to_string())
            .or_default()
            .clone()
    }

    /// Current [`PluginStamp`] of a plugin file — the cache key (V1). A missing file also drops any
    /// cached entry for it, so a deleted plugin cannot keep being listed (P2).
    async fn plugin_stamp(&self, namespace: &str, plugin_module: &Path) -> Result<PluginStamp> {
        match std::fs::metadata(plugin_module) {
            Ok(meta) => Ok(PluginStamp::of(&meta)?),
            Err(_) => {
                self.info_cache.lock().await.remove(namespace);
                Err(PoolError::NotFound(namespace.to_string()))
            }
        }
    }

    async fn cache_lookup(&self, namespace: &str, stamp: PluginStamp) -> Option<CachedPluginInfo> {
        let hit = match self.info_cache.lock().await.get(namespace) {
            Some(entry) if entry.is_current(stamp) => Some(entry.clone()),
            _ => None,
        };
        // Both lookups in `cached_descriptor` go through here, so the counter counts the two
        // places a caller asks, not one miss per declaration read.
        if hit.is_some() {
            self.info_hits.fetch_add(1, Ordering::Relaxed);
        } else {
            self.info_misses.fetch_add(1, Ordering::Relaxed);
        }
        hit
    }

    /// The per-namespace single-flight gate: everyone asking about one plugin waits on the same
    /// lock, and the winner's result lands in the cache before the losers re-check it (P4).
    async fn discovery_gate(&self, namespace: &str) -> Arc<tokio::sync::Mutex<()>> {
        self.inflight
            .lock()
            .await
            .entry(namespace.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(())))
            .clone()
    }

    /// A throwaway, **zero-permission** worker — the only thing allowed to answer "what does this
    /// plugin declare?" before the host knows what to grant it (constitutional Principle IV).
    ///
    /// The returned value holds its [`worker_limit`] slot until it is dropped, so a settings-page
    /// load that touches the whole library waits in line instead of booting every plugin at once.
    async fn spawn_transient_worker(&self, namespace: &str) -> Result<TransientWorker> {
        let slot = self
            .transient_slots
            .clone()
            .acquire_owned()
            .await
            .expect("the transient semaphore is never closed");
        let worker = Worker::spawn(
            &self.deno_binary,
            &self.dispatcher_path,
            &self.plugins_dir,
            namespace,
            &[],
            false,
            RelayAccess::none(self.http.clone()),
        )
        .await?;
        self.transient_spawned.fetch_add(1, Ordering::Relaxed);
        Ok(TransientWorker {
            worker,
            _slot: slot,
        })
    }

    /// The cached declaration for `namespace`, loading it once if this file version has not been
    /// described yet. Every caller — `plugin_info`, `plugin_options`, and `execute`'s own
    /// permission lookup — goes through here, which is what makes a library of 34 plugins cost 34
    /// loads *in total* rather than 34 per ingest (FR-004).
    async fn cached_descriptor(&self, namespace: &str) -> Result<CachedPluginInfo> {
        if !is_safe_namespace(namespace) {
            return Err(PoolError::NotFound(namespace.to_string()));
        }
        let plugin_module = self.plugins_dir.join(format!("{namespace}.ts"));
        let stamp = self.plugin_stamp(namespace, &plugin_module).await?;
        if let Some(hit) = self.cache_lookup(namespace, stamp).await {
            return Ok(hit);
        }

        let gate = self.discovery_gate(namespace).await;
        let _held = gate.lock().await;
        // Second look: while this call queued, the winner may have filled the entry — and the file
        // may have changed again, so the stamp is taken fresh rather than reused.
        let stamp = self.plugin_stamp(namespace, &plugin_module).await?;
        if let Some(hit) = self.cache_lookup(namespace, stamp).await {
            return Ok(hit);
        }

        let mut transient = self.spawn_transient_worker(namespace).await?;
        let response = transient
            .worker
            .call(namespace, "plugin_info", serde_json::json!({}))
            .await?;
        let info: PluginInfo = serde_json::from_value(response_to_result(response)?)?;
        if info.namespace != namespace {
            // P7/FR-012: a template leftover (`metadata/chaika` declaring `trabant`) must not be
            // silently adopted under whichever name the caller happened to use.
            tracing::warn!(
                requested = namespace,
                declared = %info.namespace,
                "plugin declares a namespace that differs from its install path"
            );
        }
        let entry = CachedPluginInfo {
            stamp,
            info,
            options: None,
            fetched_at: Instant::now(),
        };
        // P5: only a successful load is written. An error above returns before this point, so the
        // next call retries instead of inheriting a failure.
        self.info_cache
            .lock()
            .await
            .insert(namespace.to_string(), entry.clone());
        Ok(entry)
    }

    /// Queries a plugin's declared metadata/permissions. Cached per `(mtime, size)` — the first
    /// call for a file version spawns a zero-permission worker, every later one reads that
    /// declaration (029 US1).
    pub async fn plugin_info(&self, namespace: &str) -> Result<PluginInfo> {
        Ok(self.cached_descriptor(namespace).await?.info)
    }

    /// Queries a download plugin's optional `pluginOptions()` declaration, on the same cached
    /// declaration as [`plugin_info`](Self::plugin_info) — cheap and side-effect-free, safe to call
    /// on every settings-page load. Returns `Ok(None)` (not an error) when the plugin exports no
    /// `pluginOptions()` at all (spec FR-015), which the dispatcher signals by returning `null`.
    pub async fn plugin_options(&self, namespace: &str) -> Result<Option<PluginOptionsResult>> {
        let entry = self.cached_descriptor(namespace).await?;
        if let Some(options) = entry.options {
            return Ok(options);
        }

        let plugin_module = self.plugins_dir.join(format!("{namespace}.ts"));
        let stamp = self.plugin_stamp(namespace, &plugin_module).await?;
        let gate = self.discovery_gate(namespace).await;
        let _held = gate.lock().await;
        // Re-read under the gate: a concurrent call may have answered this while we queued.
        if let Some(options) = self
            .cache_lookup(namespace, stamp)
            .await
            .and_then(|entry| entry.options)
        {
            return Ok(options);
        }

        let mut transient = self.spawn_transient_worker(namespace).await?;
        let response = transient
            .worker
            .call(namespace, "plugin_options", serde_json::json!({}))
            .await?;
        let value = response_to_result(response)?;
        let options = if value.is_null() {
            None
        } else {
            Some(serde_json::from_value(value)?)
        };
        if let Some(entry) = self.info_cache.lock().await.get_mut(namespace) {
            // Only if the declaration still describes the same file version we just asked about.
            if entry.stamp == stamp {
                entry.options = Some(options.clone());
            }
        }
        Ok(options)
    }

    /// Reports what a plugin's own *source* says it supports, without running its entry points —
    /// same throwaway, zero-permission subprocess as [`plugin_options`](Self::plugin_options).
    ///
    /// Static analysis rather than runtime observation is what makes a freshly-edited plugin take
    /// effect on the very next settings-page load, with no download ever having run: pairs with
    /// `discover_namespaces`' own rescan-per-request behavior.
    pub async fn plugin_introspect(&self, namespace: &str) -> Result<PluginIntrospection> {
        if !is_safe_namespace(namespace) {
            return Err(PoolError::NotFound(namespace.to_string()));
        }
        let plugin_module = self.plugins_dir.join(format!("{namespace}.ts"));
        if !plugin_module.is_file() {
            return Err(PoolError::NotFound(namespace.to_string()));
        }
        let mut transient = self.spawn_transient_worker(namespace).await?;
        let response = transient
            .worker
            .call(namespace, "plugin_introspect", serde_json::json!({}))
            .await?;
        Ok(serde_json::from_value(response_to_result(response)?)?)
    }

    /// Runs `urls` through a plugin's own optional `canonicalizeSource` export, returning `Ok(None)`
    /// when the plugin has no such export (the host then applies only its own generic `trim_url`).
    ///
    /// Batched deliberately: a `source:` tag comparison normalizes both sides of every candidate
    /// pair, so a per-URL call would mean one subprocess round-trip per library archive.
    pub async fn canonicalize_sources(
        &self,
        namespace: &str,
        urls: &[String],
    ) -> Result<Option<Vec<String>>> {
        if urls.is_empty() {
            return Ok(Some(Vec::new()));
        }
        let value = self
            .execute(
                namespace,
                "canonicalize_source",
                serde_json::json!({ "urls": urls }),
            )
            .await?;
        if value.is_null() {
            return Ok(None);
        }
        let parsed: CanonicalizeSourceResult = serde_json::from_value(value)?;
        // A plugin returning the wrong number of results would silently misalign every comparison
        // that follows, which is exactly the failure mode `canonicalizeSource`'s contract warns is
        // harder to diagnose than an exception — so refuse it here instead.
        if parsed.canonical.len() != urls.len() {
            return Err(PoolError::PluginError(crate::protocol::ResponseError {
                message: format!(
                    "canonicalizeSource returned {} results for {} inputs",
                    parsed.canonical.len(),
                    urls.len()
                ),
                kind: "plugin_error".to_string(),
                error_code: None,
                data: None,
            }));
        }
        Ok(Some(parsed.canonical))
    }

    /// Asks a source extension what it currently offers for `criteria` (issue #55).
    ///
    /// Returns `Ok(None)` when the plugin exports no `discover` — the same "absence is normal, not
    /// an error" shape [`plugin_options`](Self::plugin_options) uses. Such a source cannot back a
    /// subscription, which callers check *before* offering it rather than finding out here.
    ///
    /// Runs through the persistent worker (not a throwaway subprocess) because discovery performs
    /// real network I/O under the plugin's declared permissions, exactly like a metadata or download
    /// call. `args` must already carry whatever signed-in state the plugin declared it needs; this
    /// method does not fetch it.
    pub async fn discover(
        &self,
        namespace: &str,
        args: serde_json::Value,
    ) -> Result<Option<DiscoveryResult>> {
        let value = self.execute(namespace, "discover", args).await?;
        if value.is_null() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(value)?))
    }

    /// Re-reads specific works straight from the source, without paging a listing.
    ///
    /// For a plugin declaring the `refresh_sources` capability. `args` is the same signed-in
    /// criteria object `discover` gets, plus `sources`. `Ok(None)` when the plugin has no such
    /// export.
    pub async fn refresh_sources(
        &self,
        namespace: &str,
        args: serde_json::Value,
    ) -> Result<Option<DiscoveryResult>> {
        let value = self.execute(namespace, "refresh_sources", args).await?;
        if value.is_null() {
            return Ok(None);
        }
        Ok(Some(serde_json::from_value(value)?))
    }

    /// Executes `method` against `namespace`'s persistent worker, starting it (with exactly its
    /// declared permissions) on first use, and respawning it if the plugin file's `mtime` has
    /// advanced since (a running worker never reloads its module on its own).
    ///
    /// Only this namespace's slot is locked for the call's duration, so a slow plugin cannot delay
    /// any other plugin's calls — callers into the *same* plugin queue in arrival order, which is
    /// what keeps that plugin at exactly one process.
    pub async fn execute(
        &self,
        namespace: &str,
        method: &str,
        args: serde_json::Value,
    ) -> Result<serde_json::Value> {
        if !is_safe_namespace(namespace) {
            return Err(PoolError::NotFound(namespace.to_string()));
        }
        let counters = self.counters_for(namespace).await;
        counters.calls.fetch_add(1, Ordering::Relaxed);
        counters.last_call_ms.store(now_ms(), Ordering::Relaxed);
        let slot = self.worker_slot(namespace).await;
        let mut slot = slot.lock().await;

        // A stat failure here just means the freshness check is skipped this time — the real
        // request below will surface whatever's actually wrong.
        let current_mtime = plugin_mtime(&self.plugins_dir.join(format!("{namespace}.ts"))).ok();
        let stale = match (slot.worker.as_ref(), current_mtime) {
            (Some(worker), Some(current)) => current > worker.mtime,
            _ => false,
        };
        if stale {
            slot.worker = None;
        }

        if slot.worker.is_none() {
            let info = self.plugin_info(namespace).await?;
            // A `script` plugin returns what the host should write; it never needs the grant itself.
            let flags = build_flags(&info.declared_permissions, info.kind != "script");
            let worker = Worker::spawn(
                &self.deno_binary,
                &self.dispatcher_path,
                &self.plugins_dir,
                namespace,
                &flags,
                info.declared_permissions.read,
                RelayAccess {
                    http: self.http.clone(),
                    allowed_hosts: info.declared_permissions.net.clone(),
                },
            )
            .await?;
            slot.worker = Some(worker);
        }

        let worker = slot.worker.as_mut().expect("just spawned or present");
        let result = worker.call(namespace, method, args).await;

        // Failure isolation: drop this plugin's worker on any error so the next call gets a
        // fresh process, without touching any other plugin's worker or in-flight request.
        if result.is_err() {
            slot.worker = None;
            counters.failures.fetch_add(1, Ordering::Relaxed);
        }

        response_to_result(result?)
    }

    /// This namespace's slot, created empty on first use. The map lock is held only across the
    /// lookup/insert, never across a call.
    async fn worker_slot(&self, namespace: &str) -> Arc<tokio::sync::Mutex<WorkerSlot>> {
        let mut workers = self.workers.lock().await;
        workers
            .entry(namespace.to_string())
            .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(WorkerSlot { worker: None })))
            .clone()
    }
}

fn response_to_result(response: Response) -> Result<serde_json::Value> {
    if response.ok {
        Ok(response.result.unwrap_or(serde_json::Value::Null))
    } else {
        let error = response.error.unwrap_or(crate::protocol::ResponseError {
            message: "unknown error".to_string(),
            kind: "plugin_error".to_string(),
            error_code: None,
            data: None,
        });
        Err(PoolError::PluginError(error))
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn samples_and_dispatcher() -> Option<(PathBuf, PathBuf)> {
        which_deno()?;
        let manifest_dir = env!("CARGO_MANIFEST_DIR");
        let dispatcher = PathBuf::from(manifest_dir).join("dispatcher/dispatcher.ts");
        let samples = PathBuf::from(manifest_dir).join("samples");
        Some((dispatcher, samples))
    }

    fn which_deno() -> Option<PathBuf> {
        std::env::var_os("PATH").and_then(|paths| {
            std::env::split_paths(&paths)
                .map(|dir| dir.join("deno"))
                .find(|p| p.is_file())
        })
    }

    #[test]
    fn is_safe_namespace_accepts_plain_identifiers() {
        assert!(is_safe_namespace("sample-metadata-plugin"));
        assert!(is_safe_namespace("plugin_123"));
    }

    #[test]
    fn is_safe_namespace_rejects_path_traversal_and_absolute_paths() {
        // Regression guard: `PathBuf::join` treats a leading `/` as absolute (discarding the
        // base entirely) and resolves `..` upward — either would let an unvalidated namespace
        // from an HTTP query parameter (`POST /plugins/use?plugin=...`) point at an arbitrary
        // `.ts` file outside `plugins_dir`.
        assert!(!is_safe_namespace("../escape"));
        assert!(!is_safe_namespace("../../etc/passwd"));
        assert!(!is_safe_namespace("/etc/passwd"));
        assert!(!is_safe_namespace(".."));
        assert!(!is_safe_namespace("."));
        assert!(!is_safe_namespace(""));
    }

    #[test]
    fn is_safe_namespace_accepts_multi_component_category_paths() {
        // Namespaces may now contain subdirectory components (`metadata/ehentai`, `custom/foo`) —
        // plugins are organized under `plugins_dir` by category (see `discover_namespaces` in
        // `lanrurugi-api::plugins`) plus a `custom/` tree for uploaded ones. Every component here
        // is still a plain name (`Normal`), which is what makes this safe: `a/b` can only ever
        // resolve to a literal `plugins_dir/a/b.ts`, never escape it, unlike `../` or an absolute
        // path.
        assert!(is_safe_namespace("metadata/ehentai"));
        assert!(is_safe_namespace("custom/metadata/foo"));
    }

    /// A scratch plugins directory with one plugin this test owns, so the descriptor cache can be
    /// driven the way a real edit/delete would drive it — the pool's whole point is *not* spawning a
    /// process per call, which only a file it can rewrite proves.
    struct Scratch {
        dir: PathBuf,
        dispatcher: PathBuf,
        namespace: String,
    }

    impl Scratch {
        fn new(namespace: &str) -> Option<Self> {
            which_deno()?;
            let dispatcher =
                PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dispatcher/dispatcher.ts");
            // `plugin-sdk.ts` must sit beside the dispatcher wherever it runs from (see
            // `Worker::spawn`).
            std::fs::write(
                dispatcher.parent()?.join("plugin-sdk.ts"),
                crate::PLUGIN_SDK_SCRIPT,
            )
            .ok()?;
            let dir = std::env::temp_dir().join(format!(
                "lrr-descriptor-{namespace}-{}-{}",
                std::process::id(),
                SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .ok()?
                    .as_nanos()
            ));
            std::fs::create_dir_all(&dir).ok()?;
            Some(Self {
                dir,
                dispatcher,
                namespace: namespace.to_string(),
            })
        }

        fn pool(&self) -> PluginPool {
            PluginPool::new("deno", self.dispatcher.clone(), self.dir.clone())
        }

        fn module(&self) -> PathBuf {
            self.dir.join(format!("{}.ts", self.namespace))
        }

        /// Writes a well-formed declaration whose `name` the caller can vary — the observable that
        /// says "this read came from the file, not from the cache".
        fn declare(&self, name: &str) {
            let source = format!(
                "export function pluginInfo() {{\n  return {{\n    namespace: \"{}\",\n    type: \"metadata\",\n    parameters: [],\n    declared_permissions: {{ net: [], read: false, write: false }},\n    name: \"{name}\",\n    author: \"t\",\n    description: \"t\",\n    version: \"1\",\n  }};\n}}\n",
                self.namespace
            );
            std::fs::write(self.module(), source).expect("write scratch plugin");
        }

        fn declare_broken(&self) {
            std::fs::write(
                self.module(),
                "export function pluginInfo() {\n  throw new Error(\"broken on purpose\");\n}\n",
            )
            .expect("write scratch plugin");
        }

        /// Well-formed declaration plus one metadata method that sits on a timer — a stand-in for
        /// the site round trip a real plugin spends its call in, so "did these overlap" is
        /// observable as elapsed time.
        fn declare_sleeping(&self, sleep_ms: u64) {
            let source = format!(
                "export function pluginInfo() {{\n  return {{\n    namespace: \"{}\",\n    type: \"metadata\",\n    parameters: [],\n    declared_permissions: {{ net: [], read: false, write: false }},\n    name: \"t\",\n    author: \"t\",\n    description: \"t\",\n    version: \"1\",\n  }};\n}}\nexport async function execMetadata() {{\n  await new Promise((resolve) => setTimeout(resolve, {sleep_ms}));\n  return {{ title: \"x\" }};\n}}\n",
                self.namespace
            );
            std::fs::write(self.module(), source).expect("write scratch plugin");
        }
    }

    #[tokio::test]
    async fn a_second_descriptor_read_is_served_from_the_cache() {
        let Some(scratch) = Scratch::new("cachehit") else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        scratch.declare("first");
        let pool = scratch.pool();

        let first = pool.cached_descriptor(&scratch.namespace).await.unwrap();
        let second = pool.cached_descriptor(&scratch.namespace).await.unwrap();
        assert_eq!(
            first.fetched_at, second.fetched_at,
            "a hit returns the entry already held — a fresh timestamp would mean a second deno process"
        );
        assert_eq!(second.info.name, "first");
    }

    #[tokio::test]
    async fn rewriting_the_plugin_file_invalidates_the_cached_declaration() {
        let Some(scratch) = Scratch::new("cachebust") else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        scratch.declare("first");
        let pool = scratch.pool();
        let before = pool.cached_descriptor(&scratch.namespace).await.unwrap();
        assert_eq!(before.info.name, "first");

        // Deliberately a different length as well as different content: this must miss even on a
        // filesystem whose mtime resolution is coarse.
        scratch.declare("renamed-and-longer");
        let after = pool.cached_descriptor(&scratch.namespace).await.unwrap();
        assert_eq!(after.info.name, "renamed-and-longer");
        assert_ne!(before.fetched_at, after.fetched_at);
    }

    /// The case the old 60 s TTL existed to bound: a swap that keeps both `mtime` and `size`
    /// (`cp -p`, `rsync -a`, untarring a backup over the file). The stamp still has to move —
    /// neither `ctime` nor `inode` travels with that kind of replacement.
    #[test]
    fn a_swap_that_preserves_the_timestamp_still_changes_the_stamp() {
        let path = std::env::temp_dir().join(format!(
            "lrr-stamp-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::write(
            &path,
            "export function pluginInfo() { return { name: 'a' }; }\n",
        )
        .unwrap();
        let before = PluginStamp::of(&std::fs::metadata(&path).unwrap()).unwrap();

        // Same length, old mtime restored the way `cp -p` does it, landing via `rename` the way an
        // editor's write-to-temp replacement does.
        let staged = path.with_extension("staged");
        std::fs::write(
            &staged,
            "export function pluginInfo() { return { name: 'b' }; }\n",
        )
        .unwrap();
        let file = std::fs::OpenOptions::new()
            .write(true)
            .open(&staged)
            .unwrap();
        file.set_times(std::fs::FileTimes::new().set_modified(before.modified))
            .unwrap();
        drop(file);
        std::fs::rename(&staged, &path).unwrap();

        let after = PluginStamp::of(&std::fs::metadata(&path).unwrap()).unwrap();
        std::fs::remove_file(&path).ok();
        assert_eq!(
            after.size, before.size,
            "the replacement is the same length"
        );
        assert_eq!(
            after.modified, before.modified,
            "…and carries the old mtime, which is what defeats a `(mtime, size)` cache key"
        );
        assert_ne!(
            after, before,
            "inode/ctime still moved, so the cached declaration must miss"
        );
    }

    /// The TTL is a backstop, not the freshness mechanism: a five-minute-old entry is still
    /// current, which is what keeps an idle library from re-spawning 21 declaration processes on
    /// the next ingest.
    #[test]
    fn an_entry_stays_current_across_an_idle_stretch_longer_than_one_minute() {
        let info: PluginInfo = serde_json::from_value(serde_json::json!({
            "namespace": "idle",
            "type": "metadata",
            "declared_permissions": { "net": [], "read": false, "write": false },
        }))
        .unwrap();
        let stamp = PluginStamp {
            modified: SystemTime::UNIX_EPOCH,
            size: 10,
            changed: (1, 2),
            inode: 3,
        };
        let entry = CachedPluginInfo {
            stamp,
            info,
            options: None,
            fetched_at: Instant::now() - Duration::from_secs(5 * 60),
        };
        assert!(
            entry.is_current(stamp),
            "a 60 s TTL would have made this stale"
        );
        assert!(
            !entry.is_current(PluginStamp { size: 11, ..stamp }),
            "any stamp field moving must still invalidate it"
        );
    }

    #[tokio::test]
    async fn a_deleted_plugin_is_not_found_and_drops_its_cached_entry() {
        let Some(scratch) = Scratch::new("cachegone") else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        scratch.declare("first");
        let pool = scratch.pool();
        pool.plugin_info(&scratch.namespace).await.unwrap();

        std::fs::remove_file(scratch.module()).unwrap();
        let err = pool.plugin_info(&scratch.namespace).await.unwrap_err();
        assert!(matches!(err, PoolError::NotFound(_)));
        assert!(
            pool.info_cache.lock().await.is_empty(),
            "a deleted plugin must not keep being listed from a stale cached declaration"
        );
    }

    #[tokio::test]
    async fn a_plugin_that_fails_to_declare_is_not_cached() {
        let Some(scratch) = Scratch::new("cachefail") else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        scratch.declare_broken();
        let pool = scratch.pool();
        let err = pool.plugin_info(&scratch.namespace).await.unwrap_err();
        assert!(!matches!(err, PoolError::NotFound(_)), "got {err:?}");
        assert!(
            pool.info_cache.lock().await.is_empty(),
            "a failure must not be remembered as if it were a declaration"
        );
    }

    #[tokio::test]
    async fn concurrent_reads_of_a_cold_cache_load_the_plugin_once() {
        let Some(scratch) = Scratch::new("cacherace") else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        scratch.declare("first");
        let pool = Arc::new(scratch.pool());
        let mut tasks = Vec::new();
        for _ in 0..8 {
            let pool = Arc::clone(&pool);
            let namespace = scratch.namespace.clone();
            tasks.push(tokio::spawn(async move {
                pool.cached_descriptor(&namespace)
                    .await
                    .map(|entry| entry.fetched_at)
            }));
        }
        let mut stamps = Vec::new();
        for task in tasks {
            stamps.push(task.await.unwrap().unwrap());
        }
        assert!(
            stamps.windows(2).all(|pair| pair[0] == pair[1]),
            "every concurrent caller must observe the same load, not one load per caller"
        );
        assert_eq!(pool.info_cache.lock().await.len(), 1);
    }

    #[test]
    fn the_process_cap_is_bounded_whatever_the_machine_reports() {
        // Derived from the core count, but always something a dispatcher-heavy box can afford:
        // never zero (nothing would ever load) and never unbounded (a 64-core host does not get
        // 32 Deno processes out of this).
        assert!((2..=8).contains(&worker_limit()));
    }

    #[tokio::test]
    async fn a_declaration_read_waits_for_a_free_slot_and_returns_it() {
        let Some(scratch) = Scratch::new("gated-plugin") else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        scratch.declare("gated");
        let limit = 2;
        let pool = PluginPool::with_worker_limit(
            "deno",
            scratch.dispatcher.clone(),
            scratch.dir.clone(),
            limit,
        );

        // Take every slot first, so the read below is provably gated rather than merely fast.
        let held: Vec<_> = (0..limit)
            .map(|_| {
                pool.transient_slots
                    .clone()
                    .try_acquire_owned()
                    .expect("the scratch pool starts with every slot free")
            })
            .collect();
        let gated = tokio::time::timeout(
            std::time::Duration::from_millis(250),
            pool.plugin_info(&scratch.namespace),
        )
        .await;
        assert!(
            gated.is_err(),
            "a declaration read must not start while every slot is taken"
        );

        drop(held);
        let info = tokio::time::timeout(
            std::time::Duration::from_secs(30),
            pool.plugin_info(&scratch.namespace),
        )
        .await
        .expect("a freed slot lets the read through")
        .expect("read the declaration");
        assert_eq!(info.namespace, scratch.namespace);
        // The timed-out attempt and the successful one both have to hand their slot back, or the
        // next library-wide discover would stall behind a phantom holder.
        assert_eq!(pool.transient_slots.available_permits(), limit);
    }

    /// FR-015: two plugins waiting on their own sites must wait at the same time, not one after the
    /// other — the global worker-map lock used to make the second call queue behind the first.
    #[tokio::test]
    async fn two_plugins_slow_methods_run_at_the_same_time() {
        let (Some(first), Some(second)) = (
            Scratch::new("parallel-plugin-a"),
            Scratch::new("parallel-plugin-b"),
        ) else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        const SLEEP_MS: u64 = 150;
        first.declare_sleeping(SLEEP_MS);
        second.declare_sleeping(SLEEP_MS);
        // One pool with both plugins in it, like a real installation.
        let pool = PluginPool::new("deno", first.dispatcher.clone(), first.dir.clone());
        std::fs::copy(second.module(), first.dir.join("parallel-plugin-b.ts"))
            .expect("copy the second scratch plugin next to the first");

        let started = std::time::Instant::now();
        let (a, b) = tokio::join!(
            pool.execute(&first.namespace, "exec_metadata", serde_json::json!({})),
            pool.execute("parallel-plugin-b", "exec_metadata", serde_json::json!({})),
        );
        let elapsed = started.elapsed();
        a.expect("first plugin answered");
        b.expect("second plugin answered");
        assert!(
            elapsed < std::time::Duration::from_millis(SLEEP_MS * 2),
            "two plugins must not queue behind each other, took {elapsed:?}"
        );
        assert_eq!(pool.workers.lock().await.len(), 2);
    }

    /// The other half of FR-015: the *same* plugin still answers one call at a time from one
    /// process, so its calls queue in arrival order rather than racing inside one dispatcher.
    #[tokio::test]
    async fn one_plugin_still_answers_its_calls_one_at_a_time() {
        let Some(scratch) = Scratch::new("serialized-plugin") else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        const SLEEP_MS: u64 = 150;
        scratch.declare_sleeping(SLEEP_MS);
        let pool = scratch.pool();

        let started = std::time::Instant::now();
        let (a, b) = tokio::join!(
            pool.execute(&scratch.namespace, "exec_metadata", serde_json::json!({})),
            pool.execute(&scratch.namespace, "exec_metadata", serde_json::json!({})),
        );
        let elapsed = started.elapsed();
        a.expect("first call answered");
        b.expect("second call answered");
        assert!(
            elapsed >= std::time::Duration::from_millis(SLEEP_MS * 2),
            "calls into one plugin must not overlap, took {elapsed:?}"
        );
        assert_eq!(
            pool.workers.lock().await.len(),
            1,
            "one namespace stays one process however many calls arrive"
        );
    }

    #[tokio::test]
    async fn plugin_info_matches_sample_plugins_declaration() {
        let Some((dispatcher, samples)) = samples_and_dispatcher() else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        let pool = PluginPool::new("deno", dispatcher, samples);
        let info = pool.plugin_info("sample-metadata-plugin").await.unwrap();
        assert_eq!(info.namespace, "sample-metadata-plugin");
        assert_eq!(info.kind, "metadata");
        assert_eq!(
            info.declared_permissions.net,
            vec!["metadata.example.invalid"]
        );
        assert!(!info.declared_permissions.read);
        assert!(!info.declared_permissions.write);
    }

    #[tokio::test]
    async fn plugin_options_returns_none_when_the_plugin_exports_none() {
        let Some((dispatcher, samples)) = samples_and_dispatcher() else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        let pool = PluginPool::new("deno", dispatcher, samples);
        let options = pool.plugin_options("sample-metadata-plugin").await.unwrap();
        assert!(options.is_none());
    }

    #[tokio::test]
    async fn plugin_options_matches_sample_download_plugins_declaration() {
        let Some((dispatcher, samples)) = samples_and_dispatcher() else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        let pool = PluginPool::new("deno", dispatcher, samples);
        let options = pool
            .plugin_options("sample-download-plugin")
            .await
            .unwrap()
            .expect("sample-download-plugin declares pluginOptions()");
        assert_eq!(options.domain_rules.len(), 1);
        assert_eq!(
            options.domain_rules[0].pattern.as_deref(),
            Some("*.download.example.invalid")
        );
        assert_eq!(options.domain_rules[0].max_concurrent, Some(2));
        let bundle = options
            .bundle_as_archive
            .expect("sample-download-plugin declares bundle_as_archive");
        assert!(bundle.default);
    }

    #[tokio::test]
    async fn exec_metadata_runs_end_to_end_through_the_real_dispatcher() {
        let Some((dispatcher, samples)) = samples_and_dispatcher() else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        let pool = PluginPool::new("deno", dispatcher, samples);
        let result = pool
            .execute(
                "sample-metadata-plugin",
                "exec_metadata",
                serde_json::json!({ "archive_id": "deadbeef" }),
            )
            .await
            .unwrap();
        assert_eq!(result["tags"], "source:sample,archive:deadbeef");
        assert_eq!(result["summary"], "Enriched by sample-metadata-plugin.");
    }

    #[tokio::test]
    async fn unknown_plugin_method_is_isolated_as_a_plugin_error() {
        let Some((dispatcher, samples)) = samples_and_dispatcher() else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        let pool = PluginPool::new("deno", dispatcher, samples);
        let err = pool
            .execute(
                "sample-metadata-plugin",
                "exec_download",
                serde_json::json!({}),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, PoolError::PluginError(_)));

        // The pool itself must still be usable afterwards for the same plugin (failure isolation
        // doesn't poison future calls, just that one).
        let result = pool
            .execute(
                "sample-metadata-plugin",
                "exec_metadata",
                serde_json::json!({ "archive_id": "still-works" }),
            )
            .await
            .unwrap();
        assert_eq!(result["tags"], "source:sample,archive:still-works");
    }

    /// Writes one throwaway plugin whose `execDownload` body is `body`, and reports what
    /// `plugin_introspect` makes of it. Drives the REAL dispatcher rather than a copy of its
    /// lexical scanner, so these cases can't drift from the code that actually ships.
    async fn introspects_version_history(name: &str, body: &str) -> Option<bool> {
        which_deno()?;
        let dispatcher = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dispatcher/dispatcher.ts");
        let dir = std::env::temp_dir().join(format!("lrr-introspect-{name}"));
        std::fs::create_dir_all(&dir).ok()?;
        // `plugin-sdk.ts` must sit beside the dispatcher wherever it runs from (see `Worker::spawn`).
        std::fs::write(
            dispatcher.parent()?.join("plugin-sdk.ts"),
            crate::PLUGIN_SDK_SCRIPT,
        )
        .ok()?;
        let source = format!(
            "export function pluginInfo() {{\n  return {{\n    namespace: \"{name}\",\n    type: \"download\",\n    parameters: [],\n    declared_permissions: {{ net: [], read: false, write: false }},\n    name: \"{name}\",\n    author: \"t\",\n    description: \"t\",\n    version: \"1\",\n  }};\n}}\n\nexport async function execDownload(_hostArgs) {{\n{body}\n}}\n"
        );
        std::fs::write(dir.join(format!("{name}.ts")), source).ok()?;
        let pool = PluginPool::new("deno", dispatcher, dir);
        Some(
            pool.plugin_introspect(name)
                .await
                .ok()?
                .returns_version_history,
        )
    }

    /// Same harness, but for a `discover` body, returning which candidate fields the static analysis
    /// found. Drives whether the settings form offers a filter over each one.
    async fn introspects_candidate_fields(name: &str, body: &str) -> Option<Vec<String>> {
        which_deno()?;
        let dispatcher = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("dispatcher/dispatcher.ts");
        let dir = std::env::temp_dir().join(format!("lrr-introspect-{name}"));
        std::fs::create_dir_all(&dir).ok()?;
        std::fs::write(
            dispatcher.parent()?.join("plugin-sdk.ts"),
            crate::PLUGIN_SDK_SCRIPT,
        )
        .ok()?;
        let source = format!(
            "export function pluginInfo() {{\n  return {{\n    namespace: \"{name}\",\n    type: \"discovery\",\n    parameters: [],\n    declared_permissions: {{ net: [], read: false, write: false }},\n    name: \"{name}\",\n    author: \"t\",\n    description: \"t\",\n    version: \"1\",\n  }};\n}}\n\nexport async function discover(_criteria) {{\n{body}\n}}\n"
        );
        std::fs::write(dir.join(format!("{name}.ts")), source).ok()?;
        let pool = PluginPool::new("deno", dispatcher, dir);
        let mut fields = pool.plugin_introspect(name).await.ok()?.candidate_fields;
        fields.sort();
        Some(fields)
    }

    /// A filter may only be offered over a field the extension actually writes — offering one over a
    /// field never populated would present a rule that silently matches nothing.
    #[tokio::test]
    async fn candidate_fields_are_found_from_what_discover_writes() {
        let Some(found) = introspects_candidate_fields(
            "candfields",
            r#"  return { candidates: [{ source: "s", title: "t", posted_at: "2026-01-01 00:00", rating: 4, tags: [] }] };"#,
        )
        .await
        else {
            eprintln!("skipping: deno not available");
            return;
        };
        assert_eq!(
            found,
            vec![
                "posted_at".to_string(),
                "rating".to_string(),
                "tags".to_string(),
                "title".to_string()
            ],
            "and notably not category/uploader/pages, which this body never writes"
        );
    }

    /// A field named only in prose must not count, or the form would offer a filter over something
    /// never populated.
    #[tokio::test]
    async fn a_field_mentioned_only_in_a_string_does_not_count() {
        let Some(found) = introspects_candidate_fields(
            "candprose",
            r#"  console.log("no rating or pages here");
  return { candidates: [{ source: "s" }] };"#,
        )
        .await
        else {
            eprintln!("skipping: deno not available");
            return;
        };
        assert!(found.is_empty(), "got {found:?}");
    }

    /// A quote inside a regex literal has no closing partner, and a scanner that blanks "to the
    /// closing quote" would erase the rest of the file — hiding the real `version_history` key and
    /// silently reporting the plugin as unsupported. Found by review; this is the regression guard.
    #[tokio::test]
    async fn a_quote_inside_a_regex_literal_does_not_hide_version_history() {
        let Some(found) = introspects_version_history(
            "regexquote",
            r#"  const m = String(_hostArgs).match(/["']/);
  return { downloads: [], version_history: [], matched: m };"#,
        )
        .await
        else {
            eprintln!("skipping: deno not found on PATH");
            return;
        };
        assert!(
            found,
            "a regex literal containing a quote must not swallow the rest of the file"
        );
    }

    /// A regex literal whose quotes are unbalanced on their own (`[^"'<>]`) used to pair with the
    /// next real string's opening quote, blanking every line between them — e-hentai's discovery
    /// plugin lost `posted_at`/`tags`/`uploader` this way, so the preview showed no publish times
    /// and the form offered only the one filter field left over.
    #[tokio::test]
    async fn a_regex_literal_with_quotes_does_not_hide_candidate_fields() {
        let Some(found) = introspects_candidate_fields(
            "regexcand",
            r#"  const uploader = row.match(/uploader\/([^"'<>]+)/)?.[1];
  return { candidates: [{ source: "s", posted_at: "2026-01-01 00:00", uploader, tags: [] }] };"#,
        )
        .await
        else {
            eprintln!("skipping: deno not available");
            return;
        };
        assert_eq!(
            found,
            vec![
                "posted_at".to_string(),
                "tags".to_string(),
                "uploader".to_string()
            ],
        );
    }

    #[tokio::test]
    async fn a_shorthand_property_counts_as_support() {
        let Some(found) = introspects_version_history(
            "shorthand",
            "  const version_history = [];\n  return { downloads: [], version_history };",
        )
        .await
        else {
            return;
        };
        assert!(found);
    }

    #[tokio::test]
    async fn a_quoted_property_key_counts_as_support() {
        let Some(found) = introspects_version_history(
            "quotedkey",
            r#"  return { downloads: [], "version_history": [] };"#,
        )
        .await
        else {
            return;
        };
        assert!(found);
    }

    /// The contract says a mention in a comment does not count — this is the case a previous
    /// implementation got wrong by matching quoted keys against the raw, un-blanked source.
    #[tokio::test]
    async fn a_mention_in_a_comment_is_not_support() {
        let Some(found) = introspects_version_history(
            "commentonly",
            "  // this plugin does not report \"version_history\": nothing to see\n  return { downloads: [] };",
        )
        .await
        else {
            return;
        };
        assert!(!found);
    }

    /// ...nor does a mention inside a string, even one that looks like a key because it contains
    /// a colon.
    #[tokio::test]
    async fn a_mention_inside_a_string_is_not_support() {
        let Some(found) = introspects_version_history(
            "stringonly",
            r#"  const msg = "version_history: none found";
  return { downloads: [], error: { error_code: msg } };"#,
        )
        .await
        else {
            return;
        };
        assert!(!found);
    }

    /// Reading the key off some other object says nothing about whether this plugin reports one.
    #[tokio::test]
    async fn a_property_read_is_not_support() {
        let Some(found) = introspects_version_history(
            "propread",
            "  const prior = (_hostArgs ?? {}).version_history;\n  return { downloads: [], prior };",
        )
        .await
        else {
            return;
        };
        assert!(!found);
    }

    #[tokio::test]
    async fn a_plugin_that_never_mentions_it_is_not_support() {
        let Some(found) = introspects_version_history("plain", "  return { downloads: [] };").await
        else {
            return;
        };
        assert!(!found);
    }
}
