import { useQueryClient } from "@tanstack/react-query"
import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { FaCheck, FaPen, FaPlus, FaTrash, FaXmark } from "react-icons/fa6"
import { PiPushPin, PiPushPinFill } from "react-icons/pi"
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
  useSetBackupNote,
  useSettings,
  useUnpinBackup,
  useUpdateSettings,
} from "@/api/hooks"
import type { BackupEntry, BackupJobStatus, BackupScheduleItem, BackupSummary } from "@/api/types"
import { Button, IconButton, Switch } from "@/components/common-ui/Form"
import { Tabs } from "@/components/common-ui/Tabs"
import { choiceDialog, confirmDialog, typedConfirmDialog } from "@/dialog"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { absoluteTimestamp, formatRelativeTime } from "@/lib/relativeTime"
import { routes } from "@/lib/routes"
import { describeCron } from "@/schedule/cronText"
import { labelAfterPreset } from "@/schedule/planLabel"
import { FONT_SIZE_SM, useApplyTheme } from "@/theme"
import { toast } from "@/toast"

import { BackupProgressBar } from "./BackupProgressBar"
import { LegacyImportPanel } from "./LegacyImportPanel"

/** How many `pre-*` exports retention keeps (`PRE_KEEP` in `crates/lanrurugi-api/src/snapshots.rs`). */
const PRE_KEEP = 5

/** Fallback for the per-schedule full-snapshot budget when an older server omits `full_keep`. */
const SNAPSHOT_KEEP_FALLBACK = 3

/** The server refuses more than this many plans; the Add button enforces it too. */
const MAX_PLANS = 10
const NOTE_MAX_LENGTH = 200

/** The seven cron fields, in order: second, minute, hour, day, month, weekday, year. */
const CRON_FIELDS = ["second", "minute", "hour", "day", "month", "weekday", "year"] as const

/** Preset → 7-field cron. `custom` keeps whatever the user has typed, and any hand edit of a
 *  field switches the plan to `custom` for the same reason. */
// One-click shapes for the cases users actually schedule. Selecting one only rewrites the seven
// fields; the expression stays the source of truth, so editing any field simply deselects the row.
const CRON_PRESETS = [
  { value: "every-5-minutes", cron: "0 */5 * * * * *" },
  { value: "every-15-minutes", cron: "0 */15 * * * * *" },
  { value: "hourly", cron: "0 0 * * * * *" },
  { value: "every-6-hours", cron: "0 0 */6 * * * *" },
  { value: "daily", cron: "0 0 3 * * * *" },
  { value: "weekdays", cron: "0 0 3 * * 1-5 *" },
  { value: "weekly", cron: "0 0 3 * * 0 *" },
  { value: "monthly", cron: "0 0 3 1 * * *" },
  // `0#1` is the backend's own n-th-weekday syntax: the first Sunday of every month.
  { value: "monthly-first-weekday", cron: "0 0 3 * * 0#1 *" },
  { value: "every-3-months-first-weekday", cron: "0 0 3 * */3 0#1 *" },
  { value: "yearly-march-first-weekday", cron: "0 0 0 * 3 0#1 *" },
] as const

/** The cron a newly added plan starts from. */
const NEW_PLAN_CRON = "0 0 3 * * * *"

/** Splits a cron string into exactly seven fields, padding a shorter (still-being-typed) one with
 *  `*` so the seven controls never shift under the user. */
function splitCron(value: string): string[] {
  const parts = value.trim().split(/\s+/).filter(Boolean)
  return Array.from({ length: CRON_FIELDS.length }, (_, index) => parts[index] ?? "*")
}

function joinCron(parts: string[]): string {
  return parts.join(" ")
}

/** A plan id that starts with `plan-` and is unique within this list — the setting stores it, and
 *  each snapshot records it as `schedule_id`, so it must stay stable across saves. */
function newPlanId(existing: readonly string[]): string {
  const base = `plan-${Date.now().toString(36)}`
  const taken = new Set(existing)
  if (!taken.has(base)) return base
  let suffix = 2
  while (taken.has(`${base}-${suffix}`)) suffix += 1
  return `${base}-${suffix}`
}

/** The `snapshot_schedules` setting is a JSON array string. Tolerant on read: a corrupt value or a
 *  shape an older/newer build wrote is dropped rather than crashing the page. */
function parsePlanSettings(text: string | undefined): BackupScheduleItem[] | null {
  if (!text || !text.trim()) return null
  try {
    const parsed: unknown = JSON.parse(text)
    if (!Array.isArray(parsed)) return null
    return parsed.flatMap((raw, index) => {
      if (!raw || typeof raw !== "object") return []
      const item = raw as Record<string, unknown>
      const cron = typeof item.cron === "string" ? item.cron : ""
      if (!cron.trim()) return []
      return [
        {
          id: typeof item.id === "string" && item.id ? item.id : `plan-${index + 1}`,
          label: typeof item.label === "string" ? item.label : "",
          enabled: item.enabled !== false,
          preset: typeof item.preset === "string" ? item.preset : "custom",
          cron,
          last_fire_at: null,
          next_runs: [],
        },
      ]
    })
  } catch {
    return null
  }
}

/** `HH:mm:ss` in the reader's own timezone — the live timer's start/end labels. */
function formatClock(date: Date): string {
  const pad = (n: number) => String(n).padStart(2, "0")
  return `${pad(date.getHours())}:${pad(date.getMinutes())}:${pad(date.getSeconds())}`
}

