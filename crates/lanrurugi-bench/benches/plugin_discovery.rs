//! [029] microbenchmarks: what a plugin declaration read costs, what recording a job phase costs,
//! and whether two different plugins can talk to their sites at the same time. The first is the
//! number the descriptor cache is supposed to remove from every plugin call; the second exists to
//! show the new timing instrumentation is noise next to it; the third is the number the
//! per-namespace worker lock is supposed to fix (a global one serialized every plugin in the
//! process behind whichever was waiting on a site).
//!
//! All of them run against a real `deno` process with a scratch plugin, because that spawn *is*
//! the cost being measured — a mock would measure nothing. Without `deno` on `PATH` the benchmarks
//! print a skip line and return, so the target stays runnable anywhere.

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use criterion::{criterion_group, criterion_main, Criterion};
use lanrurugi_core::jobs::JobRegistry;
use lanrurugi_plugin::pool::PluginPool;
use lanrurugi_plugin::PLUGIN_SDK_SCRIPT;

const NAMESPACE: &str = "metadata/bench";

/// A scratch plugins directory with one plugin, mirroring `pool::tests::Scratch` — the file has to
/// be ours so a rewrite can invalidate the cache on demand.
struct Scratch {
    dir: PathBuf,
    dispatcher: PathBuf,
}

impl Scratch {
    fn new() -> Option<Self> {
        std::process::Command::new("deno")
            .arg("--version")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .ok()?;
        let dispatcher = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../lanrurugi-plugin/dispatcher/dispatcher.ts");
        std::fs::write(
            dispatcher.parent()?.join("plugin-sdk.ts"),
            PLUGIN_SDK_SCRIPT,
        )
        .ok()?;
        let dir = std::env::temp_dir().join(format!(
            "lrr-bench-descriptor-{}-{}",
            std::process::id(),
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()?
                .as_nanos()
        ));
        std::fs::create_dir_all(dir.join("metadata")).ok()?;
        Some(Self { dir, dispatcher })
    }

    fn pool(&self) -> PluginPool {
        PluginPool::new("deno", self.dispatcher.clone(), self.dir.clone())
    }

    fn module(&self) -> PathBuf {
        self.dir.join(format!("{NAMESPACE}.ts"))
    }
}

/// Writes the declaration, varying the `name`'s length so the file's size changes too — a rewrite
/// that kept both mtime and size would legitimately stay cached.
fn write_plugin(module: &Path, filler: usize) {
    let name = "n".repeat(1 + filler % 40);
    std::fs::write(
        module,
        format!(
            "export function pluginInfo() {{\n  return {{\n    namespace: \"{NAMESPACE}\",\n    type: \"metadata\",\n    parameters: [],\n    declared_permissions: {{ net: [], read: false, write: false }},\n    name: \"{name}\",\n    author: \"t\",\n    description: \"t\",\n    version: \"1\",\n  }};\n}}\n"
        ),
    )
    .expect("write scratch plugin");
}

fn bench_descriptor(c: &mut Criterion) {
    let Some(scratch) = Scratch::new() else {
        eprintln!("skipping plugin_descriptor: deno not found on PATH");
        return;
    };
    let module = scratch.module();
    let rt = tokio::runtime::Runtime::new().unwrap();
    let mut group = c.benchmark_group("plugin_descriptor");
    // One spawn is tens to hundreds of milliseconds; ten samples is plenty to see the split
    // between process start and the cache hit that replaces it.
    group
        .sample_size(10)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(10));

    // Deliberately `iter`, not `iter_batched`: criterion batches `iter_batched`'s setup, so only
    // the first call in each batch actually missed the cache and the mean came out two orders of
    // magnitude too low. The rewrite has to be inside the timed closure for every iteration to pay
    // for a spawn — it costs microseconds against a spawn's tens of milliseconds.
    group.bench_function("cold_spawn", |b| {
        let pool = scratch.pool();
        let mut turn = 0usize;
        b.iter(|| {
            turn += 1;
            write_plugin(&module, turn);
            rt.block_on(async { pool.plugin_info(NAMESPACE).await.unwrap() });
        });
    });

    group.bench_function("warm_cache_hit", |b| {
        let pool = scratch.pool();
        write_plugin(&module, 0);
        rt.block_on(async { pool.plugin_info(NAMESPACE).await.unwrap() });
        b.iter(|| {
            rt.block_on(async { pool.plugin_info(NAMESPACE).await.unwrap() });
        });
    });

    group.finish();
}

