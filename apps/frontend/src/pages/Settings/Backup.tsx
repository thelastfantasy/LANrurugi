import { useQueryClient } from "@tanstack/react-query"
import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useLocation, useNavigate } from "react-router-dom"

import { ApiError } from "@/api/client"
import {
  invalidateAfterDataRestore,
  useBackupJobProgress,
  useBackups,
  useCreateBackup,
  useCreateExport,
  useCronPreview,
  useDeleteBackup,
  useDeleteImportSnapshot,
  useImportSnapshots,
  usePinBackup,
  useRestoreBackup,
  useSettings,
  useUnpinBackup,
  useUpdateSettings,
} from "@/api/hooks"
import type { BackupEntry, BackupJobStatus, BackupSummary } from "@/api/types"
import { Button, IconButton, Switch } from "@/components/common-ui/Form"
import { Tabs } from "@/components/common-ui/Tabs"
import { confirmDialog } from "@/dialog"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM, useApplyTheme } from "@/theme"
import { toast } from "@/toast"

import { BackupProgressBar } from "./BackupProgressBar"
import { LegacyImportPanel } from "./LegacyImportPanel"

/** The server's own default cron (`0 3 * * 1` — weekly, Monday 03:00) and the preset the page
 *  starts from, used only until one of the two endpoints has answered. */
const DEFAULT_CRON = "0 0 3 * * 1 *"
const DEFAULT_PRESET = "weekly"

/** How many `pre-*` exports retention keeps (`PRE_KEEP` in `crates/lanrurugi-api/src/snapshots.rs`). */
const PRE_KEEP = 5

/** How many plain (unpinned) full snapshots retention keeps — the server's own default. */
const SNAPSHOT_KEEP = 3

/** The seven cron fields, in order: second, minute, hour, day, month, weekday, year. */
const CRON_FIELDS = ["second", "minute", "hour", "day", "month", "weekday", "year"] as const

/** Preset → 7-field cron. `custom` keeps whatever the user has typed. */
const CRON_PRESETS = [
  { value: "every_minute", cron: "0 * * * * * *" },
  { value: "every_5_minutes", cron: "0 */5 * * * * *" },
  { value: "hourly", cron: "0 0 * * * * *" },
  { value: "daily", cron: "0 0 3 * * * *" },
  { value: "weekly", cron: "0 0 3 * * 1 *" },
  { value: "monthly", cron: "0 0 3 1 * * *" },
  { value: "yearly", cron: "0 0 3 1 1 * *" },
  { value: "custom", cron: null },
] as const

/** Splits a cron string into exactly seven fields, padding a shorter (still-being-typed) one with
 *  `*` so the seven controls never shift under the user. */
function splitCron(value: string): string[] {
  const parts = value.trim().split(/\s+/).filter(Boolean)
  return Array.from({ length: CRON_FIELDS.length }, (_, index) => parts[index] ?? "*")
}

function joinCron(parts: string[]): string {
  return parts.join(" ")
}

