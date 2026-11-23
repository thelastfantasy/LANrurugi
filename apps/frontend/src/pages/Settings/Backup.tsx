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
} from "@/api/hooks"
import type { BackupEntry, DeletionEntry } from "@/api/types"
import { Button, IconButton } from "@/components/common-ui/Form"
import { confirmDialog } from "@/dialog"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { formatBytes } from "@/lib/formatBytes"
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

  /** A reason/source a newer server adds isn't in this build's locale files — show it verbatim. */
  const reasonLabel = (reason: string) =>
    t(`settings.snapshotReason.${reason}`, { defaultValue: reason })
  const sourceLabel = (source: string) =>
    t(`settings.deletionSource.${source}`, { defaultValue: source })

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

      <Button
        disabled={createBackup.isPending}
        style={{ minHeight: 34, padding: "4px 16px" }}
        onClick={() => void handleCreateSnapshot()}
      >
        {t("settings.autoSnapshotsCreateNow")}
      </Button>

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
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "auto auto auto auto auto auto",
            columnGap: 20,
            rowGap: 4,
            margin: "16px auto 0",
            width: "fit-content",
            fontSize: FONT_SIZE_SM,
            textAlign: "left",
            alignItems: "center",
          }}
        >
          <span style={{ fontWeight: "bold" }}>{t("settings.autoSnapshotsName")}</span>
          <span style={{ fontWeight: "bold" }}>{t("settings.autoSnapshotsReason")}</span>
          <span style={{ fontWeight: "bold" }}>{t("settings.autoSnapshotsSize")}</span>
          <span style={{ fontWeight: "bold" }}>{t("settings.autoSnapshotsCreatedAt")}</span>
          <span />
          <span />
          {backupsQuery.data.entries.map((entry) => (
            <Fragment key={entry.name}>
              <span>{entry.name}</span>
              <span>{reasonLabel(entry.reason)}</span>
              <span>{formatBytes(entry.size_bytes)}</span>
              <span>{new Date(entry.created_at).toLocaleString()}</span>
              {/* A real link, so the browser downloads the file itself (and can save/open it in a
                  new tab) instead of the page buffering it through fetch. */}
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
              <span style={{ whiteSpace: "nowrap" }}>
                <Button
                  disabled={!entry.usable || restoreBackup.isPending}
                  title={entry.usable ? undefined : t("settings.autoSnapshotsUnusable")}
                  style={{ minHeight: 24, padding: "2px 10px", fontSize: FONT_SIZE_SM }}
                  onClick={() => void handleRestoreSnapshot(entry)}
                >
                  {t("settings.autoSnapshotsRestore")}
                </Button>
              </span>
            </Fragment>
          ))}
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
        <div
          style={{
            display: "grid",
            gridTemplateColumns: "auto auto auto auto",
            columnGap: 20,
            rowGap: 4,
            margin: "16px auto 0",
            width: "fit-content",
            fontSize: FONT_SIZE_SM,
            textAlign: "left",
            alignItems: "center",
          }}
        >
          <span style={{ fontWeight: "bold" }}>{t("settings.recentDeletionsTitleColumn")}</span>
          <span style={{ fontWeight: "bold" }}>{t("settings.recentDeletionsDeletedAt")}</span>
          <span style={{ fontWeight: "bold" }}>{t("settings.recentDeletionsSource")}</span>
          <span />
          {deletionsQuery.data.entries.map((entry) => (
            <Fragment key={entry.id}>
              <span
                title={entry.title}
                style={{
                  maxWidth: 320,
                  overflow: "hidden",
                  textOverflow: "ellipsis",
                  whiteSpace: "nowrap",
                }}
              >
                {entry.title}
              </span>
              <span>{new Date(entry.deleted_at).toLocaleString()}</span>
              <span>{sourceLabel(entry.source)}</span>
              <span style={{ display: "flex", alignItems: "center", gap: 8 }}>
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
            </Fragment>
          ))}
        </div>
      )}

      <h3 style={{ marginTop: 16 }}>{deletionStatus}</h3>

      <div style={{ marginTop: 30 }}>
        <Button onClick={() => navigate(routes.library())}>{t("common.returnToLibrary")}</Button>
      </div>
    </div>
  )
}
