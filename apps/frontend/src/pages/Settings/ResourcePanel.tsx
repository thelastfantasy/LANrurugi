import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { CollapsibleSection } from "@/components/Display"
import { note, setResourceContext } from "@/diagnostics/sdk"
import { formatBytes } from "@/lib/formatBytes"
import { FONT_SIZE_SM } from "@/theme"
import { toast } from "@/toast"

import {
  hitRatio,
  labelWithProfile,
  milliseconds,
  percent,
  rate,
  seriesPoints,
  toCsv,
  toText,
} from "./resources/format"
import { Sparkline } from "./resources/Sparkline"
import type {
  ResourceDisk,
  ResourcePluginProcess,
  ResourceProcessEntry,
  ResourceSnapshot,
} from "./resources/types"
import { useResourceStream } from "./resources/useResourceStream"

/** Settings → resources (`specs/030-resource-monitor/` US2).
 *
 * The section owns the subscription, and the tables only exist while it is open: closing it drops the
 * last observer, and the server stops sampling. That is the whole "zero cost when nobody is looking"
 * claim, and it is enforced by unmounting rather than by a timer. */
export function ResourcePanel() {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)

  return (
    <CollapsibleSection
      id="resources"
      icon="fa-gauge-high"
      title={t("settings.resources.title")}
      open={open}
      onOpenChange={setOpen}
    >
      {open ? <ResourceBody /> : null}
    </CollapsibleSection>
  )
}

const INTERVALS = [
  { ms: 1000, key: "settings.resources.intervalOne" },
  { ms: 2000, key: "settings.resources.intervalTwo" },
  { ms: 5000, key: "settings.resources.intervalFive" },
] as const

function ResourceBody() {
  const { t } = useTranslation()
  const [intervalMs, setIntervalMs] = useState<number>(1000)
  const { snapshot, samples, status, error } = useResourceStream(intervalMs)

  // While the panel is open, its readings ride along with anything reported to the tracker
  // (FR-016): a breadcrumb every ten seconds, and the newest sample as the `resources` context so an
  // error carries the machine's state rather than a guess. Unmounting clears the context again.
  const lastNoted = useRef(0)
  useEffect(() => {
    const newest = samples.at(-1)
    if (!newest) return
    setResourceContext(newest as unknown as Record<string, unknown>)
    const now = Date.now()
    if (now - lastNoted.current < 10_000) return
    lastNoted.current = now
    note("resource usage", newest as unknown as Record<string, unknown>)
  }, [samples])
  useEffect(() => () => setResourceContext(null), [])

  const rssSeries = useMemo(() => seriesPoints(samples.map((sample) => sample.rss_bytes)), [samples])
  const cpuSeries = useMemo(() => seriesPoints(samples.map((sample) => sample.cpu_percent_1s)), [samples])
  const psiSeries = useMemo(
    () => seriesPoints(samples.map((sample) => sample.psi_memory_full10)),
    [samples],
  )

  return (
    <div className="resources-panel" style={{ fontSize: FONT_SIZE_SM }}>
      <div className="resources-head">
        <span className={`resources-status resources-status-${status}`}>
          {t(`settings.resources.status.${status}`)}
        </span>
        <span>{t("settings.resources.description")}</span>
        <span className="resources-export">
          <button
            type="button"
            className="stdbtn"
            disabled={!snapshot}
            onClick={() => {
              if (!snapshot) return
              void navigator.clipboard
                .writeText(toText(snapshot))
                .then(() => toast({ text: t("settings.resources.copied"), icon: "success" }))
                .catch(() => toast({ text: t("settings.resources.copyFailed"), icon: "error" }))
            }}
          >
            {t("settings.resources.copyText")}
          </button>
          <button
            type="button"
            className="stdbtn"
            disabled={samples.length === 0}
            onClick={() => {
              const url = URL.createObjectURL(new Blob([toCsv(samples)], { type: "text/csv" }))
              const link = document.createElement("a")
              link.href = url
              link.download = "lanrurugi-resources.csv"
              link.click()
              URL.revokeObjectURL(url)
            }}
          >
            {t("settings.resources.exportCsv")}
          </button>
        </span>
        <span className="resources-intervals">
          {INTERVALS.map((option) => (
            <button
              key={option.ms}
              type="button"
              className={`stdbtn${intervalMs === option.ms ? " active" : ""}`}
              onClick={() => setIntervalMs(option.ms)}
            >
              {t(option.key)}
            </button>
          ))}
        </span>
      </div>

      {error ? <div className="resources-alert">{error}</div> : null}
      {!snapshot ? (
        <div className="resources-waiting">{t("settings.resources.waiting")}</div>
      ) : (
        <SnapshotTables snapshot={snapshot} rssSeries={rssSeries} cpuSeries={cpuSeries} psiSeries={psiSeries} />
      )}
    </div>
  )
}

