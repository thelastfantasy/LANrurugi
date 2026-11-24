import type { CSSProperties } from "react"
import { Fragment, useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { useBackupJobProgress } from "@/api/hooks"
import { useImportLegacyCount, useJobs } from "@/api/hooks"
import type { BackupJobStatus, ImportConflictMode, ImportLegacyResult } from "@/api/types"
import { Button, Checkbox, RadioGroup, RadioItem } from "@/components/common-ui/Form"
import { ImportLegacySummary, JobProgressBar } from "@/components/Display"
import { confirmDialog } from "@/dialog"
import { FONT_SIZE_SM } from "@/theme"

import { BackupProgressBar } from "./BackupProgressBar"

/** Jobs that catalogue the library. Importing while one runs leaves its unscanned archives
 *  unmatched, since the import pairs archives by file name. */
export const LIBRARY_SCAN_JOB_NAMES = ["startup_scan", "rebuild_index"]

/** The library scan still running, if any. */
export function runningLibraryScan<T extends { name: string; state: string }>(
  jobs: readonly T[] | undefined,
): T | undefined {
  return jobs?.find(
    (job) =>
      LIBRARY_SCAN_JOB_NAMES.includes(job.name) && (job.state === "queued" || job.state === "active"),
  )
}

/** One `ImportConflictMode` example row set — same sample archive across all three modes so only
 * `result` varies, with a fixed `gridTemplateColumns` keeping all three tables' columns aligned. */
function ImportConflictExample({
  resultTitle,
  resultTag,
  resultRating,
}: {
  resultTitle: string
  resultTag: string
  resultRating: string
}) {
  const { t } = useTranslation()
  const rows: [string, string, string, string][] = [
    [
      t("settings.importLegacyExampleFieldTitle"),
      t("settings.importLegacyExampleHereTitle"),
      t("settings.importLegacyExampleLegacyTitle"),
      resultTitle,
    ],
    [
      t("settings.importLegacyExampleFieldTag"),
      t("settings.importLegacyExampleHereTag"),
      t("settings.importLegacyExampleLegacyTag"),
      resultTag,
    ],
    [
      t("settings.importLegacyExampleFieldCategory"),
      t("settings.importLegacyExampleHereCategory"),
      t("settings.importLegacyExampleLegacyCategory"),
      t("settings.importLegacyExampleResultCategory"),
    ],
    [
      t("settings.importLegacyExampleFieldRating"),
      t("settings.importLegacyExampleHereRating"),
      t("settings.importLegacyExampleLegacyRating"),
      resultRating,
    ],
  ]
  return (
    // The four columns have a hard 120px floor each, so the sample can be wider than a phone's
    // viewport on its own; scrolling it inside this wrapper keeps that off the page's own width.
    <div className="backup-records-scroll">
      <div
        style={{
          display: "grid",
          gridTemplateColumns: "auto minmax(120px, auto) minmax(120px, auto) minmax(120px, auto)",
          columnGap: 20,
          rowGap: 0,
          marginTop: 6,
          fontSize: FONT_SIZE_SM,
          opacity: 0.7,
        }}
      >
        <span style={{ padding: "3px 0", fontWeight: "bold" }} />
        <span style={{ padding: "3px 0", fontWeight: "bold" }}>
          {t("settings.importLegacyExampleHere")}
        </span>
        <span style={{ padding: "3px 0", fontWeight: "bold" }}>
          {t("settings.importLegacyExampleLegacy")}
        </span>
        <span style={{ padding: "3px 0", fontWeight: "bold" }}>
          {t("settings.importLegacyExampleResult")}
        </span>
        {rows.map(([field, here, legacy, result], i) => {
          const rowStyle: CSSProperties = {
            padding: "3px 0",
            backgroundColor: i % 2 === 1 ? "rgba(128, 128, 128, 0.08)" : undefined,
          }
          return (
            <Fragment key={field}>
              <span style={rowStyle}>{field}</span>
              <span style={rowStyle}>{here}</span>
              <span style={rowStyle}>{legacy}</span>
              <span style={rowStyle}>{result}</span>
            </Fragment>
          )
        })}
      </div>
    </div>
  )
}

/** Import from a LANraragi backup JSON, shared by the Backup page and the first-run wizard so both
 *  offer the same choices. Waits for a running library scan before allowing the upload. */
export function LegacyImportPanel({
  defaultConflictMode,
  onImported,
  onBusyChange,
  disabled,
}: {
  defaultConflictMode: ImportConflictMode
  onImported?: () => void
  onBusyChange?: (busy: boolean) => void
  /** Another job on the page is running; every control here waits for it. */
  disabled?: boolean
}) {
  const { t } = useTranslation()
  const legacyFileInputRef = useRef<HTMLInputElement>(null)
  const [conflictMode, setConflictMode] = useState<ImportConflictMode>(defaultConflictMode)
  const [minimizeTags, setMinimizeTags] = useState(false)
  const [importBusy, setImportBusy] = useState(false)
  const [importStatus, setImportStatus] = useState<string>("")
  const [importResult, setImportResult] = useState<ImportLegacyResult | null>(null)
  const [jobId, setJobId] = useState<string | null>(null)
  const [startedPerf, setStartedPerf] = useState(0)
  const [runSeconds, setRunSeconds] = useState(0)

  const importCountQuery = useImportLegacyCount()
  const jobsQuery = useJobs()
  const scan = runningLibraryScan(jobsQuery.data)
  const importBlocked = importBusy || jobsQuery.isPending || scan !== undefined || disabled

  /** Same shared bar as the snapshot/export/restore flows. */
  function handleJobTerminal(status: BackupJobStatus) {
    if (status.state === "done") {
      if (status.notes) setImportResult(status.notes as ImportLegacyResult)
      setImportStatus(t("settings.importLegacyComplete"))
      void importCountQuery.refetch()
      onImported?.()
    } else {
      setImportStatus(
        t("settings.importLegacyFailedError", {
          error: status.error ?? t("edit.unknownError"),
        }),
      )
    }
    setImportBusy(false)
    setJobId(null)
  }

  const { status: jobStatus, live: jobLive } = useBackupJobProgress(jobId, handleJobTerminal)

  useEffect(() => {
    if (!jobId) return
    const update = () => setRunSeconds((performance.now() - startedPerf) / 1000)
    update()
    const id = setInterval(update, 100)
    return () => clearInterval(id)
  }, [jobId, startedPerf])

  useEffect(() => {
    onBusyChange?.(importBusy)
  }, [importBusy, onBusyChange])

  async function handleImportLegacy(file: File) {
    // A 2nd-or-later import already has an automatic rollback snapshot, but it only covers what
    // that import touched, not the whole library — nudge toward a full backup first.
    if ((importCountQuery.data?.import_count ?? 0) >= 1) {
      const proceed = await confirmDialog(t("settings.importLegacyRepeatWarning"))
      if (!proceed) {
        if (legacyFileInputRef.current) legacyFileInputRef.current.value = ""
        return
      }
    }

    setImportBusy(true)
    setImportStatus(t("settings.importLegacyInProgress"))
    setImportResult(null)
    setRunSeconds(0)
    try {
      const formData = new FormData()
      formData.append("file", file)
      const response = await fetch(
        `/api/database/import-legacy?on_existing=${conflictMode}&minimize_tags=${minimizeTags}`,
        { method: "POST", body: formData },
      )
      if (!response.ok) throw new Error(`Import request failed with ${response.status}`)
      const { job_id } = (await response.json()) as { job_id: string }
      setStartedPerf(performance.now())
      setJobId(job_id)
    } catch (e) {
      setImportStatus(t("settings.importLegacyFailedError", { error: String(e) }))
      setImportBusy(false)
    } finally {
      if (legacyFileInputRef.current) legacyFileInputRef.current.value = ""
    }
  }

  return (
    <div style={{ textAlign: "center" }}>
      <h3>{t("settings.importFromLegacyTitle")}</h3>
      <span
        style={{
          display: "block",
          margin: "0 auto 16px",
          fontSize: FONT_SIZE_SM,
          width: "80%",
          textAlign: "center",
        }}
      >
        {t("settings.importFromLegacyDescription")}
      </span>
      <div
        style={{
          display: "flex",
          flexDirection: "column",
          gap: 8,
          maxWidth: 720,
          margin: "0 auto",
          textAlign: "left",
        }}
      >
        <div>
          <span style={{ fontSize: FONT_SIZE_SM }}>
            {t("settings.importLegacyConflictModeLabel")}
          </span>
          <RadioGroup
            value={conflictMode}
            onValueChange={setConflictMode}
            disabled={importBusy}
            style={{ display: "flex", flexDirection: "column", gap: 4, marginTop: 4 }}
          >
            <RadioItem value="overwrite">
              <span>
                {t("settings.importLegacyConflictModeOverwrite")}
                <ImportConflictExample
                  resultTitle={t("settings.importLegacyExampleOverwriteResultTitle")}
                  resultTag={t("settings.importLegacyExampleOverwriteResultTag")}
                  resultRating={t("settings.importLegacyExampleOverwriteResultRating")}
                />
              </span>
            </RadioItem>
            <RadioItem value="merge">
              <span>
                {t("settings.importLegacyConflictModeMerge")}
                <ImportConflictExample
                  resultTitle={t("settings.importLegacyExampleMergeResultTitle")}
                  resultTag={t("settings.importLegacyExampleMergeResultTag")}
                  resultRating={t("settings.importLegacyExampleMergeResultRating")}
                />
              </span>
            </RadioItem>
            <RadioItem value="skip">
              <span>
                {t("settings.importLegacyConflictModeSkip")}
                <ImportConflictExample
                  resultTitle={t("settings.importLegacyExampleSkipResultTitle")}
                  resultTag={t("settings.importLegacyExampleSkipResultTag")}
                  resultRating={t("settings.importLegacyExampleSkipResultRating")}
                />
              </span>
            </RadioItem>
          </RadioGroup>
        </div>
        <div style={{ fontSize: FONT_SIZE_SM }}>
          <Checkbox
            id="minimize-tags"
            checked={minimizeTags}
            onCheckedChange={setMinimizeTags}
            disabled={importBusy}
          />{" "}
          <label
            htmlFor="minimize-tags"
            style={{ cursor: importBusy ? "not-allowed" : "pointer" }}
          >
            {t("settings.importLegacyMinimizeTagsLabel")}
          </label>
        </div>
        {scan && (
          <div style={{ fontSize: FONT_SIZE_SM }}>
            <p>{t("settings.importLegacyWaitingForScan")}</p>
            <JobProgressBar job={scan} />
          </div>
        )}
        <span className="fileinput-button" style={{ display: "block" }}>
          <Button style={{ height: 32, width: "100%" }} disabled={importBlocked}>
            {t("settings.importLegacyButton")}
          </Button>
          <input
            ref={legacyFileInputRef}
            type="file"
            name="file"
            accept=".json,application/json"
            disabled={importBlocked}
            onChange={(e) => {
              const file = e.target.files?.[0]
              if (file) void handleImportLegacy(file)
            }}
          />
        </span>
      </div>

      <span
        style={{
          display: "block",
          margin: "16px auto 0",
          fontSize: FONT_SIZE_SM,
          width: "80%",
          textAlign: "center",
        }}
      >
        {jobId ? (
          <BackupProgressBar
            percent={jobStatus?.percent ?? 0}
            label={t("settings.backupRunProgressImport", {
              percent: (jobStatus?.percent ?? 0).toFixed(1),
              seconds: runSeconds.toFixed(1),
            })}
            detail={
              (jobStatus?.bytes_expected ?? 0) > 0
                ? `${((jobStatus?.bytes_written ?? 0) / (1024 * 1024)).toFixed(1)} MiB / ${(
                    (jobStatus?.bytes_expected ?? 0) / (1024 * 1024)
                  ).toFixed(1)} MiB`
                : undefined
            }
            note={!jobLive ? t("settings.backupRunRealtimeUnavailable") : undefined}
          />
        ) : (
          importStatus && <h3>{importStatus}</h3>
        )}
      </span>

      {importResult && (
        <ImportLegacySummary
          result={importResult}
          style={{ margin: "16px auto", textAlign: "left" }}
        />
      )}
    </div>
  )
}
