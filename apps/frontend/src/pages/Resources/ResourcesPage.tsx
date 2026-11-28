import { useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { FaCircleQuestion } from "react-icons/fa6"

import { Tooltip } from "@/components/common-ui/Display/Tooltip"
import { note, setResourceContext } from "@/diagnostics/sdk"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { formatBytes } from "@/lib/formatBytes"
import { FONT_SIZE_SM, useApplyTheme } from "@/theme"
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
} from "./format"
import {
  cpuReading,
  diskReading,
  type Level,
  levelFor,
  memoryReading,
  pressureReading,
} from "./health"
import { Sparkline } from "./Sparkline"
import type {
  ResourceDisk,
  ResourcePluginProcess,
  ResourceProcessEntry,
  ResourceSnapshot,
} from "./types"
import { useResourceStream } from "./useResourceStream"

/** The resource monitor page (`specs/030-resource-monitor/` US2).
 *
 * The page owns the subscription, and its tables only exist while it is mounted: leaving the route
 * drops the last observer and the server stops sampling. That is the whole "zero cost when nobody is
 * looking" claim, and it is enforced by unmounting rather than by a timer. */
export function Resources() {
  const { t } = useTranslation()
  useApplyTheme()
  useDocumentTitle(t("settings.resources.title") ?? undefined)

  return (
    <div className="ido" style={{ paddingLeft: 12, paddingRight: 12 }}>
      <h1 className="ih">{t("settings.resources.title")}</h1>
      <p style={{ fontSize: FONT_SIZE_SM }}>{t("settings.resources.description")}</p>
      <ResourceBody />
    </div>
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
  // Reasons come from the server as `code` or `code: detail`, so the sentence a user reads is
  // translated here instead of shipped as English prose from the backend.
  // `truncations` carries codes like `plugins.processes`; dots would read as an i18next path
  // separator, so the key uses underscores and an unknown code falls back to itself.
  const phaseLabel = (name: string): string => {
    const key = `settings.resources.phase.${name}`
    const translated = t(key)
    return translated === key ? name : translated
  }

  // Feature ids like `artist_backfill` are the usage ledger's own keys; the panel shows a name.
  const featureLabel = (feature: string): string => {
    const key = `settings.resources.feature.${feature}`
    const translated = t(key)
    return translated === key ? feature : translated
  }

  const truncationLabel = (code: string): string => {
    const key = `settings.resources.truncatedBy.${code.replace(/\./g, "_")}`
    const translated = t(key)
    return translated === key ? code : translated
  }
  const io = snapshot.cgroup?.io_stat ?? []
  const ioRead = io.reduce((sum, entry) => sum + entry.read_bytes, 0)
  const ioWrite = io.reduce((sum, entry) => sum + entry.write_bytes, 0)

  // "off" and "tried and failed" read the same in the value cell; only "off" has a how-to.
  const gpuOff = /^(disabled|GPU monitoring is off)/.test(snapshot.gpu_unavailable ?? "")

  const reason = (raw: string | null | undefined): string => {
    if (!raw) return missing
    // Servers before 030 shipped these reasons as English prose; map the old wording so a page
    // talking to an older backend still reads in the user's language.
    const legacy: Array<[RegExp, string]> = [
      [/^GPU monitoring is off/, "disabled"],
      [/^no GPU counters on this host/, "no_device"],
      [/^GPU probe failed/, "probe_failed"],
      [/no cgroup v2 hierarchy/, "no_cgroup"],
      [/is not readable$/, "unreadable"],
      [/^the snapshot's reader failed/, "reader_failed"],
      [/^Redis INFO unavailable/, "info_failed"],
    ]
    const mapped = legacy.find(([pattern]) => pattern.test(raw))
    const [code, ...rest] = (mapped ? mapped[1] : raw).split(":")
    const key = `settings.resources.unavailable.${code.trim()}`
    const translated = t(key)
    const label = translated === key ? code.trim() : translated
    const detail = mapped ? "" : rest.join(":").trim()
    return detail ? `${label} · ${detail}` : label
  }

  return (
    <div className="resources-stack">
      <Overview snapshot={snapshot} />
      <div className="resources-grid">
      <Group title={t("settings.resources.server")}>
        <Row
          label={labelWithProfile("lanrurugi-server", snapshot.build.profile)}
          value={`pid ${server.pid}`}
        />
        <Row
          label={t("settings.resources.rss")}
          value={bytes(server.rss_bytes)}
          sparkline={rssSeries.length > 1 ? <Sparkline values={rssSeries} label={t("settings.resources.rss")} width={78} height={20} /> : null}
        />
        <Row label={t("settings.resources.peakRss")} value={bytes(server.peak_rss_bytes)} />
        <Row label={t("settings.resources.swap")} value={bytes(server.swap_bytes)} />
        <Row
          label={t("settings.resources.cpu")}
          value={percent(server.cpu_percent_1s) ?? missing}
          hint={t("settings.resources.cpuHint")}
          sparkline={
            cpuSeries.length > 1 ? <Sparkline values={cpuSeries} label={t("settings.resources.cpu")} width={78} height={20} /> : null
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
          level={levelFor(fdRatio, 80, 95)}
        />
        <Row
          label={t("settings.resources.readWrite")}
          value={`${bytes(server.read_bytes)} / ${bytes(server.write_bytes)}`}
        />
      </Group>

      <Group title={t("settings.resources.cgroup")}>
        {!cgroup ? (
          <Row label={t("settings.resources.cgroup")} value={reason(snapshot.cgroup_unavailable)} />
        ) : (
          <>
            <Row
              label={t("settings.resources.memoryCurrent")}
              value={`${bytes(cgroup.memory_current_bytes)} · ${t("settings.resources.pageCacheNote")}`}
              level={memoryReading(snapshot).level}
            />
            <Row label={t("settings.resources.memoryAnon")} value={bytes(cgroup.anon_bytes)} />
            <Row label={t("settings.resources.memoryFile")} value={bytes(cgroup.file_bytes)} />
            <Row label={t("settings.resources.memorySlab")} value={bytes(cgroup.slab_bytes)} />
            <Row label={t("settings.resources.memoryPeak")} value={bytes(cgroup.peak_bytes)} />
            <Row
              label={t("settings.resources.memorySwap")}
              value={bytes(cgroup.swap_current_bytes)}
              level={
                cgroup.swap_current_bytes === null
                  ? "unknown"
                  : cgroup.swap_current_bytes > 0
                    ? "warn"
                    : "ok"
              }
              hint={t("settings.resources.memorySwapHint")}
            />
            <Row label={t("settings.resources.pids")} value={count(cgroup.pids_current)} />
            <Row
              label={t("settings.resources.psi")}
              value={percent(cgroup.psi_memory.full10) ?? missing}
              hint={t("settings.resources.psiHint")}
              level={levelFor(cgroup.psi_memory.full10, 5, 10)}
              sparkline={
                psiSeries.length > 1 ? (
                  <Sparkline values={psiSeries} label={t("settings.resources.psi")} width={78} height={20} />
                ) : null
              }
            />
            <Row
              label={t("settings.resources.memoryEvents")}
              value={`high ${count(cgroup.events.high)} · max ${count(cgroup.events.max)} · oom ${count(
                cgroup.events.oom,
              )} · oom_kill ${count(oomKills)}`}
              level={(oomKills ?? 0) > 0 ? "bad" : (cgroup.events.max ?? 0) > 0 ? "warn" : "ok"}
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
          hint={t("settings.resources.persistentWorkersHint")}
          value={t("settings.resources.persistentWorkerValue", {
            count: snapshot.plugins.processes.length,
          })}
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
              label={featureLabel(usage.feature)}
              value={t("settings.resources.modelUsage", {
                requests: usage.requests,
                failures: usage.failures,
                tokens: usage.tokens_in + usage.tokens_out,
              })}
            />
          ))
        )}
        <Row
          label={t("settings.resources.inProcess")}
          value={t("settings.resources.inProcessHint")}
          hint={t("settings.resources.inProcessNote")}
        />
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
            .map(([name, value]) => `${phaseLabel(name)} ${milliseconds(value) ?? missing}`)
            .join(" · ") || missing}
          hint={t("settings.resources.phasesHint")}
        />
        <Row label={t("settings.resources.permitWait")} value={milliseconds(snapshot.queues.permit_wait_p50_ms) ?? missing} />
      </Group>

      <Group title={t("settings.resources.redis")}>
        {!redis ? (
          <Row label={t("settings.resources.redis")} value={reason(snapshot.redis_unavailable)} />
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
              level={redis.persistence_ok === null ? "unknown" : redis.persistence_ok ? "ok" : "bad"}
            />
            <Row label={t("settings.resources.redisEvicted")} value={count(redis.evicted_keys)} />
          </>
        )}
      </Group>

      <Group title={t("settings.resources.network")}>
        <Row
          label={t("settings.resources.networkDown")}
          value={`${bytes(snapshot.network.rx_bytes)} · ${rate(snapshot.network.rx_rate_bps) ?? missing}`}
          hint={t("settings.resources.networkHint")}
        />
        <Row
          label={t("settings.resources.networkUp")}
          value={`${bytes(snapshot.network.tx_bytes)} · ${rate(snapshot.network.tx_rate_bps) ?? missing}`}
          hint={t("settings.resources.networkHint")}
        />
      </Group>

      <Group title={t("settings.resources.io")}>
        <Row
          label={t("settings.resources.ioProcess")}
          value={`${t("settings.resources.ioRead")} ${bytes(snapshot.server.read_bytes)} · ${t(
            "settings.resources.ioWrite",
          )} ${bytes(snapshot.server.write_bytes)}`}
          hint={t("settings.resources.ioProcessHint")}
        />
        {io.map((entry) => (
          <Row
            key={entry.device}
            label={entry.device}
            value={`${t("settings.resources.ioRead")} ${bytes(entry.read_bytes)} · ${t(
              "settings.resources.ioWrite",
            )} ${bytes(entry.write_bytes)}`}
            hint={t("settings.resources.ioHint")}
          />
        ))}
        {io.length > 0 ? (
          <Row
            label={t("settings.resources.ioTotal")}
            value={`${t("settings.resources.ioRead")} ${bytes(ioRead)} · ${t("settings.resources.ioWrite")} ${bytes(
              ioWrite,
            )}`}
          />
        ) : (
          <Note text={t("settings.resources.ioUnavailable")} />
        )}
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
          <>
            <Row label={t("settings.resources.gpu")} value={reason(snapshot.gpu_unavailable)} />
            {gpuOff ? <Note text={t("settings.resources.gpuEnableHint")} /> : null}
          </>
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
          <Row
            label={t("settings.resources.truncated")}
            value={snapshot.truncations.map((code) => truncationLabel(code)).join(" · ")}
          />
        ) : null}
      </Group>
      </div>
    </div>
  )
}

