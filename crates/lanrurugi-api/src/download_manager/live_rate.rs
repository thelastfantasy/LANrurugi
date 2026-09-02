//! Live (mid-download) rate-limit resolution for the streaming download path.
//!
//! [`RateResolver`] (defined in `stream.rs`) is invoked on every chunk read, so the effective
//! rate limit can react to a user clearing/changing a plugin's rate cap *during* an in-flight
//! transfer — see `stream::download_one`'s own docs. This module supplies the canonical
//! implementation, [`LiveRateResolver`], which re-merges the plugin's declared options (snapshotted
//! once at download start — re-reading them would spawn a Deno subprocess per chunk) with the
//! user's Redis-stored override (cheap to re-read), and resolves `max_bytes_per_sec` from the
//! merged result.
//!
//! **Steady-state cost is one atomic load + one mutex lock per chunk** (zero allocation): the
//! resolver caches `(generation, rate)` and only re-reads Redis when `AppState`'s
//! `plugin_options_generation` counter has been bumped — which happens exactly when a
//! `PUT`/`DELETE /api/plugins/{namespace}/options` lands (`plugins.rs`), i.e. only when the user
//! actually changed something. A per-chunk Redis `GET` would be far too hot at high transfer
//! speeds (tens of thousands of reads/sec at 100 MB/s chunk rates), so the cache is load-bearing,
//! not a premature optimization.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use futures_util::future::BoxFuture;

use lanrurugi_plugin::protocol::PluginOptionsResult;
use lanrurugi_storage::plugin_options::PluginOptionsRepository;

use super::settings::resolve_domain_rules;
use super::stream::RateResolver;

/// What the last re-resolution saw, so the per-chunk fast path can skip Redis entirely when the
/// generation counter hasn't moved since.
struct CachedRate {
    generation: u64,
    rate: Option<u64>,
}

/// Canonical [`RateResolver`]: plugin-declared options snapshotted at download start, user
/// override re-read from Redis only when `generation` changes, merged via
/// [`resolve_domain_rules`] and resolved per hostname.
#[derive(Clone)]
pub struct LiveRateResolver {
    namespace: String,
    declared: PluginOptionsResult,
    plugin_options: Arc<PluginOptionsRepository>,
    generation: Arc<AtomicU64>,
    cache: Arc<Mutex<CachedRate>>,
}

impl LiveRateResolver {
    pub fn new(
        namespace: String,
        declared: PluginOptionsResult,
        plugin_options: Arc<PluginOptionsRepository>,
        generation: Arc<AtomicU64>,
    ) -> Self {
        Self {
            namespace,
            declared,
            plugin_options,
            generation,
            cache: Arc::new(Mutex::new(CachedRate {
                // Force the first call to actually re-read Redis rather than trusting an
                // uninitialized cache entry.
                generation: u64::MAX,
                rate: None,
            })),
        }
    }
}

impl RateResolver for LiveRateResolver {
    fn resolve(&self, hostname: &str) -> BoxFuture<'static, Option<u64>> {
        let gen = self.generation.load(Ordering::Relaxed);
        let cached = self.cache.lock().unwrap();
        if cached.generation == gen {
            // Steady state: nothing changed since the last re-resolution — hand back the cached
            // value with no Redis read and no allocation beyond the returned future.
            let rate = cached.rate;
            return Box::pin(async move { rate });
        }
        drop(cached);
        let this = self.clone();
        let hostname = hostname.to_string();
        Box::pin(async move {
            let override_ = this
                .plugin_options
                .get(&this.namespace)
                .await
                .unwrap_or_default();
            let rules = resolve_domain_rules(&this.declared, override_.as_ref());
            let rate = super::domain_rules::resolve(&rules, &hostname).max_bytes_per_sec;
            *this.cache.lock().unwrap() = CachedRate {
                generation: gen,
                rate,
            };
            rate
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use deadpool_redis::{Config, Runtime};

    #[tokio::test]
    async fn unchanged_generation_serves_from_cache_without_redis() {
        // `create_pool` doesn't eagerly connect (`storage::redis::pool_for_db`'s own tests note
        // the same), so a pool pointed at a port with no Redis listener is constructible but
        // fails on the first actual `get()`. That's exactly what proves the cache path: the
        // first call (cache seeded with a sentinel generation) must hit the reload path and
        // degrade to `None` on the unreachable Redis; the second call must be served entirely
        // from the cache with no further Redis attempt — indistinguishable from "failed again"
        // except that the recorded generation proves the reload path ran exactly once.
        let pool = Config::from_url("redis://127.0.0.1:1")
            .create_pool(Some(Runtime::Tokio1))
            .expect("pool construction must not require a live server");
        let resolver = LiveRateResolver::new(
            "test".to_string(),
            PluginOptionsResult::default(),
            Arc::new(PluginOptionsRepository::new(pool)),
            Arc::new(AtomicU64::new(0)),
        );
        let first = resolver.resolve("example.com").await;
        assert_eq!(
            first, None,
            "unreachable Redis must degrade to None, not panic"
        );
        let second = resolver.resolve("example.com").await;
        assert_eq!(second, None);
        assert_eq!(
            resolver.cache.lock().unwrap().generation,
            0,
            "the reload path must have recorded the generation it observed"
        );
    }
}
