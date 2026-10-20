import { Fragment, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { waitForJob } from "@/api/client"
import { useDeleteImportSnapshot, useImportSnapshots } from "@/api/hooks"
import { Button, IconButton } from "@/components/common-ui/Form"
import { confirmDialog } from "@/dialog"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM, useApplyTheme } from "@/theme"

import { LegacyImportPanel } from "./LegacyImportPanel"

/** Mirrors legacy's backup/restore layout (backup/restore actions, spinner, return button) using
 * common-ui `Button`s instead of legacy's table markup; status is a plain text line, no progress bar. */
export function Backup() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const fileInputRef = useRef<HTMLInputElement>(null)
  const [status, setStatus] = useState<string>("")
  const [busy, setBusy] = useState(false)

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
