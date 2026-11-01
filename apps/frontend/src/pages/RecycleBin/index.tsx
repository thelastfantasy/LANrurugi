import type { CSSProperties } from "react"
import { useMemo, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import {
  useEmptyBin,
  usePurgeEntry,
  useRecycleBin,
  useRestoreFromBin,
  useSafetyPasswordStatus,
} from "@/api/hooks"
import type { RecycleEntry } from "@/api/types"
import { Button, IconButton, Input } from "@/components/common-ui/Form"
import { useIsNarrowViewport } from "@/hooks"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { formatBytes } from "@/lib/formatBytes"
import { routes } from "@/lib/routes"
import { FONT_SIZE_SM } from "@/theme"
import { toast } from "@/toast"

import { RecycleRow } from "./RecycleRow"
import { SafetyPasswordDialog } from "./SafetyPasswordDialog"

type PendingAction = { kind: "purge"; entry: RecycleEntry } | { kind: "empty" } | null

/** Grid shared by the header and every row, so columns line up (same approach the Activity page
 * uses for its table). */
export const RECYCLE_GRID = "52px minmax(0,1fr) 92px 150px 118px 84px"

/** Cells of the loading skeleton, shaped like `RecycleRow`'s own cells. */
const SKELETON_CELL: CSSProperties = {
  display: "flex",
  alignItems: "center",
  padding: "8px 0",
  borderBottom: "1px solid rgba(128,128,128,0.15)",
  minWidth: 0,
}

/** The recycle bin (spec 019). Recoverable until the safety password says otherwise. */
export function RecycleBinPage() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const narrow = useIsNarrowViewport()
  useDocumentTitle(t("recycleBin.title") ?? undefined)
  const bin = useRecycleBin()
  const safety = useSafetyPasswordStatus()
  const restore = useRestoreFromBin()
  const purge = usePurgeEntry()
  const empty = useEmptyBin()

  const [query, setQuery] = useState("")
  const [pending, setPending] = useState<PendingAction>(null)
  const [expanded, setExpanded] = useState<Set<string>>(new Set())

  function toggleExpanded(id: string) {
    setExpanded((prev) => {
      const next = new Set(prev)
      if (next.has(id)) next.delete(id)
      else next.add(id)
      return next
    })
  }

  const entries = useMemo(() => bin.data?.entries ?? [], [bin.data])
  const filtered = useMemo(() => {
    const needle = query.trim().toLowerCase()
    if (!needle) return entries
    return entries.filter((entry) =>
      [entry.title, entry.name, entry.file_name, entry.original_path]
        .join(" ")
        .toLowerCase()
        .includes(needle),
    )
  }, [entries, query])

  async function handleRestore(entry: RecycleEntry) {
    try {
      const result = await restore.mutateAsync(entry.id)
      toast({
        heading:
          result.kind === "tankoubon"
            ? t("recycleBin.tankRestored", { count: result.members ?? entry.member_count })
            : t("recycleBin.restored"),
        text: result.renamed && result.path ? t("recycleBin.restoredTo", { path: result.path }) : undefined,
        icon: "success",
      })
    } catch {
      toast({ heading: t("recycleBin.restoreFailed"), icon: "error" })
    }
  }

  const pendingTitle =
    pending?.kind === "empty"
      ? t("recycleBin.emptyConfirmTitle")
      : pending?.kind === "purge"
        ? t("recycleBin.purgeConfirmTitle")
        : ""
  const pendingDescription =
    pending?.kind === "empty"
      ? t("recycleBin.emptyConfirmBody")
      : pending?.kind === "purge"
        ? pending.entry.title
        : undefined

  async function handleConfirm(pin: string) {
    if (!pending) return
    if (pending.kind === "purge") {
      await purge.mutateAsync({ id: pending.entry.id, safetyPassword: pin })
      toast({ heading: t("recycleBin.purged"), icon: "success" })
    } else {
      const result = await empty.mutateAsync(pin)
      toast({ heading: t("recycleBin.emptied", { count: result.deleted }), icon: "success" })
    }
    setPending(null)
  }

  const limits = bin.data?.limits
  const usage = bin.data?.usage
  const retention =
    limits && limits.retention_days > 0
      ? t("recycleBin.keptDays", { count: limits.retention_days })
      : t("recycleBin.keptForever")
  const cap = limits && limits.max_bytes > 0 ? formatBytes(limits.max_bytes) : t("recycleBin.unlimited")
  const summary = usage
    ? ` · ${t("recycleBin.summary", {
        count: usage.count,
        bytes: formatBytes(usage.bytes),
        cap,
        retention,
      })}`
    : ""

  return (
    <div className="ido" style={{ paddingLeft: 12, paddingRight: 12, boxSizing: "border-box" }}>
      <h1 className="ih">{t("recycleBin.title")}</h1>
      <p style={{ fontSize: FONT_SIZE_SM }}>
        {t("recycleBin.help")}
        {summary}
      </p>

      {/* Centred like the Activity page's filter bar; every control is 30px tall so the icon
          button and the labelled button share one baseline. */}
      <div
        style={{
          display: "flex",
          justifyContent: narrow ? "flex-start" : "center",
          alignItems: "center",
          gap: 8,
          flexWrap: "wrap",
        }}
      >
        <div
          style={{
            position: "relative",
            // Desktop: the Activity-filter width. Phone: take the first row's free space so the
            // refresh button and the empty-bin action line up at the edges.
            flex: narrow ? "1 1 200px" : "0 0 auto",
            width: narrow ? "auto" : 320,
            maxWidth: "100%",
            height: 30,
          }}
        >
          <i
            className="fa fa-search"
            aria-hidden="true"
            style={{
              position: "absolute",
              left: 9,
              top: "50%",
              transform: "translateY(-50%)",
              opacity: 0.55,
              pointerEvents: "none",
              fontSize: "0.85em",
            }}
          ></i>
          <Input
            value={query}
            onValueChange={setQuery}
            placeholder={t("recycleBin.filterPlaceholder")}
            aria-label={t("recycleBin.filterPlaceholder")}
            style={{
              width: "100%",
              height: 30,
              margin: 0,
              padding: "2px 6px 2px 26px",
              boxSizing: "border-box",
            }}
          />
        </div>
        <IconButton
          icon="fa fa-sync"
          size={30}
          title={t("recycleBin.refresh")}
          aria-label={t("recycleBin.refresh")}
          onClick={() => void bin.refetch()}
          disabled={bin.isFetching}
          style={{ width: 30, height: 30, minWidth: 30, margin: 0, boxSizing: "border-box" }}
        />
        <div style={{ marginLeft: narrow ? "auto" : 0 }}>
          <Button
            onClick={() => setPending({ kind: "empty" })}
            disabled={entries.length === 0}
          style={{
            height: 30,
            minWidth: 0,
            width: "auto",
            margin: 0,
            padding: "0 10px 1px",
            boxSizing: "border-box",
            display: "inline-flex",
            alignItems: "center",
            }}
          >
            {t("recycleBin.emptyBin")}
          </Button>
        </div>
      </div>

      {bin.isLoading && narrow && (
        <div role="status" aria-label={t("common.loading")} style={{ marginTop: 16 }}>
          {Array.from({ length: 4 }, (_, row) => (
            <div
              key={row}
              aria-hidden="true"
              style={{
                display: "flex",
                gap: 10,
                padding: "10px 0",
                borderBottom: "1px solid rgba(128,128,128,0.15)",
                alignItems: "flex-start",
              }}
            >
              <span className="skeleton-bar" style={{ width: 48, height: 66 }} />
              <div style={{ flex: 1, minWidth: 0, display: "flex", flexDirection: "column", gap: 8 }}>
                <span
                  className="skeleton-bar"
                  style={{ width: `${60 + ((row * 7) % 25)}%`, animationDelay: `${row * 0.15}s` }}
                />
                <span
                  className="skeleton-bar"
                  style={{ width: "80%", height: 8, animationDelay: `${row * 0.15}s` }}
                />
              </div>
              <span className="skeleton-bar" style={{ width: 30, height: 30 }} />
            </div>
          ))}
        </div>
      )}

      {bin.isLoading && !narrow && (
        <div style={{ overflowX: "auto", marginTop: 16 }}>
          <div
            role="status"
            aria-label={t("common.loading")}
            style={{
              display: "grid",
              gridTemplateColumns: RECYCLE_GRID,
              columnGap: 12,
              fontSize: FONT_SIZE_SM,
              minWidth: 720,
              textAlign: "left",
            }}
          >
            {/* Same header as the real table, so nothing shifts when the rows arrive. */}
            <div />
            <div style={{ opacity: 0.45, padding: "4px 0" }}>{t("recycleBin.columnArchive")}</div>
            <div style={{ opacity: 0.45, padding: "4px 0" }}>{t("recycleBin.columnSize")}</div>
            <div style={{ opacity: 0.45, padding: "4px 0" }}>{t("recycleBin.columnDeleted")}</div>
            <div style={{ opacity: 0.45, padding: "4px 0" }}>{t("recycleBin.columnRemaining")}</div>
            <div style={{ opacity: 0.45, padding: "4px 0", textAlign: "right" }}>
              {t("recycleBin.columnActions")}
            </div>
            {Array.from({ length: 4 }, (_, row) => (
              <div key={row} style={{ display: "contents" }} aria-hidden="true">
                <div style={{ ...SKELETON_CELL }}>
                  <span className="skeleton-bar" style={{ width: 44, height: 60 }} />
                </div>
                <div style={{ ...SKELETON_CELL, flexDirection: "column", alignItems: "stretch", gap: 6 }}>
                  <span
                    className="skeleton-bar"
                    style={{ width: `${55 + ((row * 11) % 30)}%`, animationDelay: `${row * 0.15}s` }}
                  />
                  <span className="skeleton-bar" style={{ width: "40%", height: 8, animationDelay: `${row * 0.15}s` }} />
                </div>
                <div style={SKELETON_CELL}>
                  <span className="skeleton-bar" style={{ width: "60%" }} />
                </div>
                <div style={SKELETON_CELL}>
                  <span className="skeleton-bar" style={{ width: "80%" }} />
                </div>
                <div style={SKELETON_CELL}>
                  <span className="skeleton-bar" style={{ width: "60%" }} />
                </div>
                <div style={{ ...SKELETON_CELL, justifyContent: "flex-end" }}>
                  <span className="skeleton-bar" style={{ width: 26, height: 26 }} />
                </div>
              </div>
            ))}
          </div>
        </div>
      )}

      {bin.isError && (
        <div style={{ textAlign: "center", margin: "24px 0" }}>
          <i className="fa fa-3x fa-exclamation-triangle"></i>
          <p>{t("recycleBin.loadFailed")}</p>
          <Button onClick={() => void bin.refetch()}>{t("recycleBin.retry")}</Button>
        </div>
      )}

      {!bin.isLoading && !bin.isError && filtered.length === 0 && (
        <div style={{ textAlign: "center", margin: "24px 0" }}>
          <i className="fa fa-3x fa-recycle"></i>
          <p>{entries.length === 0 ? t("recycleBin.empty") : t("recycleBin.noMatches")}</p>
        </div>
      )}

      {!bin.isLoading && !bin.isError && filtered.length > 0 && narrow && (
        <div style={{ marginTop: 16 }}>
          {filtered.map((entry) => (
            <RecycleRow
              key={entry.id}
              entry={entry}
              narrow
              expanded={expanded.has(entry.id)}
              onToggleExpand={() => toggleExpanded(entry.id)}
              onRestore={(target) => void handleRestore(target)}
              onPurge={(target) => setPending({ kind: "purge", entry: target })}
              restoring={restore.isPending && restore.variables === entry.id}
            />
          ))}
        </div>
      )}

      {!bin.isLoading && !bin.isError && filtered.length > 0 && !narrow && (
        <div style={{ overflowX: "auto", marginTop: 16 }}>
          <div
            style={{
              display: "grid",
              gridTemplateColumns: RECYCLE_GRID,
              columnGap: 12,
              fontSize: FONT_SIZE_SM,
              minWidth: 720,
              // `.ido` centres its text; the table's cells are laid out from the left, so without
              // this every header sits in the middle of its column while the values start at the edge.
              textAlign: "left",
            }}
          >
            <div></div>
            <div style={{ opacity: 0.65, padding: "4px 0" }}>{t("recycleBin.columnArchive")}</div>
            <div style={{ opacity: 0.65, padding: "4px 0" }}>{t("recycleBin.columnSize")}</div>
            <div style={{ opacity: 0.65, padding: "4px 0" }}>{t("recycleBin.columnDeleted")}</div>
            <div style={{ opacity: 0.65, padding: "4px 0" }}>{t("recycleBin.columnRemaining")}</div>
            <div style={{ opacity: 0.65, padding: "4px 0", textAlign: "right" }}>
              {t("recycleBin.columnActions")}
            </div>
            {filtered.map((entry) => (
              <RecycleRow
                key={entry.id}
                entry={entry}
                expanded={expanded.has(entry.id)}
                onToggleExpand={() => toggleExpanded(entry.id)}
                onRestore={(target) => void handleRestore(target)}
                onPurge={(target) => setPending({ kind: "purge", entry: target })}
                restoring={restore.isPending && restore.variables === entry.id}
              />
            ))}
          </div>
        </div>
      )}

      {bin.data && bin.data.leftovers.count > 0 && (
        <p style={{ fontSize: FONT_SIZE_SM }}>{t("recycleBin.leftovers", { count: bin.data.leftovers.count })}</p>
      )}

      <div style={{ marginTop: 26, textAlign: "center" }}>
        <Button onClick={() => navigate(routes.library())}>{t("common.returnToLibrary")}</Button>
      </div>

      {pending && (
        <SafetyPasswordDialog
          title={pendingTitle}
          description={pendingDescription}
          confirmLabel={
            pending.kind === "empty"
              ? t("recycleBin.emptyBin")
              : t("recycleBin.deletePermanently")
          }
          configured={bin.data?.safety_password.configured ?? safety.data?.configured ?? false}
          status={safety.data}
          onClose={() => setPending(null)}
          onConfirm={handleConfirm}
        />
      )}
    </div>
  )
}
