import type { ResourceSample, ResourceSnapshot } from "./types"

/** Fill in the collections the server omits when they are empty.
 *
 * `serde`'s `skip_serializing_if` drops an empty `Vec`/`BTreeMap`, which is right for the wire (a
 * quiet server should not pay for `"usage": []`) but wrong for a renderer: `Object.entries(undefined)`
 * throws, and a snapshot that blanks the whole settings page is worse than one that shows an empty
 * table. Anything absent here is therefore "measured, and there was nothing", which is exactly what
 * an empty collection means anyway. */
export function normaliseSnapshot(raw: ResourceSnapshot): ResourceSnapshot {
  const rawCgroup = raw.cgroup as (ResourceSnapshot["cgroup"] & { io_stat?: unknown }) | null
  return {
    ...raw,
    cgroup: rawCgroup ? { ...rawCgroup, io_stat: rawCgroup.io_stat ?? [] } : null,
    plugins: { ...raw.plugins, processes: raw.plugins.processes ?? [] },
    processes: { ...raw.processes, tree: raw.processes.tree ?? [] },
    models: {
      ...raw.models,
      usage: raw.models.usage ?? [],
      in_process: raw.models.in_process ?? [],
    },
    queues: {
      ...raw.queues,
      phase_p50_ms: raw.queues.phase_p50_ms ?? {},
      phase_p95_ms: raw.queues.phase_p95_ms ?? {},
    },
    redis: raw.redis ? { ...raw.redis, keys: raw.redis.keys ?? {} } : null,
    truncations: raw.truncations ?? [],
  }
}

/** A `sample` frame is flat and every field is either a number or `null`, so the only work is
 * keeping a missing key from becoming `undefined` in a chart's `Math`. */
export function normaliseSample(raw: ResourceSample): ResourceSample {
  return {
    ...raw,
    cpu_percent_1s: raw.cpu_percent_1s ?? null,
    rss_bytes: raw.rss_bytes ?? null,
    anon_bytes: raw.anon_bytes ?? null,
    file_bytes: raw.file_bytes ?? null,
    pids: raw.pids ?? null,
    fd_count: raw.fd_count ?? null,
    psi_memory_full10: raw.psi_memory_full10 ?? null,
    tracked_tasks: raw.tracked_tasks ?? 0,
    queue_active: raw.queue_active ?? 0,
    queue_queued: raw.queue_queued ?? 0,
    plugins_alive: raw.plugins_alive ?? 0,
    memory_current_bytes: raw.memory_current_bytes ?? null,
    memory_max_bytes: raw.memory_max_bytes ?? null,
    disk_free_bytes: raw.disk_free_bytes ?? null,
    disk_total_bytes: raw.disk_total_bytes ?? null,
    rx_bytes: raw.rx_bytes ?? null,
    tx_bytes: raw.tx_bytes ?? null,
    rx_rate_bps: raw.rx_rate_bps ?? null,
    tx_rate_bps: raw.tx_rate_bps ?? null,
    io_read_bytes: raw.io_read_bytes ?? 0,
    io_write_bytes: raw.io_write_bytes ?? 0,
  }
}

/** Folds a one-second sample onto the snapshot the stream opened with, so the numbers a reader
 * watches (tiles included) move instead of freezing on the first frame. Fields the sample does not
 * carry keep their snapshot value. */
export function applySample(snapshot: ResourceSnapshot, sample: ResourceSample): ResourceSnapshot {
  const cgroup = snapshot.cgroup
  return {
    ...snapshot,
    at: sample.at,
    server: {
      ...snapshot.server,
      cpu_percent_1s: sample.cpu_percent_1s,
      rss_bytes: sample.rss_bytes,
      fd_count: sample.fd_count,
      read_bytes: sample.io_read_bytes ?? null,
      write_bytes: sample.io_write_bytes ?? null,
    },
    cgroup: cgroup
      ? {
          ...cgroup,
          memory_current_bytes: sample.memory_current_bytes ?? cgroup.memory_current_bytes,
          memory_max_bytes: sample.memory_max_bytes ?? cgroup.memory_max_bytes,
          anon_bytes: sample.anon_bytes,
          file_bytes: sample.file_bytes,
          pids_current: sample.pids,
          psi_memory: { ...cgroup.psi_memory, full10: sample.psi_memory_full10 ?? cgroup.psi_memory.full10 },
        }
      : cgroup,
    runtime: { ...snapshot.runtime, tracked_tasks: sample.tracked_tasks },
    queues: {
      ...snapshot.queues,
      download_active: sample.queue_active,
      download_queued: sample.queue_queued,
    },
    network: {
      rx_bytes: sample.rx_bytes ?? snapshot.network.rx_bytes,
      tx_bytes: sample.tx_bytes ?? snapshot.network.tx_bytes,
      rx_rate_bps: sample.rx_rate_bps ?? null,
      tx_rate_bps: sample.tx_rate_bps ?? null,
    },
    watch: { ...snapshot.watch, samples_taken: snapshot.watch.samples_taken + 1 },
    storage: snapshot.storage.library
      ? {
          ...snapshot.storage,
          library: {
            ...snapshot.storage.library,
            free_bytes: sample.disk_free_bytes ?? snapshot.storage.library.free_bytes,
            total_bytes: sample.disk_total_bytes ?? snapshot.storage.library.total_bytes,
          },
        }
      : snapshot.storage,
  }
}