/** `YYYY-MM-DD HH:mm:ss` — the summary line quotes one exact instant; cards show relative time. */
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

/** One upcoming run, compact: `11/30 周一 03:00`, carrying the year only when it isn't this one and
 *  the seconds only when the cron's second field asks for them. Full `YYYY/MM/DD HH:mm:ss` ten times
 *  over is a wall of numbers nobody reads. */
function scheduleRunLabel(
  value: string,
  timezone: string | undefined,
  showSeconds: boolean,
): string | null {
  const date = new Date(value)
  if (Number.isNaN(date.getTime())) return null
  try {
    // `formatToParts`, not `format`: a locale's year is "2026年" and its date order is its own,
    // so the parts are assembled here into one fixed `YYYY/MM/DD 周一 03:00` shape.
    const parts = new Intl.DateTimeFormat(undefined, {
      timeZone: timezone || undefined,
      year: "numeric",
      month: "2-digit",
      day: "2-digit",
      weekday: "short",
      hour: "2-digit",
      minute: "2-digit",
      ...(showSeconds ? { second: "2-digit" } : {}),
      hourCycle: "h23",
    }).formatToParts(date)
    const get = (type: Intl.DateTimeFormatPartTypes) =>
      parts.find((part) => part.type === type)?.value ?? ""
    const thisYear = new Date().getFullYear()
    const day =
      get("year") === String(thisYear)
        ? `${get("month")}/${get("day")}`
        : `${get("year")}/${get("month")}/${get("day")}`
    const time = showSeconds
      ? `${get("hour")}:${get("minute")}:${get("second")}`
      : `${get("hour")}:${get("minute")}`
    return `${day} ${get("weekday")} ${time}`
  } catch {
    return date.toLocaleString()
  }
}

/** The alias row on one snapshot/export card. Read-only until the pencil is clicked, so a note
 *  never turns every card into a form; failures stay on this line instead of a dialog. */
function BackupNote({ entry }: { entry: BackupEntry }) {
  const { t } = useTranslation()
  const setNote = useSetBackupNote()
  const [editing, setEditing] = useState(false)
  const [draft, setDraft] = useState("")
  const [error, setError] = useState("")

  const begin = () => {
    setDraft(entry.note ?? "")
    setError("")
    setEditing(true)
  }

  const cancel = () => {
    setEditing(false)
    setError("")
  }

  const save = () => {
    const value = draft.trim()
    if (value.length > NOTE_MAX_LENGTH) {
      setError(t("settings.backupNoteTooLong", { max: NOTE_MAX_LENGTH }))
      return
    }
    setNote.mutate(
      { name: entry.name, note: value === "" ? null : value },
      {
        onSuccess: () => {
          setEditing(false)
          setError("")
        },
        onError: (cause) => setError(t("settings.backupNoteSaveFailed", { error: String(cause) })),
      },
    )
  }

  if (editing) {
    return (
      <div className="backup-note backup-note--editing">
        <input
          className="stdinput backup-note-input"
          value={draft}
          placeholder={t("settings.backupNotePlaceholder")}
          aria-label={t("settings.backupNoteEdit")}
          autoFocus
          onChange={(event) => setDraft(event.target.value)}
          onKeyDown={(event) => {
            if (event.key === "Enter") {
              event.preventDefault()
              save()
            } else if (event.key === "Escape") {
              event.preventDefault()
              cancel()
            }
          }}
        />
        <button
          type="button"
          className="stdbtn backup-note-btn"
          disabled={setNote.isPending}
          title={t("settings.backupNoteSave")}
          aria-label={t("settings.backupNoteSave")}
          onClick={save}
        >
          <FaCheck aria-hidden="true" />
        </button>
        <button
          type="button"
          className="stdbtn backup-note-btn"
          disabled={setNote.isPending}
          title={t("settings.backupNoteCancel")}
          aria-label={t("settings.backupNoteCancel")}
          onClick={cancel}
        >
          <FaXmark aria-hidden="true" />
        </button>
        {error && (
          <p className="backup-note-error" role="alert">
            {error}
          </p>
        )}
      </div>
    )
  }

  if (!entry.note) {
    return (
      <div className="backup-note">
        <button type="button" className="stdbtn backup-note-add" onClick={begin}>
          <FaPen aria-hidden="true" /> {t("settings.backupNoteAdd")}
        </button>
      </div>
    )
  }

  return (
    <div className="backup-note">
      <span className="backup-note-text" title={entry.note}>
        {entry.note}
      </span>
      <button
        type="button"
        className="stdbtn backup-note-edit"
        title={t("settings.backupNoteEdit")}
        aria-label={t("settings.backupNoteEdit")}
        onClick={begin}
      >
        <FaPen aria-hidden="true" />
      </button>
    </div>
  )
}

/** Mirrors legacy's backup/restore layout (backup/restore actions, spinner, return button) using
 * common-ui `Button`s instead of legacy's table markup; status is a plain text line, no progress bar. */
