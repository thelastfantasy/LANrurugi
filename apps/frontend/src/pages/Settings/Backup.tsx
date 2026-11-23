import { Fragment, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { waitForJob } from "@/api/client"
import {
  useBackups,
  useCreateBackup,
  useDeleteImportSnapshot,
  useDeletions,
  useImportSnapshots,
  useRestoreAllDeletions,
  useRestoreBackup,
  useRestoreDeletion,
  useSettings,
  useUpdateSettings,
} from "@/api/hooks"
import type { BackupEntry, BackupSummary, DeletionEntry } from "@/api/types"
import { Button, IconButton, Input, Select, Switch } from "@/components/common-ui/Form"
import { confirmDialog } from "@/dialog"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { formatBytes } from "@/lib/formatBytes"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM, useApplyTheme } from "@/theme"
import { toast } from "@/toast"

import { LegacyImportPanel } from "./LegacyImportPanel"

/** The server's own defaults (`DEFAULT_SNAPSHOT_DAILY_AT`/`_WEEKLY_DAY`/`_SCHEDULE_ENABLED` in
 *  `settings.rs`), used only until one of the two endpoints has answered with the real values. */
const DEFAULT_DAILY_AT = "03:00"
const DEFAULT_WEEKLY_DAY = 1

/** Sunday-first, matching the backend's `weekly_day` (`0 = Sunday`, like `Date.getDay()`). */
const WEEKDAYS = [0, 1, 2, 3, 4, 5, 6] as const

/** A native `<input type="time">` only accepts `"HH:MM"`. A stored value carrying seconds, or an
 *  unpadded hour from a hand-edited config, would render as an empty control — normalize instead
 *  of trusting it. Returns `null` when the value can't be a time at all. */
function normalizeDailyAt(value: string | null | undefined): string | null {
  const match = /^(\d{1,2}):(\d{2})/.exec((value ?? "").trim())
  if (!match) return null
  const hours = Number(match[1])
  const minutes = Number(match[2])
  if (hours > 23 || minutes > 59) return null
  return `${String(hours).padStart(2, "0")}:${match[2]}`
}

function normalizeWeeklyDay(value: number | null | undefined): number | null {
  return typeof value === "number" && Number.isInteger(value) && value >= 0 && value <= 6
    ? value
    : null
}

/** A `last_*`/`next_*` instant in the reader's own timezone, or `null` when there is none (or the
 *  server sent something unparseable) so the caller can print its own wording. */
function scheduleInstantLabel(value: string | null | undefined): string | null {
  if (!value) return null
  const date = new Date(value)
  return Number.isNaN(date.getTime()) ? null : date.toLocaleString()
}

/** Mirrors legacy's backup/restore layout (backup/restore actions, spinner, return button) using
 * common-ui `Button`s instead of legacy's table markup; status is a plain text line, no progress bar. */
