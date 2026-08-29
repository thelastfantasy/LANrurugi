import type { CSSProperties } from "react"
import { Fragment, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { waitForJob } from "@/api/client"
import { useDeleteImportSnapshot, useImportLegacyCount, useImportSnapshots } from "@/api/hooks"
import type { ImportConflictMode, ImportLegacyResult } from "@/api/types"
import { IconButton } from "@/components/common-ui/Display/IconButton"
import { Button, Checkbox, RadioGroup, RadioItem } from "@/components/common-ui/Form"
import { confirmDialog } from "@/dialog"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM, useApplyTheme } from "@/theme"

/** A "field / this library / legacy backup / result" example for one `ImportConflictMode` radio
 * option, using the same fixed sample archive for all three modes (see the `importLegacyExample*`
 * i18n keys) so the only thing that varies between them is the `result` column — makes the
 * difference between modes legible at a glance rather than three unrelated prose sentences. A
 * fixed `gridTemplateColumns` (not per-`<dl>` auto width, which left the three modes' tables
 * visibly misaligned — reported live, 2026-08-29) keeps all three modes' columns aligned with each
 * other. Category is included even though it's not affected by `conflictMode` at all (categories
 * are always merged in regardless — see `importFromLegacyDescription`'s own "分类将始终被恢复"),
 * since seeing that row's `result` stay identical across all three modes is itself informative. */
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

