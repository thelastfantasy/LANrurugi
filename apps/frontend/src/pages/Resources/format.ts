import { formatBytes } from "@/lib/formatBytes"

import type { ResourceSample, ResourceSnapshot } from "./types"

/** Display helpers for the resource panel. Pure and unit-tested, because "which unit does 1.5 MB/s
 * become" is exactly the kind of thing a wrong reading hides for months. */

/** A `null` measurement is rendered by the caller as "—" with the reason in a hint; this returns
 * `null` so that choice stays visible at the call site instead of being baked into a string. */
export function percent(value: number | null | undefined, digits = 1): string | null {
  if (value === null || value === undefined || !Number.isFinite(value)) return null
  return `${value.toFixed(digits)}%`
}

/** A rate over the panel's observation window. `null` on the first sample, when there is no window
 * to divide by yet — the server never reports a fabricated rate, and neither does this. */
export function rate(bytesPerSecond: number | null | undefined): string | null {
  if (bytesPerSecond === null || bytesPerSecond === undefined || !Number.isFinite(bytesPerSecond)) {
    return null
  }
  return `${formatBytes(bytesPerSecond)}/s`
}

export function duration(seconds: number | null | undefined): string | null {
  if (seconds === null || seconds === undefined || !Number.isFinite(seconds) || seconds < 0) {
    return null
  }
  const total = Math.round(seconds)
  const days = Math.floor(total / 86_400)
  const hours = Math.floor((total % 86_400) / 3_600)
  const minutes = Math.floor((total % 3_600) / 60)
  if (days > 0) return `${days}d ${hours}h`
  if (hours > 0) return `${hours}h ${minutes}m`
  if (minutes > 0) return `${minutes}m ${total % 60}s`
  return `${total}s`
}

export function milliseconds(value: number | null | undefined): string | null {
  if (value === null || value === undefined || !Number.isFinite(value)) return null
  if (value >= 1_000) return `${(value / 1_000).toFixed(1)} s`
  return `${Math.round(value)} ms`
}

/** Hit share of `hits / (hits + misses)`, or `null` when nothing was asked yet — a 0% cache with no
 * lookups is not a broken cache, it is an untouched one. */
export function hitRatio(hits: number, misses: number): number | null {
  const total = hits + misses
  if (!Number.isFinite(total) || total <= 0) return null
  return (hits / total) * 100
}

/** FR-017: a debug build's processes are labelled, because "lanrurugi-server using 1.6 GB" reads very
 * differently when that binary is an unoptimised debug build. `deno` is never labelled — it is a
 * separate program, not a build of ours. */
export function labelWithProfile(name: string, profile: string): string {
  return profile === "debug" ? `${name} (debug)` : name
}

/** Newest value of a series, and the trend the sparkline draws, in one pass. */
export function seriesPoints(values: (number | null)[]): number[] {
  return values.filter((value): value is number => value !== null && Number.isFinite(value))
}

/** A byte count that may be unknowable: `—` is not 0 B, and the export must preserve that. */
function bytes(value: number | null | undefined): string {
  return value === null || value === undefined || !Number.isFinite(value) ? "—" : formatBytes(value)
}

/** "过去 1 s" vs "自本进程启动": a rate needs a window, and the panel says which window it used. */
export function windowLabel(windowMs: number): string {
  if (!Number.isFinite(windowMs) || windowMs <= 0) return "since start"
  return `last ${windowMs >= 1000 ? `${(windowMs / 1000).toFixed(windowMs % 1000 === 0 ? 0 : 1)} s` : `${windowMs} ms`}`
}

/** The plain-text export (US5): one line per reading, each carrying its window and source, because a
 * number pasted into an issue without those two is exactly what makes triage slow. */
export function toText(snapshot: ResourceSnapshot): string {
  const lines: string[] = []
  const window = windowLabel(snapshot.window_ms)
  lines.push(`LANrurugi ${snapshot.version} (${snapshot.release}) ${snapshot.platform} [${snapshot.build.profile}]`)
  lines.push(`at ${new Date(snapshot.at).toISOString()}, window ${window}`)
  const server = snapshot.server
  lines.push(`server pid ${server.pid ?? "?"}: rss ${bytes(server.rss_bytes)} (peak ${bytes(server.peak_rss_bytes)}), swap ${bytes(server.swap_bytes)}, cpu ${percent(server.cpu_percent_1s) ?? "—"}, threads ${server.threads ?? "?"}, fds ${server.fd_count ?? "?"}/${server.fd_limit ?? "?"}`)
  if (snapshot.cgroup) {
    const c = snapshot.cgroup
    lines.push(`cgroup: current ${bytes(c.memory_current_bytes)} (includes page cache), anon ${bytes(c.anon_bytes)}, file ${bytes(c.file_bytes)}, slab ${bytes(c.slab_bytes)}, peak ${bytes(c.peak_bytes)}, pids ${c.pids_current ?? "?"}`)
    lines.push(`cgroup psi memory full10 ${percent(c.psi_memory.full10) ?? "—"}, oom ${c.events.oom}, oom_kill ${c.events.oom_kill}`)
  } else if (snapshot.cgroup_unavailable) {
    lines.push(`cgroup: ${snapshot.cgroup_unavailable}`)
  }
  if (snapshot.redis) {
    const r = snapshot.redis
    lines.push(`redis: used ${bytes(r.used_memory_bytes)} (rss ${bytes(r.used_memory_rss_bytes)}), clients ${r.connected_clients ?? "?"}, ops/s ${r.ops_per_sec ?? "?"}, hit rate ${percent(hitRatio(r.keyspace_hits ?? 0, r.keyspace_misses ?? 0)) ?? "—"}, evicted ${r.evicted_keys}`)
  } else if (snapshot.redis_unavailable) {
    lines.push(`redis: ${snapshot.redis_unavailable}`)
  }
  lines.push(`plugins: ${snapshot.plugins.processes.length} persistent, ${snapshot.plugins.transient_in_flight}/${snapshot.plugins.transient_available} transient in flight`)
  for (const plugin of snapshot.plugins.processes) {
    lines.push(`  ${plugin.namespace} pid ${plugin.pid ?? "busy"} rss ${plugin.rss_bytes === null ? "—" : bytes(plugin.rss_bytes)} calls ${plugin.calls} failures ${plugin.failures}`)
  }
  const queue = snapshot.queues
  lines.push(`queue: downloads ${queue.download_active} active / ${queue.download_queued} queued / ${queue.download_error} error, jobs ${queue.jobs_active} active / ${queue.jobs_failed_total} failed`)
  lines.push(`library: ${snapshot.library.archives} archives, ${bytes(snapshot.library.total_bytes)}`)
  if (snapshot.truncations.length > 0) lines.push(`truncated: ${snapshot.truncations.join(", ")}`)
  return lines.join("\n")
}

/** The CSV export: one row per sample, for pasting into a spreadsheet. */
export function toCsv(samples: ResourceSample[]): string {
  const header = "at,monotonic_ms,cpu_percent_1s,rss_bytes,anon_bytes,file_bytes,pids,fd_count,psi_memory_full10,tracked_tasks,queue_active,queue_queued,plugins_alive"
  const rows = samples.map((s) =>
    [s.at, s.monotonic_ms, s.cpu_percent_1s, s.rss_bytes, s.anon_bytes, s.file_bytes, s.pids, s.fd_count, s.psi_memory_full10, s.tracked_tasks, s.queue_active, s.queue_queued, s.plugins_alive]
      .map((value) => (value === null ? "" : String(value)))
      .join(","),
  )
  return [header, ...rows].join("\n")
}
