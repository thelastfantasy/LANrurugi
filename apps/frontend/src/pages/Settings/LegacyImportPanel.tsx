import type { CSSProperties } from "react"
import { Fragment, useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { waitForJob } from "@/api/client"
import { useImportLegacyCount, useJobs } from "@/api/hooks"
import type { ImportConflictMode, ImportLegacyResult } from "@/api/types"
import { Button, Checkbox, RadioGroup, RadioItem } from "@/components/common-ui/Form"
import { ImportLegacySummary, JobProgressBar } from "@/components/Display"
import { confirmDialog } from "@/dialog"
import { FONT_SIZE_SM } from "@/theme"

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
  )
}

/** Import from a LANraragi backup JSON, shared by the Backup page and the first-run wizard so both
 *  offer the same choices. Waits for a running library scan before allowing the upload. */
export function LegacyImportPanel({
  defaultConflictMode,
  onImported,
  onBusyChange,
}: {
  defaultConflictMode: ImportConflictMode
  onImported?: () => void
  onBusyChange?: (busy: boolean) => void
}) {
  const { t } = useTranslation()
  const legacyFileInputRef = useRef<HTMLInputElement>(null)
  const [conflictMode, setConflictMode] = useState<ImportConflictMode>(defaultConflictMode)
  const [minimizeTags, setMinimizeTags] = useState(false)
  const [importBusy, setImportBusy] = useState(false)
  const [importStatus, setImportStatus] = useState<string>("")
  const [importResult, setImportResult] = useState<ImportLegacyResult | null>(null)

  const importCountQuery = useImportLegacyCount()
  const jobsQuery = useJobs()
  const scan = runningLibraryScan(jobsQuery.data)
  const importBlocked = importBusy || jobsQuery.isPending || scan !== undefined

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
    try {
      const formData = new FormData()
      formData.append("file", file)
      const response = await fetch(
        `/api/database/import-legacy?on_existing=${conflictMode}&minimize_tags=${minimizeTags}`,
        { method: "POST", body: formData },
      )
      const { job } = (await response.json()) as { job: string }
      const jobStatus = await waitForJob(job)

      if (jobStatus.state === "failed") {
        setImportStatus(
          t("settings.importLegacyFailedError", {
            error: jobStatus.error ?? t("edit.unknownError"),
          }),
        )
      } else {
        setImportResult(jobStatus.notes as ImportLegacyResult)
        setImportStatus(t("settings.importLegacyComplete"))
        void importCountQuery.refetch()
        onImported?.()
      }
    } catch (e) {
      setImportStatus(t("settings.importLegacyFailedError", { error: String(e) }))
    } finally {
      setImportBusy(false)
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
          <Button style={{ height: 40, width: "100%" }} disabled={importBlocked}>
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
        {importBusy && (
          <div>
            <i className="fa fa-3x fa-compact-disc fa-spin" style={{ marginTop: 20 }}></i>
          </div>
        )}
        <h3>{importStatus}</h3>
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