export function Backup() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const fileInputRef = useRef<HTMLInputElement>(null)
  const [status, setStatus] = useState<string>("")
  const [busy, setBusy] = useState(false)
  const [snapshotStatus, setSnapshotStatus] = useState<string>("")
  const [deletionStatus, setDeletionStatus] = useState<string>("")

  const snapshotsQuery = useImportSnapshots()
  const deleteSnapshot = useDeleteImportSnapshot()

  // specs/028 deletion safety net — same query/mutation style as the import-snapshot list above.
  const backupsQuery = useBackups()
  const createBackup = useCreateBackup()
  const restoreBackup = useRestoreBackup()
  const deletionsQuery = useDeletions()
  const restoreDeletion = useRestoreDeletion()
  const restoreAllDeletions = useRestoreAllDeletions()

  // The schedule block saves through the page-wide settings path (`PUT /api/settings`) and then
  // re-reads `/api/backups` for the instants the server computed from the new values.
  const settingsQuery = useSettings()
  const updateSettings = useUpdateSettings()
  const [scheduleEnabled, setScheduleEnabled] = useState(true)
  const [dailyAt, setDailyAt] = useState(DEFAULT_DAILY_AT)
  const [weeklyDay, setWeeklyDay] = useState(DEFAULT_WEEKLY_DAY)
  const [scheduleStatus, setScheduleStatus] = useState("")
  const [expandedSnapshot, setExpandedSnapshot] = useState<string | null>(null)

  const schedule = backupsQuery.data?.schedule ?? null
  // `/api/backups` is authoritative (it also carries the computed instants); the settings fields
  // are a fallback so the controls still round-trip if only one of the two endpoints is deployed.
  const serverEnabled = schedule ? schedule.enabled : settingsQuery.data?.snapshot_schedule_enabled
  const serverDailyAt =
    normalizeDailyAt(schedule?.daily_at) ?? normalizeDailyAt(settingsQuery.data?.snapshot_daily_at)
  const serverWeeklyDay =
    normalizeWeeklyDay(schedule?.weekly_day) ??
    normalizeWeeklyDay(settingsQuery.data?.snapshot_weekly_day)
  // Adjust state during render (the same pattern `SettingsPage` uses for `language`): re-seed the
  // controls only when what the server reports actually changed. Keying off a plain string means a
  // background refetch that brings back the same values never discards unsaved edits.
  const serverScheduleKey = `${serverEnabled === undefined ? "" : String(serverEnabled)}|${serverDailyAt ?? ""}|${serverWeeklyDay ?? ""}`
  const [syncedScheduleKey, setSyncedScheduleKey] = useState<string | null>(null)
  if (serverEnabled !== undefined && serverScheduleKey !== syncedScheduleKey) {
    setSyncedScheduleKey(serverScheduleKey)
    setScheduleEnabled(serverEnabled)
    setDailyAt(serverDailyAt ?? DEFAULT_DAILY_AT)
    setWeeklyDay(serverWeeklyDay ?? DEFAULT_WEEKLY_DAY)
  }

  /** A reason/source a newer server adds isn't in this build's locale files — show it verbatim. */
  const reasonLabel = (reason: string) =>
    t(`settings.snapshotReason.${reason}`, { defaultValue: reason })
  const sourceLabel = (source: string) =>
    t(`settings.deletionSource.${source}`, { defaultValue: source })
  const weekdayLabel = (day: number) =>
    t(`settings.snapshotScheduleWeekday.${day}`, { defaultValue: String(day) })
  const summaryLabel = (summary: BackupSummary | null | undefined) =>
    summary
      ? t("settings.autoSnapshotsSummaryValue", {
          archives: summary.archives,
          tagged: summary.archives_with_tags,
          categories: summary.categories,
          tankoubons: summary.tankoubons,
          bookmarks: summary.bookmarks,
        })
      : "—"
  const summaryFields = (summary: BackupSummary) => [
    { label: t("settings.autoSnapshotsDetailArchives"), value: summary.archives },
    { label: t("settings.autoSnapshotsDetailArchivesWithTags"), value: summary.archives_with_tags },
    { label: t("settings.autoSnapshotsDetailCategories"), value: summary.categories },
    { label: t("settings.autoSnapshotsDetailTankoubons"), value: summary.tankoubons },
    { label: t("settings.autoSnapshotsDetailStamps"), value: summary.stamps },
    { label: t("settings.autoSnapshotsDetailBookmarks"), value: summary.bookmarks },
    { label: t("settings.autoSnapshotsDetailBytes"), value: formatBytes(summary.bytes) },
  ]

  async function handleBackup() {
    setBusy(true)
    setStatus(t("settings.backupGenerationInProgress"))
    try {
      const response = await fetch("/api/database/backup", { method: "POST" })
      const { job } = (await response.json()) as { job: string }
      const jobStatus = await waitForJob(job)

      if (jobStatus.state === "failed") {
        setStatus(t("settings.backupFailedError", { error: jobStatus.error ?? t("edit.unknownError") }))
        return
      }

      const blob = new Blob([JSON.stringify(jobStatus.notes, null, 2)], {
        type: "application/json",
      })
      const url = URL.createObjectURL(blob)
      const a = document.createElement("a")
      a.href = url
      a.download = `lanrurugi-backup-${new Date().toISOString()}.json`
      a.click()
      URL.revokeObjectURL(url)
      setStatus(t("settings.backupCompleteDownloadWillStart"))
    } catch (e) {
      setStatus(t("settings.backupFailedError", { error: String(e) }))
    } finally {
      setBusy(false)
    }
  }

  async function handleRestore(file: File) {
    setBusy(true)
    setStatus(t("settings.uploadingFile"))
    try {
      const formData = new FormData()
      formData.append("file", file)
      const response = await fetch("/api/database/restore", {
        method: "POST",
        body: formData,
      })
      const { job } = (await response.json()) as { job: string }
      const jobStatus = await waitForJob(job)

      if (jobStatus.state === "failed") {
        setStatus(
          t("settings.restoreFailedError", { error: jobStatus.error ?? t("edit.unknownError") }),
        )
      } else {
        setStatus(t("settings.backupRestored"))
      }
    } catch (e) {
      setStatus(t("settings.restoreFailedError", { error: String(e) }))
    } finally {
      setBusy(false)
      if (fileInputRef.current) fileInputRef.current.value = ""
    }
  }

  async function handleCreateSnapshot() {
    setSnapshotStatus(t("settings.autoSnapshotsCreating"))
    try {
      const response = await createBackup.mutateAsync()
      setSnapshotStatus(t("settings.autoSnapshotsCreated", { name: response.entry.name }))
    } catch (e) {
      setSnapshotStatus(t("settings.autoSnapshotsCreateFailed", { error: String(e) }))
    }
  }

  async function handleSaveSchedule() {
    setScheduleStatus("")
    try {
      await updateSettings.mutateAsync({
        snapshot_schedule_enabled: scheduleEnabled,
        // The backend validates this as a scheduling instant and rejects anything but exactly
        // `HH:MM`, so never send the raw control value if it was cleared.
        snapshot_daily_at: normalizeDailyAt(dailyAt) ?? DEFAULT_DAILY_AT,
        snapshot_weekly_day: weeklyDay,
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

    setSnapshotStatus(t("settings.autoSnapshotsRestoring", { name: entry.name }))
    try {
      const response = await restoreBackup.mutateAsync(entry.name)
      setSnapshotStatus(
        t("settings.autoSnapshotsRestored", { name: entry.name, safety: response.safety }),
      )
    } catch (e) {
      setSnapshotStatus(t("settings.autoSnapshotsRestoreFailed", { error: String(e) }))
    }
  }

  /** Restoring only re-creates records, so no confirmation — only "restore all" (many at once) asks. */
  async function handleRestoreDeletion(entry: DeletionEntry) {
    setDeletionStatus(t("settings.recentDeletionsRestoring", { title: entry.title }))
    try {
      const response = await restoreDeletion.mutateAsync(entry.id)
      setDeletionStatus(
        response.file_missing
          ? t("settings.recentDeletionsRestoredFileMissing", { title: entry.title })
          : t("settings.recentDeletionsRestored", { title: entry.title }),
      )
    } catch (e) {
      setDeletionStatus(
        t("settings.recentDeletionsRestoreFailed", { title: entry.title, error: String(e) }),
      )
    }
  }

  async function handleRestoreAllDeletions() {
    const proceed = await confirmDialog(t("settings.recentDeletionsRestoreAllConfirm"), true)
    if (!proceed) return

    setDeletionStatus(t("settings.recentDeletionsRestoringAll"))
    try {
      const response = await restoreAllDeletions.mutateAsync()
      setDeletionStatus(
        t("settings.recentDeletionsRestoredAll", {
          restored: response.restored,
          skipped: response.skipped,
          missing: response.missing_files,
          failed: response.failed,
        }),
      )
    } catch (e) {
      setDeletionStatus(t("settings.recentDeletionsRestoreAllFailed", { error: String(e) }))
    }
  }

  useApplyTheme()
  useDocumentTitle(t("settings.databaseBackupRestore") ?? undefined)

  return (
    <div className="ido" style={{ textAlign: "center" }}>
      <h2 className="ih" style={{ textAlign: "center" }}>
        {t("settings.databaseBackupRestore")}
      </h2>

      <div style={{ marginTop: 16 }}>
        {t("settings.youCanBackupYourExisting")}
        <div style={{ marginTop: 8 }}>{t("settings.backupingAllowsYouToDownload")}</div>
        <div
          style={{ marginTop: 8 }}
          dangerouslySetInnerHTML={{
            __html: t("settings.restoringFromABackupWill"),
          }}
        />
        <div style={{ marginTop: 8 }}>{t("settings.categoriesWillAlwaysBeRestored")}</div>
      </div>

      <div
        id="files"
        style={{
          display: "flex",
          flexWrap: "wrap",
          justifyContent: "center",
          fontSize: FONT_SIZE_SM,
          marginTop: 25,
          textAlign: "center",
        }}
      >
        <Button
          style={{ minHeight: 70, padding: "8px 16px", display: "inline-block" }}
          disabled={busy}
          onClick={() => void handleBackup()}
        >
          <i className="fa fa-download fa-2x" style={{ paddingTop: 6, paddingBottom: 5 }}></i>
          <div>{t("settings.backupDatabase")}</div>
        </Button>
        {/* `fileinput-button` overlays a transparent file input on the button — some browsers
            refuse untrusted programmatic file-picker clicks via `ref.current.click()`. */}
        <span className="fileinput-button" style={{ display: "inline-block" }}>
          <Button
            style={{ minHeight: 70, padding: "8px 16px", display: "inline-block" }}
            disabled={busy}
          >
            <i className="fa fa-upload fa-2x" style={{ paddingTop: 6, paddingBottom: 5 }}></i>
            <div>{t("settings.restoreBackup")}</div>
          </Button>
          <input
            ref={fileInputRef}
            type="file"
            name="file"
            multiple
            disabled={busy}
            onChange={(e) => {
              const file = e.target.files?.[0]
              if (file) void handleRestore(file)
            }}
          />
        </span>
      </div>

      <span
        style={{
          display: "block",
          margin: "20px auto 0",
          fontSize: FONT_SIZE_SM,
          width: "80%",
          textAlign: "center",
        }}
      >
        {busy && (
          <div id="processing">
            <i className="fa fa-3x fa-compact-disc fa-spin" style={{ marginTop: 20 }}></i>
            <h3 id="processing-status">{t("settings.processing")}</h3>
          </div>
        )}

        <h3 id="result">{status}</h3>
      </span>

      <hr style={{ margin: "30px auto", width: "60%" }} />

      <LegacyImportPanel
        defaultConflictMode="merge"
        onImported={() => void snapshotsQuery.refetch()}
      />

      <hr style={{ margin: "30px auto", width: "60%" }} />

      <h3>{t("settings.importSnapshotsTitle")}</h3>
      <span
        style={{
          display: "block",
          margin: "0 auto 16px",
          fontSize: FONT_SIZE_SM,
          width: "80%",
          textAlign: "center",
        }}
      >
        {t("settings.importSnapshotsDescription")}
      </span>

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

      <hr style={{ margin: "30px auto", width: "60%" }} />

      <h3>{t("settings.autoSnapshotsTitle")}</h3>
      <span
        style={{
          display: "block",
          margin: "0 auto 16px",
          fontSize: FONT_SIZE_SM,
          width: "80%",
          textAlign: "center",
        }}
      >
        {t("settings.autoSnapshotsDescription")}
      </span>

      <h3 style={{ marginTop: 8 }}>{t("settings.snapshotScheduleTitle")}</h3>
      <span
        style={{
          display: "block",
          margin: "0 auto 12px",
          fontSize: FONT_SIZE_SM,
          width: "80%",
          textAlign: "center",
        }}
      >
        {t("settings.snapshotScheduleDescription")}
      </span>

      {/* `flex-wrap` + a centred `justify-content` is the whole narrow-screen story here: each
          label/control pair is an `inline-flex` item, so on a phone they wrap onto their own rows
          instead of widening the page. */}
      <div
        style={{
          display: "flex",
          flexWrap: "wrap",
          alignItems: "center",
          justifyContent: "center",
          gap: "8px 16px",
          fontSize: FONT_SIZE_SM,
        }}
      >
        <label style={{ display: "inline-flex", alignItems: "center", gap: 8 }}>
          <span>{t("settings.snapshotScheduleEnabled")}</span>
          <Switch
            checked={scheduleEnabled}
            onCheckedChange={setScheduleEnabled}
            aria-label={t("settings.snapshotScheduleEnabled") ?? undefined}
          />
        </label>
        <label
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: 8,
            opacity: scheduleEnabled ? 1 : 0.6,
          }}
        >
          <span>{t("settings.snapshotScheduleDailyAt")}</span>
          <Input
            type="time"
            value={dailyAt}
            disabled={!scheduleEnabled}
            onValueChange={setDailyAt}
            aria-label={t("settings.snapshotScheduleDailyAt") ?? undefined}
            style={{ height: 25, width: 120 }}
          />
        </label>
        <label
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: 8,
            opacity: scheduleEnabled ? 1 : 0.6,
          }}
        >
          <span>{t("settings.snapshotScheduleWeeklyDay")}</span>
          <Select
            value={String(weeklyDay)}
            onValueChange={(value) => setWeeklyDay(Number(value))}
            items={WEEKDAYS.map((day) => ({ value: String(day), label: weekdayLabel(day) }))}
            ariaLabel={t("settings.snapshotScheduleWeeklyDay") ?? undefined}
            variant="stdbtn"
            size="sm"
            disabled={!scheduleEnabled}
          />
        </label>
        <Button
          disabled={updateSettings.isPending || !normalizeDailyAt(dailyAt)}
          style={{ minHeight: 26, padding: "2px 14px", fontSize: FONT_SIZE_SM }}
          onClick={() => void handleSaveSchedule()}
        >
          {t("settings.snapshotScheduleSave")}
        </Button>
      </div>

      <div style={{ marginTop: 10, fontSize: FONT_SIZE_SM, opacity: 0.8 }}>
        {schedule && !schedule.enabled && <span>{t("settings.snapshotScheduleOff")}</span>}
        {schedule && schedule.enabled && (
          <>
            <span style={{ display: "block" }}>
              {t("settings.snapshotScheduleLastDaily", {
                value:
                  scheduleInstantLabel(schedule.last_daily_at) ??
                  t("settings.snapshotScheduleNever"),
              })}
              {" · "}
              {t("settings.snapshotScheduleNextDaily", {
                value:
                  scheduleInstantLabel(schedule.next_daily_at) ??
                  t("settings.snapshotScheduleNever"),
              })}
            </span>
            <span style={{ display: "block" }}>
              {t("settings.snapshotScheduleLastWeekly", {
                value:
                  scheduleInstantLabel(schedule.last_weekly_at) ??
                  t("settings.snapshotScheduleNever"),
              })}
              {" · "}
              {t("settings.snapshotScheduleNextWeekly", {
                value:
                  scheduleInstantLabel(schedule.next_weekly_at) ??
                  t("settings.snapshotScheduleNever"),
              })}
            </span>
          </>
        )}
      </div>

      {scheduleStatus && (
        <span style={{ display: "block", marginTop: 8, fontSize: FONT_SIZE_SM }}>
          {scheduleStatus}
        </span>
      )}

      <div style={{ marginTop: 16 }}>
        <Button
          disabled={createBackup.isPending}
          style={{ minHeight: 34, padding: "4px 16px" }}
          onClick={() => void handleCreateSnapshot()}
        >
          {t("settings.autoSnapshotsCreateNow")}
        </Button>
      </div>

      {backupsQuery.data && (
        <span
          style={{
            display: "block",
            margin: "12px auto 0",
            fontSize: FONT_SIZE_SM,
            opacity: 0.7,
          }}
        >
          {t("settings.autoSnapshotsRetention", {
            daily: backupsQuery.data.daily_keep,
            weekly: backupsQuery.data.week_keep,
          })}
        </span>
      )}

      {backupsQuery.isLoading && (
        <span style={{ display: "block", marginTop: 12, fontSize: FONT_SIZE_SM, opacity: 0.7 }}>
          {t("settings.autoSnapshotsLoading")}
        </span>
      )}
      {backupsQuery.isError && (
        <span style={{ display: "block", marginTop: 12, fontSize: FONT_SIZE_SM }}>
          {t("settings.autoSnapshotsError", { error: String(backupsQuery.error) })}
        </span>
      )}
      {backupsQuery.data && backupsQuery.data.entries.length === 0 && (
        <span style={{ display: "block", marginTop: 12, fontSize: FONT_SIZE_SM, opacity: 0.7 }}>
          {t("settings.autoSnapshotsEmpty")}
        </span>
      )}

      {backupsQuery.data && backupsQuery.data.entries.length > 0 && (
        <div className="backup-records-scroll" style={{ marginTop: 16 }}>
          <div
            className="backup-records backup-records--auto-snapshots"
            style={{ fontSize: FONT_SIZE_SM }}
          >
            <div className="backup-records-head">
              <span style={{ fontWeight: "bold" }}>{t("settings.autoSnapshotsName")}</span>
              <span style={{ fontWeight: "bold" }}>{t("settings.autoSnapshotsReason")}</span>
              <span style={{ fontWeight: "bold" }}>{t("settings.autoSnapshotsSize")}</span>
              <span style={{ fontWeight: "bold" }}>{t("settings.autoSnapshotsCreatedAt")}</span>
              <span style={{ fontWeight: "bold" }}>{t("settings.autoSnapshotsSummary")}</span>
              <span />
            </div>
            {backupsQuery.data.entries.map((entry) => (
              <div className="backup-records-row" key={entry.name}>
                <span className="backup-cell-name" title={entry.name}>
                  {entry.name}
                </span>
                <span>{reasonLabel(entry.reason)}</span>
                <span>{formatBytes(entry.size_bytes)}</span>
                <span>{new Date(entry.created_at).toLocaleString()}</span>
                <span className="backup-cell-summary">{summaryLabel(entry.summary)}</span>
                <span className="backup-cell-actions">
                  {/* A real link, so the browser downloads the file itself (and can save/open it in
                      a new tab) instead of the page buffering it through fetch. */}
                  <a
                    href={`/api/backups/${encodeURIComponent(entry.name)}`}
                    download={entry.name}
                    title={t("settings.autoSnapshotsDownload")}
                    aria-label={t("settings.autoSnapshotsDownload")}
                    style={{
                      display: "inline-flex",
                      alignItems: "center",
                      justifyContent: "center",
                      width: 24,
                      height: 18,
                    }}
                  >
                    <i className="fa fa-download" aria-hidden="true" />
                  </a>
                  <Button
                    disabled={!entry.usable || restoreBackup.isPending}
                    title={entry.usable ? undefined : t("settings.autoSnapshotsUnusable")}
                    style={{ minHeight: 24, padding: "2px 10px", fontSize: FONT_SIZE_SM }}
                    onClick={() => void handleRestoreSnapshot(entry)}
                  >
                    {t("settings.autoSnapshotsRestore")}
                  </Button>
                  <Button
                    aria-expanded={expandedSnapshot === entry.name}
                    style={{ minHeight: 24, padding: "2px 10px", fontSize: FONT_SIZE_SM }}
                    onClick={() =>
                      setExpandedSnapshot((current) =>
                        current === entry.name ? null : entry.name,
                      )
                    }
                  >
                    {t("settings.autoSnapshotsDetails")}
                  </Button>
                </span>
                {expandedSnapshot === entry.name && (
                  <div className="backup-row-detail">
                    {entry.summary ? (
                      <div
                        style={{
                          display: "grid",
                          gridTemplateColumns: "auto minmax(0, 1fr)",
                          columnGap: 12,
                          rowGap: 2,
                          padding: "4px 0 6px",
                        }}
                      >
                        {summaryFields(entry.summary).map((field) => (
                          <Fragment key={field.label}>
                            <span style={{ opacity: 0.75 }}>{field.label}</span>
                            <span>{field.value}</span>
                          </Fragment>
                        ))}
                      </div>
                    ) : (
                      <span style={{ opacity: 0.75 }}>—</span>
                    )}
                  </div>
                )}
              </div>
            ))}
          </div>
        </div>
      )}

      <h3 style={{ marginTop: 16 }}>{snapshotStatus}</h3>

      <hr style={{ margin: "30px auto", width: "60%" }} />

      <h3>{t("settings.recentDeletionsTitle")}</h3>
      <span
        style={{
          display: "block",
          margin: "0 auto 16px",
          fontSize: FONT_SIZE_SM,
          width: "80%",
          textAlign: "center",
        }}
      >
        {t("settings.recentDeletionsDescription")}
      </span>

      <Button
        disabled={
          restoreAllDeletions.isPending ||
          !deletionsQuery.data ||
          deletionsQuery.data.entries.length === 0
        }
        style={{ minHeight: 34, padding: "4px 16px" }}
        onClick={() => void handleRestoreAllDeletions()}
      >
        {t("settings.recentDeletionsRestoreAll")}
      </Button>

      {deletionsQuery.data && (
        <span
          style={{
            display: "block",
            margin: "12px auto 0",
            fontSize: FONT_SIZE_SM,
            opacity: 0.7,
          }}
        >
          {t("settings.recentDeletionsRetention", { days: deletionsQuery.data.retention_days })}
        </span>
      )}

      {deletionsQuery.isLoading && (
        <span style={{ display: "block", marginTop: 12, fontSize: FONT_SIZE_SM, opacity: 0.7 }}>
          {t("settings.recentDeletionsLoading")}
        </span>
      )}
      {deletionsQuery.isError && (
        <span style={{ display: "block", marginTop: 12, fontSize: FONT_SIZE_SM }}>
          {t("settings.recentDeletionsError", { error: String(deletionsQuery.error) })}
        </span>
      )}
      {deletionsQuery.data && deletionsQuery.data.entries.length === 0 && (
        <span style={{ display: "block", marginTop: 12, fontSize: FONT_SIZE_SM, opacity: 0.7 }}>
          {t("settings.recentDeletionsEmpty")}
        </span>
      )}

      {deletionsQuery.data && deletionsQuery.data.entries.length > 0 && (
        <div className="backup-records-scroll" style={{ marginTop: 16 }}>
          <div
            className="backup-records backup-records--deletions"
            style={{ fontSize: FONT_SIZE_SM }}
          >
            <div className="backup-records-head">
              <span style={{ fontWeight: "bold" }}>{t("settings.recentDeletionsTitleColumn")}</span>
              <span style={{ fontWeight: "bold" }}>{t("settings.recentDeletionsDeletedAt")}</span>
              <span style={{ fontWeight: "bold" }}>{t("settings.recentDeletionsSource")}</span>
              <span />
            </div>
            {deletionsQuery.data.entries.map((entry) => (
              <div className="backup-records-row" key={entry.id}>
                <span className="backup-cell-title" title={entry.title}>
                  {entry.title}
                </span>
                <span>{new Date(entry.deleted_at).toLocaleString()}</span>
                <span>{sourceLabel(entry.source)}</span>
                <span className="backup-cell-actions">
                  <Button
                    disabled={restoreDeletion.isPending}
                    style={{ minHeight: 24, padding: "2px 10px", fontSize: FONT_SIZE_SM }}
                    onClick={() => void handleRestoreDeletion(entry)}
                  >
                    {t("settings.recentDeletionsRestore")}
                  </Button>
                  {!entry.file_exists && (
                    <span style={{ opacity: 0.7 }}>{t("settings.recentDeletionsFileMissing")}</span>
                  )}
                </span>
              </div>
            ))}
          </div>
        </div>
      )}

      <h3 style={{ marginTop: 16 }}>{deletionStatus}</h3>

      <div style={{ marginTop: 30 }}>
        <Button onClick={() => navigate(routes.library())}>{t("common.returnToLibrary")}</Button>
      </div>
    </div>
  )
}
