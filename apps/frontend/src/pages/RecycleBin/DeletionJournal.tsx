import type { CSSProperties } from "react"
import { useMemo } from "react"
import { useTranslation } from "react-i18next"

import { useDeletions, useRestoreAllDeletions, useRestoreDeletion } from "@/api/hooks"
import type { DeletionEntry } from "@/api/types"
import { Button, IconButton } from "@/components/common-ui/Form"
import { confirmDialog } from "@/dialog"
import { absoluteTimestamp, formatRelativeTime } from "@/lib/relativeTime"
import { FONT_SIZE_SM } from "@/theme"
import { toast } from "@/toast"

import { RecycleRowRegion, RecycleTitle } from "./RecycleTitle"

const JOURNAL_GRID = "minmax(0,1fr) 120px 150px 84px"

const CELL: CSSProperties = {
  display: "flex",
  alignItems: "center",
  padding: "8px 0",
  borderBottom: "1px solid rgba(128,128,128,0.25)",
  minWidth: 0,
}

const HEADER: CSSProperties = { opacity: 0.65, padding: "4px 0" }

const SOURCES = new Set(["clean", "single", "superseded", "split"])

/** The journal entries the "other deletions" tab lists: entries the bin already holds, and records
 * that are back already, are left out, since restoring them here would duplicate the bin or be
 * refused. */
export function useJournalEntries(binIds: ReadonlySet<string>) {
  const journal = useDeletions()
  const entries = useMemo(
    () =>
      (journal.data?.entries ?? []).filter(
        (entry) => !entry.record_exists && !binIds.has(entry.id),
      ),
    [journal.data, binIds],
  )
  return { journal, entries }
}

/** The tab's toolbar action, sitting where the bin tab has its empty-bin button. */
export function JournalRestoreAllButton({ disabled, style }: { disabled: boolean; style: CSSProperties }) {
  const { t } = useTranslation()
  const restoreAll = useRestoreAllDeletions()

  async function handleRestoreAll() {
    if (!(await confirmDialog(t("recycleBin.journalRestoreAllConfirm"), true))) return
    try {
      const result = await restoreAll.mutateAsync()
      toast({
        heading: t("recycleBin.journalRestoredAll", {
          restored: result.restored,
          skipped: result.skipped,
          missing: result.missing_files,
          failed: result.failed,
        }),
        icon: result.failed > 0 ? "warning" : "success",
      })
    } catch {
      toast({ heading: t("recycleBin.restoreFailed"), icon: "error" })
    }
  }

  return (
    <Button onClick={() => void handleRestoreAll()} disabled={disabled || restoreAll.isPending} style={style}>
      {t("recycleBin.journalRestoreAll")}
    </Button>
  )
}

/** Records removed without passing through the bin (Database clean, a revision replacing them, a
 * split), from the specs/028 deletion journal. */