/** The four readings a non-expert opens this page for, each answered as a colour and a word. */
function Overview({ snapshot }: { snapshot: ResourceSnapshot }) {
  const { t } = useTranslation()
  const memory = memoryReading(snapshot)
  const cpu = cpuReading(snapshot)
  const disk = diskReading(snapshot.storage.library)
  const pressure = pressureReading(snapshot)
  const limit = snapshot.cgroup?.memory_max_bytes ?? null
  const used = snapshot.cgroup?.memory_current_bytes ?? null

  return (
    <div className="resources-overview">
      <div className="resources-tiles">
        <Tile
          label={t("settings.resources.tileMemory")}
          level={memory.level}
          share={memory.percent}
          value={
            limit
              ? t("settings.resources.tileMemoryValue", {
                  percent: percent(memory.percent) ?? missing,
                  used: bytes(used),
                  limit: bytes(limit),
                })
              : t("settings.resources.tileMemoryNoLimit", { used: bytes(used) })
          }
          hint={t("settings.resources.tileMemoryHint")}
        />
        <Tile
          label={t("settings.resources.tileCpu")}
          level={cpu.level}
          share={cpu.percent}
          value={t("settings.resources.tileCpuValue", {
            percent: percent(cpu.percent) ?? missing,
            cores: snapshot.runtime.cpu_cores,
          })}
          hint={t("settings.resources.tileCpuHint")}
        />
        <Tile
          label={t("settings.resources.tileDisk")}
          level={disk.level}
          share={disk.percent}
          value={t("settings.resources.tileDiskValue", {
            percent: percent(disk.percent) ?? missing,
            free: bytes(snapshot.storage.library?.free_bytes ?? null),
          })}
          hint={t("settings.resources.tileDiskHint")}
        />
        <Tile
          label={t("settings.resources.tilePressure")}
          level={pressure.level}
          share={pressure.percent}
          value={t("settings.resources.tilePressureValue", {
            percent: percent(pressure.percent) ?? missing,
          })}
          hint={t("settings.resources.tilePressureHint")}
        />
        <Tile
          label={t("settings.resources.tileIo")}
          value={
            <>
              <span className="resources-tile-line">
                {t("settings.resources.ioRead")} {bytes(snapshot.server.read_bytes)}
              </span>
              <span className="resources-tile-line">
                {t("settings.resources.ioWrite")} {bytes(snapshot.server.write_bytes)}
              </span>
            </>
          }
          hint={t("settings.resources.tileIoHint")}
        />
        <Tile
          label={t("settings.resources.tileNetwork")}
          value={
            <>
              <span className="resources-tile-line">
                {t("settings.resources.networkDown")} {rate(snapshot.network.rx_rate_bps) ?? missing}
              </span>
              <span className="resources-tile-line">
                {t("settings.resources.networkUp")} {rate(snapshot.network.tx_rate_bps) ?? missing}
              </span>
            </>
          }
          hint={t("settings.resources.tileNetworkHint")}
        />
      </div>
      <p className="resources-legend">{t("settings.resources.legend")}</p>
    </div>
  )
}