// Mirrors legacy's `~/LANraragi/templates/backup.html.tt2` layout (backup/restore actions, a
// processing spinner, a return button) using common-ui `Button`s with hidden file inputs
// triggered via `ref.current.click()`, rather than legacy's own table markup. Doesn't reproduce
// the upload-plugin progress bar (`backup.js`) — status is a plain text line instead.
export function Backup() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const fileInputRef = useRef<HTMLInputElement>(null)
  const [status, setStatus] = useState<string>("")
  const [busy, setBusy] = useState(false)

  const legacyFileInputRef = useRef<HTMLInputElement>(null)
  // "merge" is the default — least information loss of the three modes (unlike "overwrite", it
  // never silently discards a title/summary edit already made on this instance; unlike "skip",
  // it still pulls in whatever tagging — e.g. a `rating:` tag — only the legacy library has).
  const [conflictMode, setConflictMode] = useState<ImportConflictMode>("merge")
  const [minimizeTags, setMinimizeTags] = useState(false)
  const [importBusy, setImportBusy] = useState(false)
  const [importStatus, setImportStatus] = useState<string>("")
  const [importResult, setImportResult] = useState<ImportLegacyResult | null>(null)

  const importCountQuery = useImportLegacyCount()
  const snapshotsQuery = useImportSnapshots()
  const deleteSnapshot = useDeleteImportSnapshot()

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

  async function handleImportLegacy(file: File) {
    // A 2nd-or-later import means this instance already has at least one automatic rollback
    // snapshot on record (see the list below) — still worth an explicit nudge toward a *full*
    // backup before importing again, since the automatic snapshot only covers what *this*
    // specific import touches, not the whole library the way `handleBackup` does.
    if ((importCountQuery.data?.import_count ?? 0) >= 1) {
      const proceed = await confirmDialog(t("settings.importLegacyRepeatWarning"))
      if (!proceed) return
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
        void snapshotsQuery.refetch()
      }
    } catch (e) {
      setImportStatus(t("settings.importLegacyFailedError", { error: String(e) }))
    } finally {
      setImportBusy(false)
      if (legacyFileInputRef.current) legacyFileInputRef.current.value = ""
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

      {/* Was a one-row, two-cell `<table>` — the cells did nothing a table specifically needs
          (no column alignment across multiple rows, no header), just side-by-side layout for two
          buttons, so a flex row reproduces the same visual result without table semantics that
          were never actually used. `justifyContent: "center"` replaces the table's own
          `margin: auto` horizontal-centering. No explicit `gap` — the spacing between the two
          buttons was never the table's own cell padding (this project's `td`s carry none), it was
          each `.stdbtn`'s own `margin: 1px` (see themes/*.css), which still applies unchanged
          since the buttons are still real `.stdbtn` elements, just no longer inside `<td>`s. */}
      <div
        id="files"
        style={{
          display: "flex",
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
        {/* `fileinput-button` (legacy's own CSS, `public/legacy/fileupload-vendor.css`) makes the
            `<input type="file">` itself the actual clickable surface — a huge (font-size: 200px),
            fully transparent input absolutely positioned over the button, not a hidden input
            opened via a JS `.click()` call. That distinction matters: a browser only treats a
            file-picker `.click()` as a trusted user gesture when it's the element the user's real
            pointer event landed on — a `<button onClick>` handler calling `ref.current.click()`
            on a *different*, hidden input is one JS-event hop removed from the actual click, which
            some browsers silently refuse to honor (confirmed live, 2026-08-29: `ref.click()` fired
            correctly in every programmatic check, yet no file picker ever opened for a real user
            click). Wrapping `Button` in this span keeps the common-ui `Button` visuals while
            restoring that same trusted-click guarantee. */}
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
        {/* Same trusted-click reasoning as the restore button's own `fileinput-button` span
            above — file selection *is* the submit action here, there's no separate "confirm"
            step once a file is chosen. */}
        <span className="fileinput-button" style={{ display: "block" }}>
          <Button style={{ height: 40, width: "100%" }} disabled={importBusy}>
            {t("settings.importLegacyButton")}
          </Button>
          <input
            ref={legacyFileInputRef}
            type="file"
            name="file"
            accept=".json,application/json"
            disabled={importBusy}
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
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "auto auto",
            columnGap: 8,
            rowGap: 2,
            margin: "16px auto",
            width: "fit-content",
            fontSize: FONT_SIZE_SM,
            textAlign: "left",
          }}
        >
          <span style={{ textAlign: "right" }}>{t("settings.importLegacyArchivesUpdated")}</span>
          <span>{importResult.archives_updated}</span>
          <span style={{ textAlign: "right" }}>
            {t("settings.importLegacyArchivesSkippedExisting")}
          </span>
          <span>{importResult.archives_skipped_already_exists}</span>
          <span style={{ textAlign: "right" }}>
            {t("settings.importLegacyArchivesSkippedNoMatch")}
          </span>
          <span>{importResult.archives_skipped_no_match}</span>
          <span style={{ textAlign: "right" }}>{t("settings.importLegacyArchivesAmbiguous")}</span>
          <span>{importResult.archives_ambiguous_match}</span>
          <span style={{ textAlign: "right" }}>
            {t("settings.importLegacyTitlesMojibakeRepaired")}
          </span>
          <span>{importResult.titles_mojibake_repaired}</span>
          <span style={{ textAlign: "right" }}>{t("settings.importLegacyCategoriesRestored")}</span>
          <span>{importResult.categories_restored}</span>
          <span style={{ textAlign: "right" }}>{t("settings.importLegacyTankoubonsRestored")}</span>
          <span>{importResult.tankoubons_restored}</span>
          <span style={{ textAlign: "right" }}>{t("settings.importLegacyStampsRestored")}</span>
          <span>{importResult.stamps_restored}</span>
        </div>
      )}

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
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "auto auto auto auto",
            columnGap: 20,
            rowGap: 4,
            margin: "0 auto",
            width: "fit-content",
            fontSize: FONT_SIZE_SM,
            textAlign: "left",
          }}
        >
          <span style={{ fontWeight: "bold" }}>{t("settings.importSnapshotsCreatedAt")}</span>
          <span style={{ fontWeight: "bold" }}>{t("settings.importSnapshotsContents")}</span>
          <span />
          <span />
          {snapshotsQuery.data.map((snapshot) => (
            <Fragment key={snapshot.id}>
              <span>{new Date(snapshot.created_at * 1000).toLocaleString()}</span>
              <span>
                {t("settings.importSnapshotsContentsSummary", {
                  archives: snapshot.archive_count,
                  categories: snapshot.category_count,
                  tankoubons: snapshot.tankoubon_count,
                  stamps: snapshot.stamp_count,
                })}
              </span>
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
            </Fragment>
          ))}
        </div>
      )}

      <div style={{ marginTop: 30 }}>
        <Button onClick={() => navigate(routes.library())}>{t("common.returnToLibrary")}</Button>
      </div>
    </div>
  )
}