export function DeletionJournal({
  journal,
  entries,
  query,
  narrow,
}: {
  journal: ReturnType<typeof useDeletions>
  entries: DeletionEntry[]
  query: string
  narrow: boolean
}) {
  const { t, i18n } = useTranslation()
  const restore = useRestoreDeletion()

  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase()
    if (!needle) return entries
    return entries.filter((entry) =>
      [entry.title, entry.path].join(" ").toLowerCase().includes(needle),
    )
  }, [entries, query])

  function sourceLabel(source: string) {
    return t(`recycleBin.journalSource.${SOURCES.has(source) ? source : "other"}`)
  }

  async function handleRestore(entry: DeletionEntry) {
    try {
      const result = await restore.mutateAsync(entry.id)
      toast({
        heading: result.file_missing ? t("recycleBin.journalRestoredFileMissing") : t("recycleBin.restored"),
        text: entry.title,
        icon: result.file_missing ? "warning" : "success",
      })
    } catch {
      toast({ heading: t("recycleBin.restoreFailed"), text: entry.title, icon: "error" })
    }
  }

  return (
    <>
      {journal.isError && (
        <div style={{ textAlign: "center", margin: "24px 0" }}>
          <i className="fa fa-3x fa-exclamation-triangle"></i>
          <p>{t("recycleBin.journalLoadFailed")}</p>
          <Button onClick={() => void journal.refetch()}>{t("recycleBin.retry")}</Button>
        </div>
      )}

      {journal.isSuccess && filtered.length === 0 && (
        <div style={{ textAlign: "center", margin: "24px 0" }}>
          <i className="fa fa-3x fa-history"></i>
          <p>{entries.length === 0 ? t("recycleBin.journalEmpty") : t("recycleBin.noMatches")}</p>
        </div>
      )}

      {filtered.length > 0 && narrow && (
        <div style={{ marginTop: 16 }}>
          {filtered.map((entry) => {
            const iso = entry.deleted_at
            return (
              <RecycleRowRegion
                key={entry.id}
                style={{
                  display: "flex",
                  gap: 10,
                  alignItems: "flex-start",
                  padding: "10px 0",
                  borderBottom: "1px solid rgba(128,128,128,0.25)",
                  textAlign: "left",
                }}
              >
                <div style={{ flex: 1, minWidth: 0 }}>
                  <RecycleTitle title={entry.title || entry.id} path={entry.path} />
                  {!entry.file_exists && (
                    <div className="recycle-conflict">
                      <i className="fa fa-exclamation-circle" aria-hidden="true"></i>{" "}
                      {t("recycleBin.journalFileMissing")}
                    </div>
                  )}
                  <div style={{ display: "flex", flexWrap: "wrap", gap: "2px 10px", fontSize: "8.5pt", opacity: 0.8, marginTop: 4 }}>
                    <span>{sourceLabel(entry.source)}</span>
                    <span title={absoluteTimestamp(iso, i18n.language)}>
                      {formatRelativeTime(iso, t, i18n.language)}
                    </span>
                  </div>
                </div>
                <IconButton
                  icon={restore.isPending && restore.variables === entry.id ? "fa fa-spinner fa-spin" : "fa fa-undo"}
                  size={30}
                  className="stdbtn"
                  title={t("recycleBin.restore")}
                  onClick={() => void handleRestore(entry)}
                  disabled={restore.isPending}
                />
              </RecycleRowRegion>
            )
          })}
        </div>
      )}

      {filtered.length > 0 && !narrow && (
        <div style={{ overflowX: "auto", marginTop: 16 }}>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: JOURNAL_GRID,
              columnGap: 12,
              fontSize: FONT_SIZE_SM,
              minWidth: 620,
              textAlign: "left",
            }}
          >
            <div style={HEADER}>{t("recycleBin.columnArchive")}</div>
            <div style={HEADER}>{t("recycleBin.journalColumnSource")}</div>
            <div style={HEADER}>{t("recycleBin.columnDeleted")}</div>
            <div style={{ ...HEADER, textAlign: "right" }}>{t("recycleBin.columnActions")}</div>
            {filtered.map((entry) => {
              const iso = entry.deleted_at
              return (
                <RecycleRowRegion key={entry.id} style={{ display: "contents" }}>
                  <div style={{ ...CELL, flexDirection: "column", alignItems: "flex-start", gap: 2 }}>
                    <RecycleTitle title={entry.title || entry.id} path={entry.path} />
                    {!entry.file_exists && (
                      <div className="recycle-conflict">
                        <i className="fa fa-exclamation-circle" aria-hidden="true"></i>{" "}
                        {t("recycleBin.journalFileMissing")}
                      </div>
                    )}
                  </div>
                  <div style={{ ...CELL, whiteSpace: "nowrap" }}>{sourceLabel(entry.source)}</div>
                  <div style={{ ...CELL, whiteSpace: "nowrap" }} title={absoluteTimestamp(iso, i18n.language)}>
                    {formatRelativeTime(iso, t, i18n.language)}
                  </div>
                  <div style={{ ...CELL, justifyContent: "flex-end" }}>
                    <IconButton
                      icon={
                        restore.isPending && restore.variables === entry.id ? "fa fa-spinner fa-spin" : "fa fa-undo"
                      }
                      size={26}
                      className="stdbtn"
                      title={t("recycleBin.restore")}
                      onClick={() => void handleRestore(entry)}
                      disabled={restore.isPending}
                    />
                  </div>
                </RecycleRowRegion>
              )
            })}
          </div>
        </div>
      )}
    </>
  )
}
