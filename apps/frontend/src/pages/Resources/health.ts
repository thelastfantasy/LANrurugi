import type { ResourceDisk, ResourceSnapshot } from "./types"

/** How a reading compares to the limit that matters, in three bands a non-expert can act on. */
export type Level = "ok" | "warn" | "bad" | "unknown"

export interface Reading {
  level: Level
  /** Share of the limit, 0–100. `null` when there is no limit to compare against. */
  percent: number | null
}

export function levelFor(value: number | null | undefined, warn: number, bad: number): Level {
  if (value === null || value === undefined || Number.isNaN(value)) return "unknown"
  if (value >= bad) return "bad"
  if (value >= warn) return "warn"
  return "ok"
}

export function worstLevel(levels: Level[]): Level {
  if (levels.includes("bad")) return "bad"
  if (levels.includes("warn")) return "warn"
  if (levels.length === 0 || levels.every((level) => level === "unknown")) return "unknown"
  return "ok"
}

/** `memory.current` against the cgroup ceiling. Cache counts in both, so the ratio is the honest one:
 * that is what the kernel reclaims before it kills anything. */
export function memoryReading(snapshot: ResourceSnapshot): Reading {
  const used = snapshot.cgroup?.memory_current_bytes ?? null
  const limit = snapshot.cgroup?.memory_max_bytes ?? null
  if (used === null || limit === null || limit === 0) return { level: "unknown", percent: null }
  const percent = (used / limit) * 100
  return { level: levelFor(percent, 70, 85), percent }
}

/** The snapshot reports a sum across cores; the ceiling is the machine, so divide by the core count. */
export function cpuReading(snapshot: ResourceSnapshot): Reading {
  const used = snapshot.server.cpu_percent_1s
  const cores = snapshot.runtime.cpu_cores
  if (used === null || !cores) return { level: "unknown", percent: null }
  const percent = used / cores
  return { level: levelFor(percent, 60, 85), percent }
}

/** Fullness of the volume the library lives on. Inodes run out separately from bytes. */
export function diskReading(disk: ResourceDisk | null | undefined): Reading {
  if (!disk || !disk.total_bytes) return { level: "unknown", percent: null }
  const percent = ((disk.total_bytes - disk.free_bytes) / disk.total_bytes) * 100
  return { level: levelFor(percent, 85, 93), percent }
}

/** PSI `full avg10`: the share of the last ten seconds some task spent stalled on memory. */
export function pressureReading(snapshot: ResourceSnapshot): Reading {
  const value = snapshot.cgroup?.psi_memory.full10 ?? null
  return { level: levelFor(value, 5, 10), percent: value }
}

export function overallLevel(snapshot: ResourceSnapshot): Level {
  return worstLevel([
    memoryReading(snapshot).level,
    cpuReading(snapshot).level,
    diskReading(snapshot.storage.library).level,
    pressureReading(snapshot).level,
  ])
}