fn bench_phase_recording(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let jobs = JobRegistry::new();
    let id = rt.block_on(jobs.create("bench"));
    let mut group = c.benchmark_group("job_phase");
    // One thousand *ingests*' worth of marks inside a single `block_on`: entering a Tokio runtime
    // costs tens of microseconds on its own, which would otherwise dwarf what is being measured.
    // Each round replays the real sequence (mark_active clearing a new task's timeline, then the
    // six boundaries), so every `begin` but the first closes the phase before it and the reverse
    // scan in `close_open_phase` stays O(1) — the shape a real task produces. Divide by `OPS` for
    // one ingest.
    const OPS: usize = 1000;
    group.bench_function("one_ingest_x1000", |b| {
        b.iter(|| {
            rt.block_on(async {
                for _ in 0..OPS {
                    jobs.mark_active(&id).await;
                    jobs.begin_phase(&id, lanrurugi_core::jobs::PHASE_QUEUED)
                        .await;
                    jobs.begin_phase(&id, lanrurugi_core::jobs::PHASE_NEGOTIATE)
                        .await;
                    jobs.begin_phase(&id, lanrurugi_core::jobs::PHASE_WAITING)
                        .await;
                    jobs.begin_phase(&id, lanrurugi_core::jobs::PHASE_TRANSFER)
                        .await;
                    jobs.begin_phase(&id, lanrurugi_core::jobs::PHASE_INGEST)
                        .await;
                    jobs.end_phase(&id).await;
                }
            });
        });
    });
    // The control: one write-lock acquisition per op, no clock read, no phase bookkeeping — what
    // an existing per-chunk progress write already costs.
    group.bench_function("set_progress_x1000", |b| {
        b.iter(|| {
            rt.block_on(async {
                for _ in 0..OPS {
                    jobs.set_progress(&id, 0.5).await;
                }
            });
        });
    });
    group.finish();
}

/// A plugin that declares one metadata method which sits on a timer. The timer stands in for the
/// site round trip a real metadata plugin spends its time in — what matters here is only that the
/// call is *waiting*, so whether another plugin's call can proceed meanwhile is visible as time.
fn write_sleepy_plugin(path: &Path, namespace: &str, sleep_ms: u64) {
    std::fs::write(
        path,
        format!(
            "export function pluginInfo() {{\n  return {{\n    namespace: \"{namespace}\",\n    type: \"metadata\",\n    parameters: [],\n    declared_permissions: {{ net: [], read: false, write: false }},\n    name: \"{namespace}\",\n    author: \"t\",\n    description: \"t\",\n    version: \"1\",\n  }};\n}}\nexport async function execMetadata() {{\n  await new Promise((resolve) => setTimeout(resolve, {sleep_ms}));\n  return {{ title: \"x\" }};\n}}\n"
        ),
    )
    .expect("write sleepy plugin");
}

fn bench_cross_plugin_concurrency(c: &mut Criterion) {
    let Some(scratch) = Scratch::new() else {
        eprintln!("skipping plugin_call_concurrency: deno not found on PATH");
        return;
    };
    const SLEEP_MS: u64 = 25;
    const A: &str = "metadata/bench-a";
    const B: &str = "metadata/bench-b";
    for namespace in [A, B] {
        let path = scratch.dir.join(format!("{namespace}.ts"));
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_sleepy_plugin(&path, namespace, SLEEP_MS);
    }

    let rt = tokio::runtime::Runtime::new().unwrap();
    let pool = scratch.pool();
    // Warm both workers up outside the measurement: the spawn is the other benchmark's subject.
    rt.block_on(async {
        pool.execute(A, "exec_metadata", serde_json::json!({}))
            .await
            .unwrap();
        pool.execute(B, "exec_metadata", serde_json::json!({}))
            .await
            .unwrap();
    });

    let mut group = c.benchmark_group("plugin_call_concurrency");
    group
        .sample_size(10)
        .warm_up_time(Duration::from_secs(1))
        .measurement_time(Duration::from_secs(10));

    // The baseline the global lock used to impose on *every* pair of calls, whatever plugins they
    // belonged to: the second call cannot start until the first answers.
    group.bench_function("two_plugins_one_after_the_other", |b| {
        b.iter(|| {
            rt.block_on(async {
                pool.execute(A, "exec_metadata", serde_json::json!({}))
                    .await
                    .unwrap();
                pool.execute(B, "exec_metadata", serde_json::json!({}))
                    .await
                    .unwrap();
            });
        });
    });

    group.bench_function("two_plugins_at_the_same_time", |b| {
        b.iter(|| {
            rt.block_on(async {
                let (first, second) = tokio::join!(
                    pool.execute(A, "exec_metadata", serde_json::json!({})),
                    pool.execute(B, "exec_metadata", serde_json::json!({})),
                );
                first.unwrap();
                second.unwrap();
            });
        });
    });

    group.finish();
}

criterion_group!(
    plugin_discovery,
    bench_descriptor,
    bench_phase_recording,
    bench_cross_plugin_concurrency
);
criterion_main!(plugin_discovery);