/** `HH:mm:ss` in the reader's own timezone — the live timer's start/end labels. */
function formatClock(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0")
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

/** `YYYY-MM-DD HH:mm:ss` — card timestamps are exact instants, never a vague "a while ago". */
function formatDateTime(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0")
  return `${date.getFullYear()}-${pad(date.getMonth() + 1)}-${pad(date.getDate())} ${formatClock(date)}`
}

/** One-decimal MiB, the unit the snapshot/export sizes are shown in. */
function formatMiB(bytes: number): string {
  return `${(bytes / (1024 * 1024)).toFixed(1)} MiB`
}

/** One fire instant as the server's own timezone, with seconds (the cron has second granularity),
 *  or `null` when the server sent something unparseable. */
function scheduleInstantLabel(value: string | null | undefined, timezone?: string): string | null {
  if (!value) return null
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return null
  try {
    return date.toLocaleString(undefined, {
      timeZone: timezone || undefined,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      hour: "2-digit",
      minute: "2-digit",
      second: "2-digit",
    })
  } catch {
    return date.toLocaleString()
  }
}

/** Mirrors legacy's backup/restore layout (backup/restore actions, spinner, return button) using
 * common-ui `Button`s instead of legacy's table markup; status is a plain text line, no progress bar. */
export function Backup() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const location = useLocation()
  const fileInputRef = useRef<HTMLInputElement>(null)
  const [actionStatus, setActionStatus] = useState<string>("")
  type JobOp = "snapshot" | "export" | "restore" | "import"
  /** Every long action on this page (four flows) runs one job at a time and reports over the same
   *  SSE hook + progress bar. */
  const [activeJob, setActiveJob] = useState<{
    op: JobOp
    jobId: string
    startedPerf: number
  } | null>(null)
  /** The LANraragi import panel runs its own job; this makes the page's buttons share the limit. */
  const [panelBusy, setPanelBusy] = useState(false)
  const [runSeconds, setRunSeconds] = useState(0)
  /** The last finished job's precise result, kept next to the flow that produced it. */
  const [runStatus, setRunStatus] = useState<{ op: JobOp; text: string } | null>(null)
  const jobBusy = activeJob !== null || panelBusy
  const queryClient = useQueryClient()

  const snapshotsQuery = useImportSnapshots()
  const deleteSnapshot = useDeleteImportSnapshot()

  // specs/028 deletion safety net — same query/mutation style as the import-snapshot list above.
  const backupsQuery = useBackups()
  const createBackup = useCreateBackup()
  const createExport = useCreateExport()
  const restoreBackup = useRestoreBackup()
  const deleteBackup = useDeleteBackup()
  const pinBackup = usePinBackup()
  const unpinBackup = useUnpinBackup()

  // The schedule block saves through the page-wide settings path (`PUT /api/settings`) and then
  // re-reads `/api/backups` for the instants the server computed from the new values.
  const settingsQuery = useSettings()
  const updateSettings = useUpdateSettings()
  const [scheduleEnabled, setScheduleEnabled] = useState(true)
  const [preset, setPreset] = useState<string>(DEFAULT_PRESET)
  const [cron, setCron] = useState<string>(DEFAULT_CRON)
  const [scheduleStatus, setScheduleStatus] = useState("")

  // The active tab is the URL, not component state: deep links, refresh and the browser's
  // back/forward buttons all resolve through the same `/backup/...` paths.
  const backupPath = location.pathname.replace(/\/+$/, "")
  const activeTab =
    backupPath === "/backup/import-export"
      ? "importExport"
      : backupPath === "/backup/legacy-import"
        ? "legacy"
        : "snapshot"

  const schedule = backupsQuery.data?.schedule ?? null
  // `/api/backups` is authoritative (it also carries the computed instants); the settings fields
  // are a fallback so the controls still round-trip if only one of the two endpoints is deployed.
  const serverEnabled = schedule ? schedule.enabled : settingsQuery.data?.snapshot_schedule_enabled
  const serverCron = schedule?.cron ?? settingsQuery.data?.snapshot_cron ?? ""
  const serverPreset = schedule?.preset ?? settingsQuery.data?.snapshot_cron_preset ?? DEFAULT_PRESET
  // Adjust state during render (the same pattern `SettingsPage` uses for `language`): re-seed the
  // controls only when what the server reports actually changed. Keying off a plain string means a
  // background refetch that brings back the same values never discards unsaved edits.
  const serverScheduleKey = `${serverEnabled === undefined ? "" : String(serverEnabled)}|${serverCron}|${serverPreset}`
  const [syncedScheduleKey, setSyncedScheduleKey] = useState<string | null>(null)
  if (serverEnabled !== undefined && serverScheduleKey !== syncedScheduleKey) {
    setSyncedScheduleKey(serverScheduleKey)
    setScheduleEnabled(serverEnabled)
    setCron(serverCron || DEFAULT_CRON)
    setPreset(serverPreset)
  }

  const cronParts = splitCron(cron)
  // Debounce the preview so typing in one of the seven fields doesn't fire a request per keystroke.
  const [previewCron, setPreviewCron] = useState<string | null>(null)
  useEffect(() => {
    const id = setTimeout(() => setPreviewCron(cron.trim()), 300)
    return () => clearTimeout(id)
  }, [cron])
  const preview = useCronPreview(previewCron)
  const previewTimezone = schedule?.timezone
  const invalidCron = preview.data?.valid === false
  const previewRuns = (preview.data?.next_runs ?? []).slice(0, 10)

  const backups = backupsQuery.data?.entries ?? []
  // The one list is split by kind. Anything without an explicit `"export"` counts as a full
  // snapshot, so a mislabelled entry never gets offered a restore that cannot work.
  const snapshots = backups.filter((entry) => entry.kind !== "export")
  const exports = backups.filter((entry) => entry.kind === "export")
  const pinnedSnapshotCount = snapshots.filter((entry) => entry.pinned).length
  const lastSnapshotAt = scheduleInstantLabel(schedule?.last_fire_at, previewTimezone)
  const nextSnapshotAt = scheduleInstantLabel(schedule?.next_runs?.[0], previewTimezone)

  /** A reason a newer server adds isn't in this build's locale files — show it verbatim. */
  const reasonLabel = (reason: string) =>
    t(`settings.snapshotReason.${reason}`, { defaultValue: reason })
  const presetLabel = (value: string) =>
    t(`settings.snapshotCronPreset.${value}`, { defaultValue: value })
  const cronFieldLabel = (field: string) =>
    t(`settings.snapshotCronField.${field}`, { defaultValue: field })
  /** The label/value pairs an export card lists. The archive count is the headline, so it leads. */
  const summaryFacts = (summary: BackupSummary) => [
    { label: t("settings.autoSnapshotsDetailArchives"), value: summary.archives },
    { label: t("settings.autoSnapshotsDetailArchivesWithTags"), value: summary.archives_with_tags },
    { label: t("settings.autoSnapshotsDetailCategories"), value: summary.categories },
    { label: t("settings.autoSnapshotsDetailTankoubons"), value: summary.tankoubons },
    { label: t("settings.autoSnapshotsDetailStamps"), value: summary.stamps },
    { label: t("settings.autoSnapshotsDetailBookmarks"), value: summary.bookmarks },
  ]

  /** One shared entry point for every long action: start the job, then follow its stream. */
  async function startJob(op: JobOp, request: () => Promise<{ job_id: string }>) {
    if (jobBusy) return
    // Called from an event handler, never during render — the timing mark has to be impure.
    // eslint-disable-next-line react-hooks/purity
    const startedPerf = performance.now()
    setRunStatus(null)
    setRunSeconds(0)
    try {
      const created = await request()
      setActiveJob({ op, jobId: created.job_id, startedPerf })
    } catch (e) {
      // The request never became a job, so there is no percentage to report.
      setRunStatus({
        op,
        text: t("settings.backupRunFailed", {
          percent: "0.0",
          seconds: "0.0",
          error: String(e),
        }),
      })
    }
  }

  /** Manual import: the uploaded JSON is restored by the server job. */
  async function handleRestore(file: File) {
    const formData = new FormData()
    formData.append("file", file)
    await startJob("import", async () => {
      const response = await fetch("/api/database/restore", { method: "POST", body: formData })
      if (!response.ok) throw new Error(`Restore request failed with ${response.status}`)
      return (await response.json()) as { job_id: string }
    })
    if (fileInputRef.current) fileInputRef.current.value = ""
  }

  /** Called from the stream's own event handler when a job finishes — freezes the result, stops the
   *  timer and refreshes whatever that job changed. */
  function handleJobTerminal(status: BackupJobStatus) {
    if (!activeJob) return
    const op = activeJob.op
    const seconds = ((performance.now() - activeJob.startedPerf) / 1000).toFixed(1)
    if (status.state === "done") {
      const size = status.entry?.size_bytes ?? status.bytes_written
      setRunStatus({
        op,
        text: t("settings.backupRunDone", {
          seconds,
          size: formatMiB(size),
          percent: (status.percent || 100).toFixed(1),
        }),
      })
      if (op === "restore" || op === "import") {
        invalidateAfterDataRestore(queryClient)
      } else {
        void backupsQuery.refetch()
      }
    } else {
      setRunStatus({
        op,
        text: t("settings.backupRunFailed", {
          percent: (status.percent ?? 0).toFixed(1),
          seconds,
          error: status.error ?? t("edit.unknownError"),
        }),
      })
    }
    setActiveJob(null)
  }

  // The stream is only open while its panel is the active one: switching tabs closes it (the hook's
  // effect cleanup), and switching back re-subscribes with a catch-up read for anything missed.
  // Restore/import live in the second tab.
  const activeJobId =
    activeJob &&
    ((activeJob.op === "snapshot" && activeTab === "snapshot") ||
      (activeJob.op !== "snapshot" && activeTab === "importExport"))
      ? activeJob.jobId
      : null
  // Live job progress over the shared SSE hook (the EventSource itself lives in `api/hooks.ts`).
  const { status: jobStatus, live: jobLive } = useBackupJobProgress(activeJobId, handleJobTerminal)
  const jobPercent = jobStatus?.percent ?? 0
  const jobWritten = jobStatus?.bytes_written ?? 0
  const jobExpected = jobStatus?.bytes_expected ?? 0

  /** 100ms is fast enough for a tenth-of-a-second readout without re-rendering the page every frame. */
  useEffect(() => {
    if (!activeJob) return
    const update = () => setRunSeconds((performance.now() - activeJob.startedPerf) / 1000)
    update()
    const id = setInterval(update, 100)
    return () => clearInterval(id)
  }, [activeJob])

  async function runCreate(kind: "snapshot" | "export") {
    await startJob(kind, () =>
      kind === "snapshot" ? createBackup.mutateAsync() : createExport.mutateAsync(),
    )
  }

  /** The shared bar/status for one flow. Rendering it right after that flow's buttons is what keeps
   *  "which job is this" obvious even when two panels have statuses of their own. */
  function jobNode(op: JobOp) {
    if (activeJob?.op === op) {
      const progressKey =
        op === "restore"
          ? "settings.backupRunProgressRestore"
          : op === "import"
            ? "settings.backupRunProgressImport"
            : "settings.backupRunProgress"
      return (
        <BackupProgressBar
          percent={jobPercent}
          label={t(progressKey, {
            percent: jobPercent.toFixed(1),
            seconds: runSeconds.toFixed(1),
          })}
          detail={
            jobExpected > 0 ? `${formatMiB(jobWritten)} / ${formatMiB(jobExpected)}` : undefined
          }
          note={!jobLive ? t("settings.backupRunRealtimeUnavailable") : undefined}
        />
      )
    }
    if (!activeJob && runStatus?.op === op) {
      return <p className="backup-run-status">{runStatus.text}</p>
    }
    return null
  }

  async function handleSaveSchedule() {
    setScheduleStatus("")
    try {
      await updateSettings.mutateAsync({
        snapshot_schedule_enabled: scheduleEnabled,
        snapshot_cron: cron.trim(),
        snapshot_cron_preset: preset,
      })
      // The wording under the controls is computed by the server from these values.
      await backupsQuery.refetch()
      toast({ heading: t("settings.snapshotScheduleSaved") ?? undefined, icon: "success" })
    } catch (e) {
      setScheduleStatus(t("settings.snapshotScheduleSaveFailed", { error: String(e) }))
    }
  }

  async function handleRestoreSnapshot(entry: BackupEntry) {
    const proceed = await confirmDialog(
      t("settings.autoSnapshotsRestoreConfirm", {
        name: entry.name,
        createdAt: new Date(entry.created_at).toLocaleString(),
      }),
      true,
    )
    if (!proceed) return

    // A full `.rdb` snapshot is never restored from here (the server rejects it); only exports are.
    await startJob("restore", () => restoreBackup.mutateAsync(entry.name))
  }

  async function handleDeleteSnapshot(entry: BackupEntry) {
    const proceed = await confirmDialog(
      t("settings.autoSnapshotsDeleteConfirm", { name: entry.name }),
      true,
    )
    if (!proceed) return

    setActionStatus(t("settings.autoSnapshotsDeleting", { name: entry.name }))
    try {
      await deleteBackup.mutateAsync(entry.name)
      setActionStatus(t("settings.autoSnapshotsDeleted", { name: entry.name }))
    } catch (e) {
      // The server's own guard (409 `last_usable`, the last remaining restorable snapshot) is shown
      // verbatim: it names the rule that blocked the delete, and the page must not talk over it.
      setActionStatus(
        e instanceof ApiError && e.status === 409
          ? e.message
          : t("settings.autoSnapshotsDeleteFailed", { error: String(e) }),
      )
    }
  }

  /** Idempotent on the server, and the list refetches through the mutation's own `onSuccess`. */
  async function handlePinSnapshot(entry: BackupEntry) {
    setActionStatus("")
    try {
      if (entry.pinned) await unpinBackup.mutateAsync(entry.name)
      else await pinBackup.mutateAsync(entry.name)
    } catch (e) {
      setActionStatus(t("settings.autoSnapshotsPinFailed", { error: String(e) }))
    }
  }

  useApplyTheme()
  useDocumentTitle(t("settings.databaseBackupRestore") ?? undefined)

  return (
    <div className="ido">
      <h2 className="ih" style={{ textAlign: "center" }}>
        {t("settings.databaseBackupRestore")}
      </h2>

      <div className="backup-settings" style={{ fontSize: FONT_SIZE_SM }}>
        <Tabs
          className="backup-tabs"
          activeKey={activeTab}
          ariaLabel={t("settings.databaseBackupRestore") ?? undefined}
          onChange={(key) =>
            navigate(
              routes.backupTab(
                key === "snapshot" ? "snapshot" : key === "importExport" ? "import-export" : "legacy-import",
              ),
            )
          }
          items={[
            { key: "snapshot", label: t("settings.autoSnapshotsTitle") },
            { key: "importExport", label: t("settings.exportTitle") },
            { key: "legacy", label: t("settings.importFromLegacyTitle") },
          ]}
        />

        {/* Both keep their state: the panels stay mounted and are only hidden, so switching tabs
            never discards a half-typed cron expression. */}
        <div role="tabpanel" style={{ display: activeTab === "snapshot" ? "block" : "none" }}>
        <h3 className="backup-block-title">{t("settings.autoSnapshotsTitle")}</h3>
        <p className="backup-block-note">{t("settings.autoSnapshotsDescription")}</p>

        <h4 className="backup-block-subtitle">{t("settings.snapshotScheduleTitle")}</h4>
        <p className="backup-block-note">{t("settings.snapshotScheduleDescription")}</p>

        <div className="backup-plan">
          <div className="backup-plan-form">
            <div className="backup-inline-row">
              <label className="backup-inline-field" htmlFor="backup-schedule-enabled">
                <span>{t("settings.snapshotScheduleEnabled")}</span>
                <Switch
                  id="backup-schedule-enabled"
                  checked={scheduleEnabled}
                  onCheckedChange={setScheduleEnabled}
                />
              </label>
              <label className="backup-inline-field" htmlFor="backup-schedule-preset">
                <span>{t("settings.snapshotSchedulePreset")}</span>
                <select
                  id="backup-schedule-preset"
                  className="stdinput backup-control backup-control--preset"
                  value={preset}
                  onChange={(e) => {
                    const next = e.target.value
                    setPreset(next)
                    const chosen = CRON_PRESETS.find((item) => item.value === next)
                    if (chosen?.cron) setCron(chosen.cron)
                  }}
                >
                  {CRON_PRESETS.map((item) => (
                    <option key={item.value} value={item.value}>
                      {presetLabel(item.value)}
                    </option>
                  ))}
                </select>
              </label>
            </div>

            {preset === "custom" && (
              <>
                <div className="backup-cron-fields">
                  {CRON_FIELDS.map((field, index) => (
                    <label className="backup-cron-field" key={field}>
                      <span>{cronFieldLabel(field)}</span>
                      <input
                        className="stdinput backup-control backup-control--cron"
                        value={cronParts[index]}
                        aria-label={cronFieldLabel(field)}
                        onChange={(e) => {
                          const next = [...cronParts]
                          next[index] = e.target.value.trim() || "*"
                          setCron(joinCron(next))
                        }}
                      />
                    </label>
                  ))}
                </div>
                <p className="backup-hint">{t("settings.snapshotCronRanges")}</p>
              </>
            )}
          </div>

          <div className="backup-plan-preview">
            <h5 className="backup-preview-title">{t("settings.snapshotCronPreviewTitle")}</h5>
            <p className="backup-preview-meta">
              {t("settings.snapshotCronTimezone", { timezone: previewTimezone ?? "—" })}
            </p>
            <code className="backup-cron-value">{cron.trim()}</code>
            {invalidCron && (
              <p className="backup-hint backup-hint--error">
                {t("settings.snapshotCronInvalid", { error: preview.data?.error ?? "" })}
              </p>
            )}
            {preview.isError && (
              <p className="backup-hint backup-hint--error">
                {t("settings.snapshotCronPreviewFailed", { error: String(preview.error) })}
              </p>
            )}
            {preview.isLoading && (
              <p className="backup-preview-meta">{t("settings.snapshotCronPreviewLoading")}</p>
            )}
            {!invalidCron && !preview.isError && previewRuns.length > 0 && (
              <>
                <p className="backup-preview-meta">
                  {t("settings.snapshotCronNextRuns", { count: previewRuns.length })}
                </p>
                <ol className="backup-preview-runs">
                  {previewRuns.map((run) => (
                    <li key={run}>{scheduleInstantLabel(run, previewTimezone)}</li>
                  ))}
                </ol>
              </>
            )}
            {!invalidCron &&
              !preview.isError &&
              !preview.isLoading &&
              preview.data?.valid &&
              previewRuns.length === 0 && (
                <p className="backup-preview-meta">{t("settings.snapshotCronNoRuns")}</p>
              )}
          </div>
        </div>

        {schedule && !schedule.enabled && (
          <div className="backup-summary">
            <span>{t("settings.snapshotScheduleOff")}</span>
          </div>
        )}

        {scheduleStatus && <p className="backup-hint">{scheduleStatus}</p>}

        <div className="backup-form-actions" style={{ marginTop: 16 }}>
          <button
            type="button"
            className="stdbtn backup-btn"
            disabled={updateSettings.isPending || invalidCron}
            onClick={() => void handleSaveSchedule()}
          >
            {t("settings.snapshotScheduleSave")}
          </button>
          <button
            type="button"
            className="stdbtn backup-btn"
            disabled={jobBusy}
            onClick={() => void runCreate("snapshot")}
          >
            {activeJob?.op === "snapshot" && (
              <>
                <i className="fa fa-compact-disc fa-spin" aria-hidden="true" />{" "}
              </>
            )}
            {t("settings.autoSnapshotsCreateNow")}
          </button>
        </div>

        {jobNode("snapshot")}

        {backupsQuery.isLoading && (
          <p className="backup-state">{t("settings.autoSnapshotsLoading")}</p>
        )}
        {backupsQuery.isError && (
          <div className="backup-error" role="alert">
            <p className="backup-error-title">{t("settings.autoSnapshotsErrorFriendly")}</p>
            <details>
              <summary>{t("settings.technicalDetails")}</summary>
              <pre className="backup-error-detail">{String(backupsQuery.error)}</pre>
            </details>
          </div>
        )}

        {backupsQuery.data && (
          <div className="backup-summary" style={{ marginTop: 12 }}>
            <span>
              {t("settings.snapshotSummaryCount", { total: snapshots.length })}
              {" · "}
              {t("settings.snapshotKeepRecent", { keep: SNAPSHOT_KEEP })}
            </span>
            {schedule?.enabled && (
              <span>
                {t("settings.snapshotSummaryLast", {
                  value: lastSnapshotAt ?? t("settings.snapshotScheduleNever"),
                })}
                {" · "}
                {t("settings.snapshotSummaryNext", {
                  value: nextSnapshotAt ?? t("settings.snapshotScheduleNever"),
                })}
              </span>
            )}
            {pinnedSnapshotCount > 0 && <span>{t("settings.snapshotPinnedNote")}</span>}
          </div>
        )}

        {backupsQuery.data && snapshots.length === 0 && (
          <p className="backup-state">{t("settings.snapshotEmpty")}</p>
        )}

        {snapshots.length > 0 && (
          <div className="backup-card-grid">
            {snapshots.map((entry) => (
              <article className="backup-card" key={entry.name}>
                <div className="backup-card-head">
                  <span className="backup-card-name" title={entry.name}>
                    {entry.name}
                  </span>
                  <span className="backup-card-badge">{t("settings.backupKindSnapshot")}</span>
                  {entry.pinned && (
                    <span className="backup-card-badge backup-card-badge--pinned">
                      {t("settings.autoSnapshotsPinned")}
                    </span>
                  )}
                </div>
                <div className="backup-card-meta">
                  <span>{formatDateTime(new Date(entry.created_at))}</span>
                  <span>{formatMiB(entry.size_bytes)}</span>
                  <span>{reasonLabel(entry.reason)}</span>
                </div>
                <div className="backup-card-actions">
                  <a
                    className="stdbtn backup-btn"
                    href={`/api/backups/${encodeURIComponent(entry.name)}`}
                    download={entry.name}
                  >
                    <i className="fa fa-download" aria-hidden="true" />{" "}
                    {t("settings.snapshotDownload")}
                  </a>
                  <button
                    type="button"
                    className="stdbtn backup-btn"
                    disabled={pinBackup.isPending || unpinBackup.isPending}
                    onClick={() => void handlePinSnapshot(entry)}
                  >
                    {entry.pinned ? t("settings.autoSnapshotsUnpin") : t("settings.autoSnapshotsPin")}
                  </button>
                  <button
                    type="button"
                    className="stdbtn stdbtn-danger backup-btn"
                    disabled={deleteBackup.isPending}
                    onClick={() => void handleDeleteSnapshot(entry)}
                  >
                    {t("settings.autoSnapshotsDelete")}
                  </button>
                </div>
              </article>
            ))}
          </div>
        )}

        {/* Part of the snapshot block, not a block of its own: small label, no divider. */}
        <p
          className="backup-block-note"
          style={{ marginTop: 20, marginBottom: 4, fontWeight: "bold" }}
        >
          {t("settings.importSnapshotsTitle")}
        </p>
        <p className="backup-block-note">{t("settings.importSnapshotsDescription")}</p>
      {snapshotsQuery.data && snapshotsQuery.data.length === 0 && (
        <span style={{ fontSize: FONT_SIZE_SM, opacity: 0.7 }}>
          {t("settings.importSnapshotsEmpty")}
        </span>
      )}
      {snapshotsQuery.data && snapshotsQuery.data.length > 0 && (
        <div className="backup-records-scroll">
          {/* Below 640px `index.css` turns every record into a stacked block and hides the header
              row; above it the row wrappers are `display: contents` and the grid's own columns
              align. The wrapper scrolls rather than letting a long row widen the page. */}
          <div
            className="backup-records backup-records--import-snapshots"
            style={{ fontSize: FONT_SIZE_SM }}
          >
            <div className="backup-records-head">
              <span style={{ fontWeight: "bold" }}>{t("settings.importSnapshotsCreatedAt")}</span>
              <span style={{ fontWeight: "bold" }}>{t("settings.importSnapshotsContents")}</span>
              <span />
            </div>
            {snapshotsQuery.data.map((snapshot) => (
              <div className="backup-records-row" key={snapshot.id}>
                <span>{new Date(snapshot.created_at * 1000).toLocaleString()}</span>
                <span className="backup-cell-summary">
                  {t("settings.importSnapshotsContentsSummary", {
                    archives: snapshot.archive_count,
                    categories: snapshot.category_count,
                    tankoubons: snapshot.tankoubon_count,
                    stamps: snapshot.stamp_count,
                  })}
                </span>
                <span className="backup-cell-actions">
                  <IconButton
                    icon="fas fa-download"
                    title={t("settings.importSnapshotsDownload") ?? undefined}
                    onClick={() => {
                      const url = `/api/database/import-snapshots/${encodeURIComponent(snapshot.id)}`
                      const a = document.createElement("a")
                      a.href = url
                      a.download = `lanrurugi-import-snapshot-${snapshot.id}.json`
                      a.click()
                    }}
                  />
                  <IconButton
                    icon="fas fa-trash"
                    title={t("settings.importSnapshotsDelete") ?? undefined}
                    onClick={() => {
                      void (async () => {
                        const proceed = await confirmDialog(
                          t("settings.importSnapshotsDeleteConfirm"),
                          true,
                        )
                        if (proceed) deleteSnapshot.mutate(snapshot.id)
                      })()
                    }}
                  />
                </span>
              </div>
            ))}
          </div>
        </div>
      )}
        </div>

        <div role="tabpanel" style={{ display: activeTab === "importExport" ? "block" : "none" }}>
        <h3 className="backup-block-title">{t("settings.exportTitle")}</h3>
        <p className="backup-block-note">{t("settings.exportDescription")}</p>

        <h4 className="backup-block-subtitle">{t("settings.exportSubtitle")}</h4>
        <div className="backup-form-actions">
          <button
            type="button"
            className="stdbtn backup-btn"
            disabled={jobBusy}
            onClick={() => void runCreate("export")}
          >
            {activeJob?.op === "export" && (
              <>
                <i className="fa fa-compact-disc fa-spin" aria-hidden="true" />{" "}
              </>
            )}
            {t("settings.exportCreateNow")}
          </button>
        </div>

        {jobNode("export")}

        {backupsQuery.data && (
          <div className="backup-summary" style={{ marginTop: 12 }}>
            <span>
              {t("settings.exportSummaryCount", { total: exports.length })}
              {" · "}
              {t("settings.exportRetention", {
                daily: backupsQuery.data.daily_keep,
                weekly: backupsQuery.data.week_keep,
                pre: PRE_KEEP,
              })}
            </span>
          </div>
        )}

        {backupsQuery.data && exports.length === 0 && (
          <p className="backup-state">{t("settings.autoSnapshotsEmpty")}</p>
        )}

        {exports.length > 0 && (
          <div className="backup-card-grid">
            {exports.map((entry) => (
              <article className="backup-card" key={entry.name}>
                <div className="backup-card-head">
                  <span className="backup-card-name" title={entry.name}>
                    {entry.name}
                  </span>
                  <span className="backup-card-badge">{t("settings.backupKindExport")}</span>
                  {entry.pinned && (
                    <span className="backup-card-badge backup-card-badge--pinned">
                      {t("settings.autoSnapshotsPinned")}
                    </span>
                  )}
                </div>
                <div className="backup-card-meta">
                  <span>{formatDateTime(new Date(entry.created_at))}</span>
                  <span>{formatMiB(entry.size_bytes)}</span>
                  <span>{reasonLabel(entry.reason)}</span>
                </div>
                {entry.summary ? (
                  <dl className="backup-card-facts">
                    {summaryFacts(entry.summary).map((fact) => (
                      <div className="backup-card-fact" key={fact.label}>
                        <dt>{fact.label}</dt>
                        <dd>{fact.value}</dd>
                      </div>
                    ))}
                  </dl>
                ) : (
                  <span className="backup-card-summary-missing">—</span>
                )}
                <div className="backup-card-actions">
                  <a
                    className="stdbtn backup-btn"
                    href={`/api/backups/${encodeURIComponent(entry.name)}`}
                    download={entry.name}
                  >
                    <i className="fa fa-download" aria-hidden="true" />{" "}
                    {t("settings.exportDownloadJson")}
                  </a>
                  <a
                    className="stdbtn backup-btn"
                    href={`/api/backups/${encodeURIComponent(entry.name)}?format=zip`}
                  >
                    <i className="fa fa-file-zipper" aria-hidden="true" />{" "}
                    {t("settings.exportDownloadZip")}
                  </a>
                  <button
                    type="button"
                    className="stdbtn backup-btn"
                    disabled={!entry.usable || jobBusy}
                    title={entry.usable ? undefined : t("settings.autoSnapshotsUnusable")}
                    onClick={() => void handleRestoreSnapshot(entry)}
                  >
                    {t("settings.autoSnapshotsRestore")}
                  </button>
                  <button
                    type="button"
                    className="stdbtn backup-btn"
                    disabled={pinBackup.isPending || unpinBackup.isPending}
                    onClick={() => void handlePinSnapshot(entry)}
                  >
                    {entry.pinned ? t("settings.autoSnapshotsUnpin") : t("settings.autoSnapshotsPin")}
                  </button>
                  <button
                    type="button"
                    className="stdbtn stdbtn-danger backup-btn"
                    disabled={deleteBackup.isPending}
                    onClick={() => void handleDeleteSnapshot(entry)}
                  >
                    {t("settings.autoSnapshotsDelete")}
                  </button>
                </div>
              </article>
            ))}
          </div>
        )}

        {jobNode("restore")}

        <h4 className="backup-block-subtitle">{t("settings.importTitle")}</h4>
        <p className="backup-block-note">{t("settings.importNote")}</p>
        <div id="files" className="backup-form-actions">
          {/* `fileinput-button` overlays a transparent file input on the button — some browsers
              refuse untrusted programmatic file-picker clicks via `ref.current.click()`. */}
          <span className="fileinput-button">
            <button type="button" className="stdbtn backup-btn" disabled={jobBusy}>
              {activeJob?.op === "import" && (
                <>
                  <i className="fa fa-compact-disc fa-spin" aria-hidden="true" />{" "}
                </>
              )}
              <i className="fa fa-upload" aria-hidden="true" />{" "}
              {t("settings.restoreBackup")}
            </button>
            <input
              ref={fileInputRef}
              type="file"
              name="file"
              multiple
              disabled={jobBusy}
              onChange={(e) => {
                const file = e.target.files?.[0]
                if (file) void handleRestore(file)
              }}
            />
          </span>
        </div>

        {jobNode("import")}
        </div>

        <div role="tabpanel" style={{ display: activeTab === "legacy" ? "block" : "none" }}>
        <div style={{ marginTop: 20 }}>
          <LegacyImportPanel
            defaultConflictMode="merge"
            disabled={jobBusy}
            onBusyChange={setPanelBusy}
            onImported={() => void snapshotsQuery.refetch()}
          />
        </div>
        </div>
      </div>

      {actionStatus && (
        <p className="backup-hint" style={{ marginTop: 16 }}>
          {actionStatus}
        </p>
      )}

      <div style={{ marginTop: 30 }}>
        <Button onClick={() => navigate(routes.library())}>{t("common.returnToLibrary")}</Button>
      </div>
    </div>
  )
}