export function Backup() {
  const { t, i18n } = useTranslation()
  const navigate = useNavigate()
  const location = useLocation()
  const fileInputRef = useRef<HTMLInputElement>(null)
  // "Custom" focuses the seconds field, which is where an expression diverging from every preset
  // normally starts — a ref per plan id because only the open row has fields to focus.
  const secondsRefs = useRef(new Map<string, HTMLInputElement | null>())
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
  /** Full snapshots kept per schedule. The server owns the number; the page only edits it. */
  const fullKeep = backupsQuery.data?.full_keep ?? SNAPSHOT_KEEP_FALLBACK
  const [keepStatus, setKeepStatus] = useState("")
  const saveFullKeep = async (raw: string, event?: { currentTarget: HTMLInputElement }) => {
    const parsed = Number.parseInt(raw, 10)
    if (!Number.isFinite(parsed) || parsed < 1 || parsed > 365) {
      setKeepStatus(t("settings.snapshotKeepInvalid"))
      // Put the field back on the saved value rather than leaving an unsaveable number on screen.
      if (event) event.currentTarget.value = String(fullKeep)
      return
    }
    setKeepStatus("")
    if (parsed === fullKeep) return
    try {
      await updateSettings.mutateAsync({ snapshot_full_keep: parsed })
      await queryClient.invalidateQueries({ queryKey: ["backups"] })
    } catch (error) {
      setKeepStatus(String(error))
    }
  }
  const createBackup = useCreateBackup()
  const createExport = useCreateExport()
  const restoreBackup = useRestoreBackup()
  const deleteBackup = useDeleteBackup()
  const pinBackup = usePinBackup()
  const unpinBackup = useUnpinBackup()

  // The plan list saves through the page-wide settings path (`PUT /api/settings`, one
  // `snapshot_schedules` JSON string) and then re-reads `/api/backups` for the instants the server
  // computed from the new crons.
  const settingsQuery = useSettings()
  const updateSettings = useUpdateSettings()
  const [plans, setPlans] = useState<BackupScheduleItem[]>([])
  const [scheduleStatus, setScheduleStatus] = useState("")
  /** The plan whose cron is being edited — only that one is previewed, so typing never fans out
   *  into ten preview requests. */
  const [editingPlanId, setEditingPlanId] = useState<string | null>(null)

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
  // `/api/backups` is authoritative (it also carries the computed instants); the raw setting is a
  // fallback so the list still round-trips if only one of the two endpoints has answered yet.
  const settingsPlans = parsePlanSettings(settingsQuery.data?.snapshot_schedules)
  const serverPlans = schedule?.items ?? settingsPlans ?? []
  // Adjust state during render (the same pattern `SettingsPage` uses for `language`): re-seed the
  // list only when the saved content actually changed. Keying off the content and not `next_runs`
  // means a background refetch never discards unsaved edits just because a scheduled time passed.
  const serverPlansKey = JSON.stringify(
    serverPlans.map((plan) => [plan.id, plan.label, plan.enabled, plan.preset, plan.cron]),
  )
  const [syncedPlansKey, setSyncedPlansKey] = useState<string | null>(null)
  if ((schedule !== null || settingsPlans !== null) && serverPlansKey !== syncedPlansKey) {
    setSyncedPlansKey(serverPlansKey)
    setPlans(serverPlans)
  }
  // The instants are refreshed in place, leaving the user's edits (and this list's identities) be.
  const serverRunsKey = JSON.stringify(
    serverPlans.map((plan) => [plan.id, plan.cron, plan.last_fire_at, plan.next_runs]),
  )
  const [syncedRunsKey, setSyncedRunsKey] = useState<string | null>(null)
  if (syncedRunsKey !== serverRunsKey) {
    setSyncedRunsKey(serverRunsKey)
    setPlans((current) =>
      current.map((plan) => {
        const server = serverPlans.find((item) => item.id === plan.id)
        return server && server.cron === plan.cron
          ? { ...plan, last_fire_at: server.last_fire_at, next_runs: server.next_runs }
          : plan
      }),
    )
  }

  // Only the plan being edited is previewed; a 500ms debounce keeps one keystroke from being one
  // request. The debounce owns the state, so no render-time pass has to clear it.
  const editingPlan = editingPlanId ? (plans.find((plan) => plan.id === editingPlanId) ?? null) : null
  const editingCron = editingPlan?.cron.trim() ?? ""
  const [previewCron, setPreviewCron] = useState<string | null>(null)
  useEffect(() => {
    const id = setTimeout(() => setPreviewCron(editingCron || null), 500)
    return () => clearTimeout(id)
  }, [editingCron])
  const preview = useCronPreview(previewCron)
  // `schedule.timezone` is a display label (`Asia/Tokyo (+09:00)`); `Intl` rejects it, so every
  // formatting call gets the bare id. The label is parsed out as a fallback for older servers.
  const previewTimezone = schedule?.timezone
  const previewTimezoneId = schedule?.timezone_name ?? previewTimezone?.split(" ")[0]
  // A result is only about the plan in view when it was fetched for that plan's current cron.
  const previewIsCurrent = editingPlan !== null && previewCron === editingCron

  const backups = backupsQuery.data?.entries ?? []
  // The one list is split by kind. Anything without an explicit `"export"` counts as a full
  // snapshot, so a mislabelled entry never gets offered a restore that cannot work.
  const snapshots = backups.filter((entry) => entry.kind !== "export")
  const exports = backups.filter((entry) => entry.kind === "export")
  const pinnedSnapshotCount = snapshots.filter((entry) => entry.pinned).length

  /** A reason a newer server adds isn't in this build's locale files — show it verbatim. */
  const reasonLabel = (reason: string) =>
    t(`settings.snapshotReason.${reason}`, { defaultValue: reason })
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

  /** A plan's display name: whatever the user typed, or a positional fallback so two unnamed plans
   *  are still tellable apart in the list and in the snapshot group headers. */
  const planName = (plan: BackupScheduleItem, index: number) =>
    plan.label.trim() || t("settings.snapshotPlanDefaultName", { index: index + 1 })
  /** A `0` second field means "on the minute" — printing `:00` on every line is noise. */
  const planShowsSeconds = (cron: string) => (splitCron(cron)[0] ?? "0") !== "0"
  /** The next fire instant a plan row shows: the live preview while that plan is being edited,
   *  otherwise whatever `/api/backups` computed for it. */
  const planNextRun = (plan: BackupScheduleItem): string | null => {
    if (editingPlanId === plan.id && previewIsCurrent && preview.data) {
      if (!preview.data.valid) return null
      const next = preview.data.next_runs?.[0]
      return next ? scheduleRunLabel(next, previewTimezoneId, planShowsSeconds(plan.cron)) : null
    }
    const server = schedule?.items.find((item) => item.id === plan.id)
    const next = (server && server.cron === plan.cron ? server.next_runs : plan.next_runs)?.[0]
    return next ? scheduleRunLabel(next, previewTimezoneId, planShowsSeconds(plan.cron)) : null
  }

  /** The newest snapshot, for the one-line summary above the groups. */
  const latestSnapshotAt = (() => {
    let newest: { at: number; text: string } | null = null
    for (const entry of snapshots) {
      const at = new Date(entry.created_at).getTime()
      if (Number.isNaN(at) || (newest !== null && at <= newest.at)) continue
      newest = { at, text: formatDateTime(new Date(entry.created_at)) }
    }
    return newest === null ? null : newest.text
  })()

  /** Snapshots grouped the way the plan list reads: one grid per saved plan in list order, then one
   *  last grid for everything unattributable — manual runs, and snapshots a deleted plan left. */
  const snapshotGroups = (() => {
    const planIds = new Set(serverPlans.map((plan) => plan.id))
    const groups = serverPlans.map((plan, index) => {
      // Pinned first within a group; the rest keeps the server's own order untouched.
      const owned = snapshots.filter((entry) => entry.schedule_id === plan.id)
      const next = plan.next_runs[0]
      return {
        key: plan.id,
        title: planName(plan, index),
        nextRun: next
          ? scheduleRunLabel(next, previewTimezoneId, planShowsSeconds(plan.cron))
          : null,
        entries: [
          ...owned.filter((entry) => entry.pinned),
          ...owned.filter((entry) => !entry.pinned),
        ],
      }
    })
    const unowned = snapshots.filter(
      (entry) => !entry.schedule_id || !planIds.has(entry.schedule_id),
    )
    if (unowned.length > 0 || groups.length === 0) {
      groups.push({
        key: "__unowned",
        title: t("settings.snapshotGroupManual"),
        nextRun: null,
        entries: [
          ...unowned.filter((entry) => entry.pinned),
          ...unowned.filter((entry) => !entry.pinned),
        ],
      })
    }
    return groups
  })()

  function updatePlan(id: string, patch: Partial<BackupScheduleItem>) {
    setPlans((current) => current.map((plan) => (plan.id === id ? { ...plan, ...patch } : plan)))
  }

  /** One row's saved content, for throwing away what was typed into it. A row that never reached
   *  the server (a fresh "add plan") is simply dropped. */
  function withoutEdits(current: BackupScheduleItem[], id: string) {
    const saved = serverPlans.find((plan) => plan.id === id)
    return saved
      ? current.map((plan) => (plan.id === id ? saved : plan))
      : current.filter((plan) => plan.id !== id)
  }

  /** Leaving a row — by cancelling, or by opening another one — discards its unsaved edits. Only
   *  one row is ever editable, so there is never more than one to roll back. */
  function stopEditing() {
    const id = editingPlanId
    if (!id) return
    setPlans((current) => withoutEdits(current, id))
    setEditingPlanId(null)
  }

  function startEditing(id: string) {
    if (editingPlanId !== id && editingPlanId) stopEditing()
    setEditingPlanId(id)
  }

  function addPlan() {
    if (plans.length >= MAX_PLANS) return
    const plan: BackupScheduleItem = {
      id: newPlanId(plans.map((item) => item.id)),
      label: "",
      enabled: true,
      preset: "daily",
      cron: NEW_PLAN_CRON,
      last_fire_at: null,
      next_runs: [],
    }
    // A new plan opens straight into the form, closing whichever row was open before it.
    startEditing(plan.id)
    setPlans((current) => [...withoutEditsOtherThan(current, editingPlanId), plan])
    setScheduleStatus("")
  }

  /** The list with any open row's drafts reverted, so a save triggered elsewhere never writes
   *  half-typed values for a row the user is not looking at. */
  function withoutEditsOtherThan(current: BackupScheduleItem[], editingId: string | null) {
    return editingId ? withoutEdits(current, editingId) : current
  }

  /** Toggling a saved row takes effect immediately: it is the one control with no form around it. */
  async function togglePlan(plan: BackupScheduleItem, enabled: boolean) {
    const base = editingPlanId === plan.id ? plans : withoutEditsOtherThan(plans, editingPlanId)
    const next = base.map((item) => (item.id === plan.id ? { ...item, enabled } : item))
    if (editingPlanId && editingPlanId !== plan.id) setEditingPlanId(null)
    setPlans(next)
    try {
      await persistPlans(next)
    } catch (e) {
      setPlans(base)
      setScheduleStatus(t("settings.snapshotScheduleSaveFailed", { error: String(e) }))
    }
  }

  /** Writes the whole list as the one `snapshot_schedules` JSON string, then re-reads `/api/backups`
   *  for the instants the server computed from the new crons. */
  async function persistPlans(list: BackupScheduleItem[]) {
    const payload = list.map((plan) => ({
      id: plan.id,
      label: plan.label.trim(),
      enabled: plan.enabled,
      preset: plan.preset,
      cron: joinCron(splitCron(plan.cron)),
    }))
    await updateSettings.mutateAsync({ snapshot_schedules: JSON.stringify(payload) })
    await backupsQuery.refetch()
  }

  /** Deleting a plan asks, in one question, whether the snapshots it produced go with it. Saving the
   *  shortened list comes first: even if a snapshot delete fails, the plan itself is gone. */
  async function handleRemovePlan(plan: BackupScheduleItem, index: number) {
    const name = planName(plan, index)
    const owned = snapshots.filter((entry) => entry.schedule_id === plan.id)
    let alsoSnapshots = false
    if (owned.length === 0) {
      const proceed = await confirmDialog(
        t("settings.snapshotPlanDeleteConfirm", { name }),
        true,
      )
      if (!proceed) return
    } else {
      const choice = await choiceDialog(
        t("settings.snapshotPlanDeleteWithSnapshotsQuestion", { name, count: owned.length }),
        [
          {
            value: "with-snapshots",
            label: t("settings.snapshotPlanDeleteWithSnapshots", { count: owned.length }),
            danger: true,
          },
          { value: "keep-snapshots", label: t("settings.snapshotPlanDeleteKeepSnapshots") },
        ],
      )
      if (choice === null) return
      alsoSnapshots = choice === "with-snapshots"
    }

    // Anything half-typed in another row is dropped rather than saved along with the deletion.
    const nextPlans = withoutEditsOtherThan(
      plans,
      editingPlanId === plan.id ? null : editingPlanId,
    ).filter((item) => item.id !== plan.id)
    setPlans(nextPlans)
    if (editingPlanId === plan.id) setEditingPlanId(null)
    setScheduleStatus("")
    try {
      await persistPlans(nextPlans)
    } catch (e) {
      setScheduleStatus(t("settings.snapshotScheduleSaveFailed", { error: String(e) }))
      return
    }

    if (alsoSnapshots) {
      // Sequential, so the server's own guards (a pinned/last-usable entry) are reported per name
      // rather than swallowed by a parallel `Promise.all`.
      const failed: string[] = []
      for (const entry of owned) {
        try {
          await deleteBackup.mutateAsync(entry.name)
        } catch {
          failed.push(entry.name)
        }
      }
      await backupsQuery.refetch()
      if (failed.length > 0) {
        setScheduleStatus(
          t("settings.snapshotPlanDeleteSnapshotsFailed", {
            failed: failed.length,
            count: owned.length,
            names: failed.join(", "),
          }),
        )
      }
    }
  }

  /** One shared entry point for every long action: start the job, then follow its stream. */
  async function startJob(op: JobOp, request: () => Promise<{ job_id: string }>) {
    if (jobBusy) return
    // Called from an event handler, never during render — the timing mark has to be impure.
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

  /** Saves the whole list — the setting is one JSON blob — then closes the row it came from. */
  async function savePlan() {
    if (plans.length > MAX_PLANS) {
      setScheduleStatus(t("settings.snapshotPlanLimit", { max: MAX_PLANS }))
      return
    }
    if (previewIsCurrent && preview.data?.valid === false) return
    setScheduleStatus("")
    try {
      await persistPlans(plans)
      setEditingPlanId(null)
      toast({ heading: t("settings.snapshotScheduleSaved") ?? undefined, icon: "success" })
    } catch (e) {
      setScheduleStatus(t("settings.snapshotScheduleSaveFailed", { error: String(e) }))
    }
  }

  /** A full `.rdb` snapshot replaces the whole dataset, so it gets a stronger gate than the JSON
   *  export restore below: the warning spell out what is overwritten, and the button stays disabled
   *  until the verb is typed. */
  async function handleRestoreFullSnapshot(entry: BackupEntry) {
    const proceed = await typedConfirmDialog(
      <>
        <p>
          <strong>{t("settings.snapshotRestoreWarnTitle")}</strong>
        </p>
        <p>{t("settings.snapshotRestoreWarnReplace", { name: entry.name })}</p>
        <p>{t("settings.snapshotRestoreWarnCreated", { createdAt: new Date(entry.created_at).toLocaleString() })}</p>
        <ul className="dialog-warning-list">
          <li>{t("settings.snapshotRestoreWarnLose")}</li>
          <li>{t("settings.snapshotRestoreWarnLogout")}</li>
          <li>{t("settings.snapshotRestoreWarnSafety")}</li>
        </ul>
      </>,
      t("settings.snapshotRestoreConfirmWord") ?? "",
    )
    if (!proceed) return
    await startJob("restore", () => restoreBackup.mutateAsync(entry.name))
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

    // Export restore: metadata only, applied by the server job (a full `.rdb` goes through
    // `handleRestoreFullSnapshot` above).
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
        <p className="backup-block-note">
          {t("settings.snapshotScheduleDescription")}{" "}
          {t("settings.snapshotScheduleTimezone", { timezone: previewTimezone ?? "" })}{" "}
          {t("settings.snapshotScheduleTimezoneFollow")}
        </p>

        {plans.length === 0 && <p className="backup-state">{t("settings.snapshotPlanEmpty")}</p>}

        <div className="backup-plan-list">
          {plans.map((plan, index) => {
            const name = planName(plan, index)
            const fields = splitCron(plan.cron)
            const isEditing = editingPlanId === plan.id
            const nextRun = planNextRun(plan)
            const lastRun = plan.last_fire_at
              ? formatRelativeTime(plan.last_fire_at, t, i18n.language)
              : null
            const lastRunExact = scheduleInstantLabel(plan.last_fire_at, previewTimezoneId)
            // The row speaks in sentences; the expression itself is only a tooltip (and the edit
            // form's inputs), and stands in on its own when nothing can be said about it.
            const description = describeCron(plan.cron, t, previewTimezoneId)
            const presetLabelOf = (value: string) =>
              t(`settings.snapshotCronPreset.${value}`, { defaultValue: value })
            const presetLabels = CRON_PRESETS.map((item) => presetLabelOf(item.value))
            const presetMatches = CRON_PRESETS.some((item) => item.cron === fields.join(" "))
            const runLine = (
              <>
                <span className="backup-plan-run">
                  {t("settings.snapshotPlanNextRun", {
                    value: nextRun ?? t("settings.snapshotScheduleNever"),
                  })}
                </span>
                {lastRun && (
                  <>
                    <span className="backup-plan-run-sep" aria-hidden="true">
                      ·
                    </span>
                    <span className="backup-plan-run" title={lastRunExact ?? undefined}>
                      {t("settings.snapshotPlanLastRun", { value: lastRun })}
                    </span>
                  </>
                )}
              </>
            )

            // Saved rows are read-only: nothing here is an input, so a plan list reads as a list
            // instead of seven form fields per row.
            if (!isEditing) {
              return (
                <div className="backup-plan-row" key={plan.id}>
                  <div className="backup-plan-summary">
                    <div className="backup-plan-summary-main">
                      <Switch
                        id={`backup-plan-enabled-${plan.id}`}
                        checked={plan.enabled}
                        aria-label={`${name} ${t("settings.snapshotScheduleEnabled")}`}
                        onCheckedChange={(checked) => void togglePlan(plan, checked)}
                      />
                      <span className="backup-plan-title">{name}</span>
                    </div>
                    <div className="backup-plan-summary-rest">
                      {/* A preset-named plan already says its cadence in the title; repeating the
                          same words as a sentence right next to it is noise. */}
                      {description.trim() !== name.trim() && (
                        <span className="backup-plan-cron" title={plan.cron}>
                          {description ? (
                            t("settings.snapshotPlanSentence", { schedule: description })
                          ) : (
                            <code className="backup-plan-cron-raw backup-plan-cron-raw--primary">
                              {plan.cron}
                            </code>
                          )}
                        </span>
                      )}
                      {runLine}
                      <div className="backup-plan-actions">
                        <button
                          type="button"
                          className="stdbtn backup-plan-action"
                          title={t("settings.snapshotPlanEdit")}
                          aria-label={`${name} ${t("settings.snapshotPlanEdit")}`}
                          onClick={() => startEditing(plan.id)}
                        >
                          <FaPen aria-hidden="true" />
                        </button>
                        <button
                          type="button"
                          className="stdbtn stdbtn-danger backup-plan-action"
                          title={t("settings.snapshotPlanDelete")}
                          aria-label={`${name} ${t("settings.snapshotPlanDelete")}`}
                          onClick={() => void handleRemovePlan(plan, index)}
                        >
                          <FaTrash aria-hidden="true" />
                        </button>
                      </div>
                    </div>
                  </div>
                </div>
              )
            }

            return (
              <div className="backup-plan-row" key={plan.id}>
                <div className="backup-plan-row-head">
                  <Switch
                    id={`backup-plan-enabled-${plan.id}`}
                    checked={plan.enabled}
                    aria-label={`${name} ${t("settings.snapshotScheduleEnabled")}`}
                    onCheckedChange={(checked) => updatePlan(plan.id, { enabled: checked })}
                  />
                  <input
                    className="stdinput backup-control backup-plan-name"
                    value={plan.label}
                    placeholder={name}
                    aria-label={t("settings.snapshotPlanName")}
                    onChange={(e) => updatePlan(plan.id, { label: e.target.value })}
                  />
                </div>

                <div className="backup-cron-fields">
                  {CRON_FIELDS.map((field, fieldIndex) => (
                    <label className="backup-cron-field" key={field}>
                      <span>{cronFieldLabel(field)}</span>
                      <input
                        ref={
                          fieldIndex === 0
                            ? (element) => {
                                secondsRefs.current.set(plan.id, element)
                              }
                            : undefined
                        }
                        className="stdinput backup-control backup-control--cron"
                        value={fields[fieldIndex]}
                        aria-label={`${name} ${cronFieldLabel(field)}`}
                        onChange={(e) => {
                          const next = [...fields]
                          next[fieldIndex] = e.target.value.trim() || "*"
                          // A hand-edited cron is no longer any preset, including the one it came from.
                          updatePlan(plan.id, { cron: joinCron(next), preset: "custom" })
                        }}
                      />
                    </label>
                  ))}
                </div>

                <span className="backup-preset-label">
                  {t("settings.snapshotCronPresetLabel")}
                </span>
                <div className="backup-preset-row">
                  {CRON_PRESETS.map((item) => {
                    const active = item.cron === fields.join(" ")
                    return (
                      <button
                        key={item.value}
                        type="button"
                        className={`stdbtn backup-preset${active ? " queue-tab-active" : ""}`}
                        aria-pressed={active}
                        // Hovering spells out both halves: what the shape means, and the expression
                        // it writes, so a preset never has to be taken on trust.
                        title={t("settings.snapshotCronPresetTitle", {
                          schedule:
                            describeCron(item.cron, t, previewTimezoneId) ||
                            t("settings.snapshotPlanSentenceInvalid"),
                          cron: item.cron,
                        })}
                        onClick={() =>
                          updatePlan(plan.id, {
                            preset: item.value,
                            cron: item.cron,
                            // Clicking a preset names the plan too, so a plan built this way is not
                            // saved with a blank name — unless the user wrote that name themselves.
                            label: labelAfterPreset(plan.label, presetLabelOf(item.value), presetLabels),
                          })
                        }
                      >
                        {presetLabelOf(item.value)}
                      </button>
                    )
                  })}
                  {/* Highlighted whenever the expression matches no preset, so "nothing selected
                      above" always has a visible explanation and a way back to a plain expression. */}
                  <button
                    type="button"
                    className={`stdbtn backup-preset${presetMatches ? "" : " queue-tab-active"}`}
                    aria-pressed={!presetMatches}
                    onClick={() => {
                      updatePlan(plan.id, { preset: "custom" })
                      secondsRefs.current.get(plan.id)?.focus()
                    }}
                  >
                    {t("settings.snapshotCronPreset.custom")}
                  </button>
                </div>

                {/* The same sentence the saved row shows, computed from the draft rather than the
                    server: this is the reviewer's line, so the expression sits beside it. */}
                <p className="backup-plan-sentence">
                  {previewIsCurrent && preview.data?.valid === false ? (
                    t("settings.snapshotPlanSentenceInvalid")
                  ) : description ? (
                    <>
                      <span>{t("settings.snapshotPlanSentence", { schedule: description })}</span>
                      <span className="backup-plan-sentence-sep" aria-hidden="true">
                        ·
                      </span>
                      <code className="backup-plan-cron-raw">{plan.cron}</code>
                    </>
                  ) : (
                    <code className="backup-plan-cron-raw backup-plan-cron-raw--primary">
                      {plan.cron}
                    </code>
                  )}
                </p>

                <div className="backup-plan-row-meta">
                  {runLine}
                  {preview.isLoading && (
                    <span>{t("settings.snapshotCronPreviewLoading")}</span>
                  )}
                  {preview.isError && (
                    <span className="backup-plan-error">
                      {t("settings.snapshotCronPreviewFailed", { error: String(preview.error) })}
                    </span>
                  )}
                  {previewIsCurrent && preview.data?.valid === false && (
                    <span className="backup-plan-error">
                      {t("settings.snapshotCronInvalid", { error: preview.data.error ?? "" })}
                    </span>
                  )}
                </div>

                <div className="backup-plan-actions backup-plan-actions--edit">
                  <button
                    type="button"
                    className="stdbtn backup-plan-save"
                    disabled={
                      updateSettings.isPending ||
                      (previewIsCurrent && preview.data?.valid === false)
                    }
                    onClick={() => void savePlan()}
                  >
                    <FaCheck aria-hidden="true" />{" "}
                    {t("settings.snapshotPlanSave")}
                  </button>
                  <button
                    type="button"
                    className="stdbtn backup-plan-cancel"
                    onClick={stopEditing}
                  >
                    {t("settings.snapshotPlanCancel")}
                  </button>
                </div>
              </div>
            )
          })}
        </div>

        <p className="backup-hint">
          {t("settings.snapshotCronRanges")} {t("settings.snapshotCronRangesNth")}
        </p>

        {scheduleStatus && <p className="backup-hint">{scheduleStatus}</p>}

        <div className="backup-form-actions" style={{ marginTop: 16 }}>
          <button
            type="button"
            className="stdbtn backup-btn"
            disabled={plans.length >= MAX_PLANS}
            onClick={addPlan}
          >
            <FaPlus aria-hidden="true" />{" "}
            {t("settings.snapshotPlanAdd")}
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

        {plans.length >= MAX_PLANS && (
          <p className="backup-hint">{t("settings.snapshotPlanLimit", { max: MAX_PLANS })}</p>
        )}

        {jobNode("snapshot")}
        {/* A restore can be started from a snapshot card on this tab, so its progress bar is
            rendered here too — the job itself lives at the page level. */}
        {jobNode("restore")}

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
              {t("settings.snapshotKeepRecent", { keep: fullKeep })}
            </span>
            <span>
              {t("settings.snapshotSummaryLast", {
                value: latestSnapshotAt ?? t("settings.snapshotScheduleNever"),
              })}
              {/* Every instant on this page is the schedule's own wall clock, not the reader's — say
                  which zone that is, or a reader six hours away reads them as their own. */}
              {previewTimezone && (
                <>
                  {" · "}
                  {t("settings.snapshotSummaryTimezone", { timezone: previewTimezone })}
                </>
              )}
            </span>
            {pinnedSnapshotCount > 0 && <span>{t("settings.snapshotPinnedNote")}</span>}
            <label className="backup-keep-editor">
              <span>{t("settings.snapshotKeepLabel")}</span>
              <input
                className="stdinput backup-control--keep"
                type="number"
                min={1}
                max={365}
                step={1}
                defaultValue={fullKeep}
                key={fullKeep}
                aria-label={t("settings.snapshotKeepLabel")}
                onBlur={(event) => void saveFullKeep(event.currentTarget.value, event)}
                onKeyDown={(event) => {
                  if (event.key === "Enter") void saveFullKeep(event.currentTarget.value, event)
                }}
              />
              <span>{t("settings.snapshotKeepUnit")}</span>
            </label>
            {keepStatus && (
              <span className="backup-hint" role="status">
                {keepStatus}
              </span>
            )}
          </div>
        )}

        {backupsQuery.data && snapshots.length === 0 && (
          <p className="backup-state">{t("settings.snapshotEmpty")}</p>
        )}

        {/* One titled grid per plan, in the plan list's own order, then everything that cannot be
            attributed to a current plan (manual runs, and snapshots a deleted plan left behind). */}
        {snapshots.length > 0 &&
          snapshotGroups.map((group) => (
            <section className="backup-group" key={group.key}>
              <h5 className="backup-group-title">
                <span>{group.title}</span>
                {group.nextRun && (
                  <span className="backup-group-next">
                    {t("settings.snapshotPlanNextRun", { value: group.nextRun })}
                  </span>
                )}
              </h5>
              {group.entries.length === 0 ? (
                <p className="backup-hint">{t("settings.snapshotGroupEmpty")}</p>
              ) : (
                <div className="backup-card-grid">
                  {group.entries.map((entry) => (
                    <article className="backup-card" key={entry.name}>
                      <div className="backup-card-head">
                        <span className="backup-card-name" title={entry.name}>
                          {entry.name}
                        </span>
                        <span className="backup-card-badge">{t("settings.backupKindSnapshot")}</span>
                        {entry.pinned && (
                          <span className="backup-card-badge backup-card-badge--pinned">
                            <PiPushPinFill aria-hidden="true" />
                            <span className="backup-card-badge-label">
                              {t("settings.autoSnapshotsPinned")}
                            </span>
                          </span>
                        )}
                      <button
                        type="button"
                        className="stdbtn stdbtn-danger backup-card-delete"
                        disabled={deleteBackup.isPending}
                        title={t("settings.autoSnapshotsDelete") ?? undefined}
                        aria-label={t("settings.autoSnapshotsDelete") ?? undefined}
                        onClick={() => void handleDeleteSnapshot(entry)}
                      >
                        <i className="fa fa-trash" aria-hidden="true" />
                      </button>
                      </div>
                      <BackupNote entry={entry} />
                      <div className="backup-card-meta">
                        <span title={absoluteTimestamp(entry.created_at, i18n.language)}>
                          {formatRelativeTime(entry.created_at, t, i18n.language)}
                        </span>
                        <span>{formatMiB(entry.size_bytes)}</span>
                        <span>{reasonLabel(entry.reason)}</span>
                      </div>
                      <div className="backup-card-actions">
                        <button
                          type="button"
                          className="stdbtn backup-btn"
                          disabled={!entry.usable || jobBusy}
                          title={entry.usable ? undefined : t("settings.autoSnapshotsUnusable")}
                          onClick={() => void handleRestoreFullSnapshot(entry)}
                        >
                          <i className="fa fa-clock-rotate-left" aria-hidden="true" />{" "}
                          {t("settings.snapshotRestore")}
                        </button>
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
                          {entry.pinned ? (
                            <PiPushPinFill aria-hidden="true" />
                          ) : (
                            <PiPushPin aria-hidden="true" />
                          )}{" "}
                          {entry.pinned
                            ? t("settings.autoSnapshotsUnpin")
                            : t("settings.autoSnapshotsPin")}
                        </button>
                      </div>
                    </article>
                  ))}
                </div>
              )}
            </section>
          ))}

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
                      <PiPushPinFill aria-hidden="true" />
                      <span className="backup-card-badge-label">
                        {t("settings.autoSnapshotsPinned")}
                      </span>
                    </span>
                  )}
                <button
                  type="button"
                  className="stdbtn stdbtn-danger backup-card-delete"
                  disabled={deleteBackup.isPending}
                  title={t("settings.autoSnapshotsDelete") ?? undefined}
                  aria-label={t("settings.autoSnapshotsDelete") ?? undefined}
                  onClick={() => void handleDeleteSnapshot(entry)}
                >
                  <i className="fa fa-trash" aria-hidden="true" />
                </button>
                </div>
                <BackupNote entry={entry} />
                <div className="backup-card-meta">
                  <span title={absoluteTimestamp(entry.created_at, i18n.language)}>
                    {formatRelativeTime(entry.created_at, t, i18n.language)}
                  </span>
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
                    {entry.pinned ? (
                      <PiPushPinFill aria-hidden="true" />
                    ) : (
                      <PiPushPin aria-hidden="true" />
                    )}{" "}
                    {entry.pinned ? t("settings.autoSnapshotsUnpin") : t("settings.autoSnapshotsPin")}
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
