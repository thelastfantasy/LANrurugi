import { describe, expect, it } from "vitest"

import { toCsv, toText, windowLabel } from "@/pages/Settings/resources/format"
import type { ResourceSample, ResourceSnapshot } from "@/pages/Settings/resources/types"

/** The two exports are the panel's copy/paste path, so the "unknown is not zero" rule has to survive
 * serialisation too — that rule is the whole reason these numbers carry a window. */

function snapshot(): ResourceSnapshot {
  return {
    at: 1_795_410_048_000,
    monotonic_ms: 12_000,
    window_ms: 1_000,
    version: "1.0.0-beta.8",
    release: "abc1234",
    platform: "linux/x86_64",
    build: { profile: "debug", binary: "lanrurugi-server" },
    server: {
      pid: 42,
      rss_bytes: 1_500_000_000,
      peak_rss_bytes: 1_700_000_000,
      swap_bytes: 0,
      threads: 33,
      fd_count: 512,
      fd_limit: 1024,
      cpu_percent_1s: null,
      cpu_seconds_total: 1_234,
      read_bytes: 10,
      write_bytes: 20,
    },
    cgroup: {
      path: "/user.slice/x.scope",
      memory_current_bytes: 2_000_000_000,
      includes_page_cache: true,
      anon_bytes: 900_000_000,
      file_bytes: 1_000_000_000,
      slab_bytes: 12_000_000,
      peak_bytes: 2_100_000_000,
      swap_current_bytes: 0,
      pids_current: 210,
      cpu_usage_usec: 1,
      events: { low: 0, high: 0, max: 0, oom: 0, oom_kill: 2 },
      psi_memory: { some10: 1, some60: 1, full10: null, full60: null },
      io_stat: [],
    },
    runtime: { cpu_cores: 8, tracked_tasks: 1, queue_depth: 0 },
    plugins: {
      limit: 4,
      transient_available: 3,
      transient_in_flight: 1,
      transient_spawned_total: 7,
      processes: [
        {
          namespace: "metadata/chaika",
          pid: null,
          busy: true,
          rss_bytes: null,
          cpu_percent_since: null,
          started_at: null,
          last_call_at: null,
          calls: 12,
          failures: 1,
          persistent: true,
        },
      ],
    },
    processes: { root_pid: 42, tree: [], truncated: false },
    models: { usage: [], in_process: [], note: "note" },
    queues: {
      download_active: 2,
      download_queued: 3,
      download_error: 0,
      jobs_active: 1,
      jobs_failed_total: 1,
      phase_p50_ms: { transfer: 1_000 },
      phase_p95_ms: { transfer: 3_000 },
      permit_wait_p50_ms: 10,
    },
    redis: {
      used_memory_bytes: 100_000_000,
      used_memory_rss_bytes: 120_000_000,
      used_memory_peak_bytes: 130_000_000,
      mem_fragmentation_ratio: 1.2,
      connected_clients: 4,
      ops_per_sec: 900,
      keys: { db0: 389 },
      expired_keys: 0,
      evicted_keys: 0,
      keyspace_hits: 30,
      keyspace_misses: 10,
      process_id: 7,
      uptime_seconds: 3_600,
      persistence_ok: true,
      rdb_last_save_at: 1_795_410_000_000,
    },
    network: { rx_bytes: 1, tx_bytes: 2, rx_rate_bps: null, tx_rate_bps: null },
    caches: { descriptor_hits: 10, descriptor_misses: 2, metadata_hits: 5, metadata_misses: 0 },
    library: { archives: 389, total_bytes: 22_540_000_000 },
    storage: { library: null, temp: null },
    gpu: null,
    gpu_unavailable: "GPU monitoring is off",
    watch: { observers: 1, samples_taken: 12, sampler_running: true },
    truncations: [],
  } as unknown as ResourceSnapshot
}

describe("resource exports", () => {
  it("labels the window and treats a first-frame rate as unknown, not zero", () => {
    expect(windowLabel(1_000)).toBe("last 1 s")
    expect(windowLabel(1_500)).toBe("last 1.5 s")
    expect(windowLabel(0)).toBe("since start")
    const text = toText(snapshot())
    // cpu_percent_1s is null on the first frame: "—", never "0.0%".
    expect(text).toContain("cpu —")
    expect(text).toContain("window last 1 s")
  })

  it("says which window it used and marks memory.current as including page cache", () => {
    const text = toText(snapshot())
    expect(text).toContain("[debug]")
    expect(text).toContain("includes page cache")
    expect(text).toContain("oom_kill 2")
    expect(text).toContain("metadata/chaika pid busy")
  })

  it("keeps an unknown plugin RSS out of the text export instead of printing 0 B", () => {
    expect(toText(snapshot())).toContain("rss —")
  })

  it("writes one CSV row per sample with a header, blanks for unknowns", () => {
    const sample: ResourceSample = {
      at: 1,
      monotonic_ms: 1_000,
      cpu_percent_1s: null,
      rss_bytes: 1_500_000_000,
      anon_bytes: 900_000_000,
      file_bytes: null,
      pids: 210,
      fd_count: 512,
      psi_memory_full10: null,
      tracked_tasks: 1,
      queue_active: 2,
      queue_queued: 3,
      plugins_alive: 1,
    }
    const lines = toCsv([sample]).split("\n")
    expect(lines).toHaveLength(2)
    expect(lines[0].startsWith("at,monotonic_ms,cpu_percent_1s")).toBe(true)
    expect(lines[1]).toBe("1,1000,,1500000000,900000000,,210,512,,1,2,3,1")
  })
})