function SnapshotTables({
  snapshot,
  rssSeries,
  cpuSeries,
  psiSeries,
}: {
  snapshot: ResourceSnapshot
  rssSeries: number[]
  cpuSeries: number[]
  psiSeries: number[]
}) {
  const { t } = useTranslation()
  const server = snapshot.server
  const cgroup = snapshot.cgroup
  const redis = snapshot.redis
  const gpu = snapshot.gpu
  const fdRatio = server.fd_count !== null && server.fd_limit ? (server.fd_count / server.fd_limit) * 100 : null
  const descriptorRatio = hitRatio(snapshot.caches.descriptor_hits, snapshot.caches.descriptor_misses)
  const metadataRatio = hitRatio(snapshot.caches.metadata_hits, snapshot.caches.metadata_misses)
  const redisRatio =
    redis && redis.keyspace_hits !== null && redis.keyspace_misses !== null
      ? hitRatio(redis.keyspace_hits, redis.keyspace_misses)
      : null
  const oomKills = cgroup?.events.oom_kill ?? null

  return (
    <>
      <Group title={t("settings.resources.server")}>
        <Row
          label={labelWithProfile("lanrurugi-server", snapshot.build.profile)}
          value={`pid ${server.pid}`}
        />
        <Row
          label={t("settings.resources.rss")}
          value={bytes(server.rss_bytes)}
          sparkline={rssSeries.length > 1 ? <Sparkline values={rssSeries} label={t("settings.resources.rss")} /> : null}
        />
        <Row label={t("settings.resources.peakRss")} value={bytes(server.peak_rss_bytes)} />
        <Row label={t("settings.resources.swap")} value={bytes(server.swap_bytes)} />
        <Row
          label={t("settings.resources.cpu")}
          value={percent(server.cpu_percent_1s) ?? missing}
          sparkline={
            cpuSeries.length > 1 ? <Sparkline values={cpuSeries} label={t("settings.resources.cpu")} /> : null
          }
        />
        <Row label={t("settings.resources.threads")} value={count(server.threads)} />
        <Row
          label={t("settings.resources.fds")}
          value={
            server.fd_count === null || server.fd_limit === null
              ? missing
              : `${server.fd_count} / ${server.fd_limit} · ${percent(fdRatio) ?? missing}`
          }
          alert={fdRatio !== null && fdRatio > 80}
        />
        <Row
          label={t("settings.resources.readWrite")}
          value={`${bytes(server.read_bytes)} / ${bytes(server.write_bytes)}`}
        />
      </Group>

      <Group title={t("settings.resources.cgroup")}>
        {!cgroup ? (
          <Row label={t("settings.resources.cgroup")} value={snapshot.cgroup_unavailable ?? missing} />
        ) : (
          <>
            <Row
              label={t("settings.resources.memoryCurrent")}
              value={`${bytes(cgroup.memory_current_bytes)} · ${t("settings.resources.pageCacheNote")}`}
            />
            <Row label={t("settings.resources.memoryAnon")} value={bytes(cgroup.anon_bytes)} />
            <Row label={t("settings.resources.memoryFile")} value={bytes(cgroup.file_bytes)} />
            <Row label={t("settings.resources.memorySlab")} value={bytes(cgroup.slab_bytes)} />
            <Row label={t("settings.resources.memoryPeak")} value={bytes(cgroup.peak_bytes)} />
            <Row label={t("settings.resources.memorySwap")} value={bytes(cgroup.swap_current_bytes)} />
            <Row label={t("settings.resources.pids")} value={count(cgroup.pids_current)} />
            <Row
              label={t("settings.resources.psi")}
              value={percent(cgroup.psi_memory.full10) ?? missing}
              hint={t("settings.resources.psiHint")}
              alert={(cgroup.psi_memory.full10 ?? 0) > 10}
              sparkline={
                psiSeries.length > 1 ? (
                  <Sparkline values={psiSeries} label={t("settings.resources.psi")} />
                ) : null
              }
            />
            <Row
              label={t("settings.resources.memoryEvents")}
              value={`high ${count(cgroup.events.high)} · max ${count(cgroup.events.max)} · oom ${count(
                cgroup.events.oom,
              )} · oom_kill ${count(oomKills)}`}
              alert={(oomKills ?? 0) > 0}
            />
          </>
        )}
      </Group>

      <Group title={t("settings.resources.runtime")}>
        <Row label={t("settings.resources.cores")} value={String(snapshot.runtime.cpu_cores)} />
        <Row label={t("settings.resources.tasks")} value={String(snapshot.runtime.tracked_tasks)} />
        <Row label={t("settings.resources.queueDepth")} value={String(snapshot.runtime.queue_depth)} />
      </Group>

      <Group title={t("settings.resources.plugins")}>
        <Row
          label={t("settings.resources.persistentWorkers")}
          value={`${snapshot.plugins.processes.length} / ${snapshot.plugins.limit}`}
        />
        <Row
          label={t("settings.resources.transientWorkers")}
          value={t("settings.resources.transientValue", {
            available: snapshot.plugins.transient_available,
            inFlight: snapshot.plugins.transient_in_flight,
            spawned: snapshot.plugins.transient_spawned_total,
          })}
          hint={t("settings.resources.transientHint")}
        />
        {snapshot.plugins.processes.map((process) => (
          <PluginRow key={process.namespace} process={process} />
        ))}
      </Group>

      <Group title={t("settings.resources.processTree")}>
        {snapshot.processes.tree.map((entry) => (
          <ProcessRow key={entry.pid} entry={entry} />
        ))}
        {snapshot.processes.truncated ? (
          <Row label={t("settings.resources.truncated")} value={String(snapshot.processes.tree.length)} />
        ) : null}
      </Group>

      <Group title={t("settings.resources.models")}>
        {snapshot.models.usage.length === 0 ? (
          <Row label={t("settings.resources.models")} value={t("settings.resources.noCallsToday")} />
        ) : (
          snapshot.models.usage.map((usage) => (
            <Row
              key={usage.feature}
              label={usage.feature}
              value={t("settings.resources.modelUsage", {
                requests: usage.requests,
                failures: usage.failures,
                tokens: usage.tokens_in + usage.tokens_out,
              })}
            />
          ))
        )}
        <Row label={t("settings.resources.inProcess")} value={snapshot.models.note} />
      </Group>

      <Group title={t("settings.resources.queues")}>
        <Row
          label={t("settings.resources.downloads")}
          value={t("settings.resources.downloadValue", {
            active: snapshot.queues.download_active,
            queued: snapshot.queues.download_queued,
            errors: snapshot.queues.download_error,
          })}
        />
        <Row
          label={t("settings.resources.jobs")}
          value={t("settings.resources.jobsValue", {
            active: snapshot.queues.jobs_active,
            failed: snapshot.queues.jobs_failed_total,
          })}
        />
        <Row
          label={t("settings.resources.phases")}
          value={Object.entries(snapshot.queues.phase_p50_ms)
            .map(([name, value]) => `${name} ${milliseconds(value) ?? missing}`)
            .join(" · ") || missing}
          hint={t("settings.resources.phasesHint")}
        />
        <Row label={t("settings.resources.permitWait")} value={milliseconds(snapshot.queues.permit_wait_p50_ms) ?? missing} />
      </Group>

      <Group title={t("settings.resources.redis")}>
        {!redis ? (
          <Row label={t("settings.resources.redis")} value={snapshot.redis_unavailable ?? missing} />
        ) : (
          <>
            <Row label={t("settings.resources.redisMemory")} value={`${bytes(redis.used_memory_bytes)} · RSS ${bytes(redis.used_memory_rss_bytes)}`} />
            <Row label={t("settings.resources.redisPeak")} value={bytes(redis.used_memory_peak_bytes)} />
            <Row
              label={t("settings.resources.redisFragmentation")}
              value={redis.mem_fragmentation_ratio === null ? missing : redis.mem_fragmentation_ratio.toFixed(2)}
            />
            <Row label={t("settings.resources.redisClients")} value={count(redis.connected_clients)} />
            <Row label={t("settings.resources.redisOps")} value={count(redis.ops_per_sec)} />
            <Row
              label={t("settings.resources.redisKeys")}
              value={Object.entries(redis.keys)
                .map(([db, keys]) => `${db}: ${keys}`)
                .join(" · ") || missing}
            />
            <Row
              label={t("settings.resources.redisHitRate")}
              value={percent(redisRatio) ?? missing}
              hint={t("settings.resources.redisHitHint")}
            />
            <Row
              label={t("settings.resources.redisPersistence")}
              value={redis.persistence_ok === null ? missing : redis.persistence_ok ? t("settings.resources.ok") : t("settings.resources.failed")}
              alert={redis.persistence_ok === false}
            />
            <Row label={t("settings.resources.redisEvicted")} value={count(redis.evicted_keys)} />
          </>
        )}
      </Group>

      <Group title={t("settings.resources.network")}>
        <Row
          label={t("settings.resources.network")}
          value={`↓ ${bytes(snapshot.network.rx_bytes)} (${rate(snapshot.network.rx_rate_bps) ?? missing}) · ↑ ${bytes(
            snapshot.network.tx_bytes,
          )} (${rate(snapshot.network.tx_rate_bps) ?? missing})`}
        />
      </Group>

      <Group title={t("settings.resources.caches")}>
        <Row
          label={t("settings.resources.descriptorCache")}
          value={percent(descriptorRatio) ?? missing}
          hint={t("settings.resources.descriptorCacheHint")}
        />
        <Row
          label={t("settings.resources.metadataCache")}
          value={percent(metadataRatio) ?? missing}
          hint={t("settings.resources.metadataCacheHint")}
        />
      </Group>

      <Group title={t("settings.resources.library")}>
        <Row
          label={t("settings.resources.archives")}
          value={`${snapshot.library.archives} · ${bytes(snapshot.library.total_bytes)}`}
        />
        <DiskRow label={t("settings.resources.storageLibrary")} disk={snapshot.storage.library} />
        <DiskRow label={t("settings.resources.storageTemp")} disk={snapshot.storage.temp} />
      </Group>

      <Group title={t("settings.resources.gpu")}>
        {!gpu ? (
          <Row label={t("settings.resources.gpu")} value={snapshot.gpu_unavailable ?? missing} />
        ) : (
          <>
            <Row label={t("settings.resources.gpuVendor")} value={gpu.vendor} />
            <Row label={t("settings.resources.gpuUtilisation")} value={percent(gpu.utilization_percent) ?? missing} />
            <Row
              label={t("settings.resources.gpuMemory")}
              value={`${bytes(gpu.memory_used_bytes)} / ${bytes(gpu.memory_total_bytes)}`}
            />
            <Row
              label={t("settings.resources.gpuTemperature")}
              value={gpu.temperature_c === null ? missing : `${gpu.temperature_c.toFixed(0)} °C`}
            />
          </>
        )}
      </Group>

      <Group title={t("settings.resources.watch")}>
        <Row label={t("settings.resources.observers")} value={String(snapshot.watch.observers)} />
        <Row label={t("settings.resources.samples")} value={String(snapshot.watch.samples_taken)} />
        <Row
          label={t("settings.resources.sampler")}
          value={snapshot.watch.sampler_running ? t("settings.resources.running") : t("settings.resources.stopped")}
        />
        <Row label={t("settings.resources.version")} value={`${snapshot.version} · ${snapshot.release} · ${snapshot.platform}`} />
        {snapshot.truncations.length > 0 ? (
          <Row label={t("settings.resources.truncated")} value={snapshot.truncations.join(" · ")} />
        ) : null}
      </Group>
    </>
  )
}