function Tile({
  label,
  level,
  share,
  value,
  hint,
}: {
  label: string
  /** Omitted when nothing in the snapshot can judge the reading (throughput, for instance). */
  level?: Level
  share?: number | null
  value: React.ReactNode
  hint: string
}) {
  const { t } = useTranslation()
  // A reading with no limit behind it must not draw a bar at all; a NaN would otherwise widen the
  // fill to the whole track and read as "100%" when the snapshot simply had no number yet.
  const filled = share === null || share === undefined || !Number.isFinite(share) ? null : Math.max(0, Math.min(100, share))
  return (
    <section className={`resources-tile${level ? ` resources-${level}` : ""}`}>
      <header className="resources-tile-head">
        <span className="resources-tile-label">{label}</span>
        {level ? <span className="resources-tile-level">{t(`settings.resources.level.${level}`)}</span> : null}
      </header>
      <div className="resources-tile-value">{value}</div>
      {filled === null ? null : (
        <span className="resources-meter" aria-hidden="true">
          <span className="resources-meter-fill" style={{ width: `${filled}%` }} />
        </span>
      )}
      <p className="resources-tile-hint">{hint}</p>
    </section>
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
    <section className="resources-card">
      <h2 className="resources-card-title">{title}</h2>
      <table className="resources-table">
        <tbody>{children}</tbody>
      </table>
    </section>
  )
}

