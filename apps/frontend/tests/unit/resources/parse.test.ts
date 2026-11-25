import { describe, expect, it } from "vitest"

import { normaliseSample, normaliseSnapshot } from "@/pages/Resources/parse"
import type { ResourceSample, ResourceSnapshot } from "@/pages/Resources/types"

/** A snapshot whose empty collections were dropped by `serde`'s `skip_serializing_if`, which is what
 * the server actually sends on a quiet instance. */
function sparse(): ResourceSnapshot {
  return {
    at: 1,
    monotonic_ms: 2,
    window_ms: 0,
    version: "0.0.0",
    release: "dev",
    platform: "linux/x86_64",
    build: { profile: "debug", binary: "lanrurugi-server" },
    server: {
      pid: 1,
      rss_bytes: null,
      peak_rss_bytes: null,
      swap_bytes: null,
      threads: null,
      fd_count: null,
      fd_limit: null,
      cpu_percent_1s: null,
      cpu_seconds_total: 0,
      read_bytes: null,
      write_bytes: null,
    },
    cgroup: null,
    cgroup_unavailable: "n/a",
    runtime: { cpu_cores: 4, tracked_tasks: 0, queue_depth: 0 },
    plugins: { limit: 8, transient_available: 8, transient_in_flight: 0, transient_spawned_total: 0 } as never,
    processes: { root_pid: 1, truncated: false } as never,
    models: { note: "n" } as never,
    queues: {
      download_active: 0,
      download_queued: 0,
      download_error: 0,
      jobs_active: 0,
      jobs_failed_total: 0,
      permit_wait_p50_ms: null,
    } as never,
    redis: { used_memory_bytes: null } as never,
    redis_unavailable: null,
    network: { rx_bytes: 0, tx_bytes: 0, rx_rate_bps: null, tx_rate_bps: null },
    caches: { descriptor_hits: 0, descriptor_misses: 0, metadata_hits: 0, metadata_misses: 0 },
    library: { archives: 0, total_bytes: 0 },
    storage: {
      library: { free_bytes: 0, total_bytes: 0, free_inodes: 0 },
      temp: { free_bytes: 0, total_bytes: 0, free_inodes: 0 },
    },
    gpu: null,
    gpu_unavailable: "off",
    watch: { observers: 0, samples_taken: 0, sampler_running: false },
  } as ResourceSnapshot
}

describe("resource snapshot normalisation", () => {
  it("turns dropped collections into empty ones instead of undefined", () => {
    const snapshot = normaliseSnapshot(sparse())
    expect(snapshot.plugins.processes).toEqual([])
    expect(snapshot.processes.tree).toEqual([])
    expect(snapshot.models.usage).toEqual([])
    expect(snapshot.models.in_process).toEqual([])
    expect(snapshot.queues.phase_p50_ms).toEqual({})
    expect(snapshot.queues.phase_p95_ms).toEqual({})
    expect(snapshot.redis?.keys).toEqual({})
    expect(snapshot.truncations).toEqual([])
    // The fields that were present keep their values.
    expect(snapshot.queues.download_active).toBe(0)
    expect(snapshot.build.profile).toBe("debug")
  })

  it("leaves an absent cgroup absent, with its reason", () => {
    const snapshot = normaliseSnapshot(sparse())
    expect(snapshot.cgroup).toBeNull()
    expect(snapshot.cgroup_unavailable).toBe("n/a")
  })

  it("keeps a cgroup's io_stat from being undefined", () => {
    const raw = sparse()
    raw.cgroup = { path: "/x", includes_page_cache: true, memory_current_bytes: null } as never
    expect(normaliseSnapshot(raw).cgroup?.io_stat).toEqual([])
  })

  it("fills a sample's missing counters so a chart never plots undefined", () => {
    const sample = { at: 1, monotonic_ms: 2, cpu_percent_1s: null } as unknown as ResourceSample
    const normalised = normaliseSample(sample)
    expect(normalised.tracked_tasks).toBe(0)
    expect(normalised.queue_active).toBe(0)
    expect(normalised.plugins_alive).toBe(0)
    expect(normalised.rss_bytes).toBeNull()
  })
})