const missing = "—"

function bytes(value: number | null | undefined): string {
  if (value === null || value === undefined) return missing
  return formatBytes(value)
}

function count(value: number | null | undefined): string {
  if (value === null || value === undefined) return missing
  return String(value)
}

function Group({ title, children }: { title: string; children: React.ReactNode }) {
  return (
    <div className="resources-group">
      <div className="resources-group-title">{title}</div>
      <table className="resources-table">
        <tbody>{children}</tbody>
      </table>
    </div>
  )
}

function Row({
  label,
  value,
  hint,
  alert,
  sparkline,
}: {
  label: string
  value: string | null
  hint?: string
  alert?: boolean
  sparkline?: React.ReactNode
}) {
  return (
    <tr className={alert ? "resources-alert" : undefined}>
      <th scope="row" title={hint}>
        {label}
        {hint ? <span className="resources-hint"> ?</span> : null}
      </th>
      <td>{value === null ? missing : value}</td>
      <td className="resources-spark">{sparkline ?? null}</td>
    </tr>
  )
}

function PluginRow({ process }: { process: ResourcePluginProcess }) {
  const { t } = useTranslation()
  return (
    <Row
      label={`${process.namespace}${process.busy ? " · busy" : ""}`}
      value={t("settings.resources.pluginValue", {
        pid: process.pid ?? missing,
        rss: bytes(process.rss_bytes),
        calls: process.calls,
        failures: process.failures,
      })}
      alert={process.failures > 0 && process.calls > 0 && process.failures / process.calls > 0.2}
    />
  )
}

function ProcessRow({ entry }: { entry: ResourceProcessEntry }) {
  return (
    <Row
      label={`${"· ".repeat(entry.depth)}${entry.comm}`}
      value={`pid ${entry.pid} · ${bytes(entry.rss_bytes)} · ${percent(entry.cpu_percent) ?? missing}`}
    />
  )
}

function DiskRow({ label, disk }: { label: string; disk: ResourceDisk }) {
  const used = disk.total_bytes - disk.free_bytes
  const ratio = disk.total_bytes > 0 ? (used / disk.total_bytes) * 100 : null
  return (
    <Row
      label={label}
      value={`${bytes(used)} / ${bytes(disk.total_bytes)} · ${percent(ratio) ?? missing} · ${disk.free_inodes} inodes`}
      alert={ratio !== null && ratio > 90}
    />
  )
}