function Row({
  label,
  value,
  hint,
  level,
  sparkline,
}: {
  label: string
  value: string | null
  hint?: string
  /** A dot and a tinted value; omit when the row has no limit to be judged against. */
  level?: Level
  sparkline?: React.ReactNode
}) {
  const tone = level && level !== "unknown" ? `resources-${level}` : ""
  const emphasis = level === "warn" || level === "bad" ? tone : undefined
  return (
    <tr className={level === "bad" ? "resources-alert" : undefined}>
      <th scope="row">
        {level ? <span className={`resources-dot ${tone}`} aria-hidden="true" /> : null}
        {label}
        {hint ? (
          <Tooltip label={hint} maxWidth={280} wrapperStyle={{ marginLeft: 4, verticalAlign: "middle" }}>
            <FaCircleQuestion className="resources-hint-icon" aria-label={hint} />
          </Tooltip>
        ) : null}
      </th>
      <td className={emphasis}>{value === null ? missing : value}</td>
      <td className="resources-spark">{sparkline ?? null}</td>
    </tr>
  )
}

/** A line that explains a row instead of being read as another value. */
function Note({ text }: { text: string }) {
  return (
    <tr className="resources-note">
      <td colSpan={3}>{text}</td>
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
      level={
        process.calls > 0
          ? levelFor((process.failures / process.calls) * 100, 10, 20)
          : "ok"
      }
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
      level={diskReading(disk).level}
    />
  )
}
