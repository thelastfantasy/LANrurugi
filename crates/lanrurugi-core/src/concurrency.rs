//! Bridges CPU-bound `rayon` work into async code via `tokio::task::spawn_blocking`, per
//! constitution Principle III / research.md §5. No CPU-bound work (hashing, image decode/resize)
//! may run directly inside an `async fn` body on a Tokio worker thread — this is the only sanctioned
//! way to cross that boundary.

use tokio::task::JoinError;

#[derive(Debug, thiserror::Error)]
pub enum BlockingTaskError {
    #[error("blocking task panicked or was cancelled: {0}")]
    Join(#[from] JoinError),
}

/// Runs `f` on Tokio's blocking-thread pool, off the async reactor, so callers proceed
/// concurrently while `f` (typically a rayon parallel-iterator/scope call) runs.
pub async fn run_blocking<F, T>(f: F) -> Result<T, BlockingTaskError>
where
    F: FnOnce() -> T + Send + 'static,
    T: Send + 'static,
{
    tokio::task::spawn_blocking(f).await.map_err(Into::into)
}

/// Thread budget for CPU-bound `rayon` work — roughly 30% of available cores, matching
/// `lanrurugi-api::recommend_precompute::precompute_worker_budget`'s own formula (duplicated here
/// per that crate's own doc comment, since this crate can't depend on `lanrurugi-api`). Callers
/// whose `work` closure blocks on an external, effectively single-threaded resource (e.g. `parallel_map`
/// below, when `work` is a GPU-worker IPC round trip) especially need this: without a cap, rayon's
/// uncapped global pool (one thread per core) spins up one OS thread per item that all sit blocked
/// waiting on that same serial resource, starving the host's other threads — including the server's
/// own Tokio runtime workers — of CPU scheduling time (issue #103: this starved `/health` itself
/// during concurrent translation).
fn worker_budget() -> usize {
    (std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        * 3
        / 10)
        .max(1)
}

/// Runs a rayon parallel operation over `items`, off the async reactor, on a dedicated pool capped
/// to `worker_budget()` threads (not rayon's uncapped global pool — see that function's doc
/// comment for why).  `work` executes once per item; results are collected in input order.
pub async fn parallel_map<T, R, F>(items: Vec<T>, work: F) -> Result<Vec<R>, BlockingTaskError>
where
    T: Send + 'static,
    R: Send + 'static,
    F: Fn(T) -> R + Send + Sync + 'static,
{
    run_blocking(move || {
        use rayon::prelude::*;
        let pool = rayon::ThreadPoolBuilder::new()
            .num_threads(worker_budget())
            .build()
            .expect("rayon pool build");
        pool.install(|| items.into_par_iter().map(work).collect())
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn parallel_map_preserves_order() {
        let input: Vec<u32> = (0..1000).collect();
        let result = parallel_map(input.clone(), |x| x * 2).await.unwrap();
        let expected: Vec<u32> = input.into_iter().map(|x| x * 2).collect();
        assert_eq!(result, expected);
    }

    #[tokio::test]
    async fn run_blocking_does_not_block_concurrent_tasks() {
        use std::time::Duration;

        let blocking = run_blocking(|| {
            std::thread::sleep(Duration::from_millis(200));
            42
        });
        let fast = async {
            tokio::time::sleep(Duration::from_millis(10)).await;
            "fast task finished first"
        };

        let (blocking_result, fast_result) = tokio::join!(blocking, fast);
        assert_eq!(blocking_result.unwrap(), 42);
        assert_eq!(fast_result, "fast task finished first");
    }
}
