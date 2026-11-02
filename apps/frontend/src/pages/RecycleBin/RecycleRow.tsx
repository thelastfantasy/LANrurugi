import type { CSSProperties } from "react"
import { useTranslation } from "react-i18next"

import type { RecycleEntry } from "@/api/types"
import { IconButton } from "@/components/common-ui/Form"
import { formatBytes } from "@/lib/formatBytes"

const CELL: CSSProperties = {
  display: "flex",
  alignItems: "center",
  padding: "8px 0",
  borderBottom: "1px solid rgba(128,128,128,0.25)",
  minWidth: 0,
}

/** One row of the recycle grid. Renders the cells directly (no wrapper) so it sits inside the
 * page's shared `gridTemplateColumns` — the same layout the Activity table uses. */
export function RecycleRow({
  entry,
  onRestore,
  onPurge,
  restoring,
  narrow = false,
  expanded = false,
  onToggleExpand,
}: {
  entry: RecycleEntry
  onRestore: (entry: RecycleEntry) => void
  onPurge: (entry: RecycleEntry) => void
  restoring: boolean
  /** Phone layout: one stacked card per entry instead of the six-column table row. */
  narrow?: boolean
  expanded?: boolean
  onToggleExpand?: () => void
}) {
  const { t } = useTranslation()
  const deletedAt = new Date(entry.deleted_at * 1000).toLocaleString()
  const isTank = entry.kind === "tankoubon"

  if (isTank && narrow) {
    return (
      <div
        style={{
          padding: "10px 0",
          borderBottom: "1px solid rgba(128,128,128,0.25)",
          textAlign: "left",
        }}
      >
        <div style={{ display: "flex", gap: 10, alignItems: "flex-start" }}>
          <div className="recycle-thumb" style={{ flex: "0 0 auto" }}>
            {entry.has_thumbnail ? (
              <img
                src={`/api/recycle-bin/${encodeURIComponent(entry.id)}/thumbnail`}
                alt=""
                loading="lazy"
              />
            ) : (
              <i className="fa fa-layer-group" aria-hidden="true"></i>
            )}
          </div>
          <div style={{ flex: 1, minWidth: 0 }}>
            <div style={{ fontWeight: "bold" }}>{entry.title}</div>
            <div style={{ display: "flex", flexWrap: "wrap", gap: "2px 10px", fontSize: "8.5pt", opacity: 0.8, marginTop: 4 }}>
              <span>{t("recycleBin.tankoubon")}</span>
              <span>{t("recycleBin.tankVolumeCount", { count: entry.member_count })}</span>
              <span>{formatBytes(entry.size)}</span>
              <span>{deletedAt}</span>
              <span>{remainingLabel(entry, t)}</span>
            </div>
            {!entry.restorable && entry.conflict && (
              <div className="recycle-conflict">{conflictText(entry.conflict, t)}</div>
            )}
            <button
              type="button"
              className="stdbtn"
              style={{ height: 24, minWidth: 0, margin: "6px 0 0", padding: "0 10px" }}
              onClick={onToggleExpand}
            >
              <i className={"fa " + (expanded ? "fa-chevron-up" : "fa-chevron-down")} aria-hidden="true"></i>{" "}
              {expanded ? t("recycleBin.hideMembers") : t("recycleBin.showMembers")}
            </button>
            {expanded && <TankMemberList members={entry.members} />}
          </div>
          <div style={{ display: "flex", flexDirection: "column", gap: 6, flex: "0 0 auto" }}>
            <IconButton
              icon={restoring ? "fa fa-spinner fa-spin" : "fa fa-undo"}
              size={30}
              className="stdbtn"
              title={t("recycleBin.restore")}
              onClick={() => onRestore(entry)}
              disabled={!entry.restorable || restoring}
            />
            <IconButton
              icon="fa fa-trash"
              size={30}
              className="stdbtn stdbtn-danger"
              title={t("recycleBin.deletePermanently")}
              onClick={() => onPurge(entry)}
            />
          </div>
        </div>
      </div>
    )
  }

  if (isTank && !narrow) {
    return (
      <>
        <div style={CELL}>
          <div className="recycle-thumb">
            {entry.has_thumbnail ? (
              <img
                src={`/api/recycle-bin/${encodeURIComponent(entry.id)}/thumbnail`}
                alt=""
                loading="lazy"
              />
            ) : (
              <i className="fa fa-layer-group" aria-hidden="true"></i>
            )}
          </div>
        </div>
        <div style={{ ...CELL, flexDirection: "column", alignItems: "flex-start", gap: 2 }}>
          <div style={{ fontWeight: "bold", maxWidth: "100%", overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {entry.title}
          </div>
          <div style={{ fontSize: "8pt", opacity: 0.8 }}>
            {t("recycleBin.tankoubon")} · {t("recycleBin.tankVolumeCount", { count: entry.member_count })}
          </div>
          {!entry.restorable && entry.conflict && (
            <div className="recycle-conflict">{conflictText(entry.conflict, t)}</div>
          )}
        </div>
        <div style={{ ...CELL, whiteSpace: "nowrap" }}>{formatBytes(entry.size)}</div>
        <div style={{ ...CELL, whiteSpace: "nowrap" }} title={deletedAt}>{deletedAt}</div>
        <div style={{ ...CELL, whiteSpace: "nowrap" }}>{remainingLabel(entry, t)}</div>
        <div style={{ ...CELL, justifyContent: "flex-end", gap: 6 }}>
          <IconButton
            icon={"fa " + (expanded ? "fa-chevron-up" : "fa-chevron-down")}
            size={26}
            className="stdbtn"
            title={expanded ? t("recycleBin.hideMembers") : t("recycleBin.showMembers")}
            onClick={onToggleExpand}
          />
          <IconButton
            icon={restoring ? "fa fa-spinner fa-spin" : "fa fa-undo"}
            size={26}
            className="stdbtn"
            title={t("recycleBin.restore")}
            onClick={() => onRestore(entry)}
            disabled={!entry.restorable || restoring}
          />
          <IconButton
            icon="fa fa-trash"
            size={26}
            className="stdbtn stdbtn-danger"
            title={t("recycleBin.deletePermanently")}
            onClick={() => onPurge(entry)}
          />
        </div>
        {expanded && (
          <div style={{ gridColumn: "1 / -1", padding: "2px 0 10px" }}>
            <TankMemberList members={entry.members} />
          </div>
        )}
      </>
    )
  }

  if (narrow) {
    return (
      <div
        style={{
          display: "flex",
          gap: 10,
          padding: "10px 0",
          borderBottom: "1px solid rgba(128,128,128,0.25)",
          alignItems: "flex-start",
          // `.ido` centres its text; the card is a left-aligned list item.
          textAlign: "left",
        }}
      >
        <div className="recycle-thumb" style={{ flex: "0 0 auto" }}>
          {entry.has_thumbnail ? (
            <img
              src={`/api/recycle-bin/${encodeURIComponent(entry.id)}/thumbnail`}
              alt=""
              loading="lazy"
              onError={(event) => {
                event.currentTarget.style.display = "none"
              }}
            />
          ) : (
            <i className="fa fa-book" aria-hidden="true"></i>
          )}
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div
            style={{
              fontWeight: "bold",
              overflow: "hidden",
              textOverflow: "ellipsis",
              whiteSpace: "nowrap",
            }}
            title={entry.title}
          >
            {entry.title || entry.name}
          </div>
          <div className="recycle-path" style={{ maxWidth: "100%" }} title={entry.original_path}>
            {entry.original_path}
          </div>
          {!entry.restorable && entry.conflict && (
            <div className="recycle-conflict">
              <i className="fa fa-exclamation-circle" aria-hidden="true"></i>{" "}
              {conflictLabel(entry, t)}
            </div>
          )}
          <div
            style={{
              display: "flex",
              flexWrap: "wrap",
              gap: "2px 10px",
              fontSize: "8.5pt",
              opacity: 0.8,
              marginTop: 4,
            }}
          >
            <span>{formatBytes(entry.size)}</span>
            <span>{t("recycleBin.pageCount", { count: entry.page_count })}</span>
            <span>{deletedAt}</span>
            <span>{remainingLabel(entry, t)}</span>
          </div>
        </div>
        <div style={{ display: "flex", flexDirection: "column", gap: 6, flex: "0 0 auto" }}>
          <IconButton
            icon={restoring ? "fa fa-spinner fa-spin" : "fa fa-undo"}
            size={30}
            className="stdbtn"
            title={entry.restorable ? t("recycleBin.restore") : conflictLabel(entry, t)}
            onClick={() => onRestore(entry)}
            disabled={!entry.restorable || restoring}
          />
          <IconButton
            icon="fa fa-trash"
            size={30}
            className="stdbtn stdbtn-danger"
            title={t("recycleBin.deletePermanently")}
            onClick={() => onPurge(entry)}
          />
        </div>
      </div>
    )
  }

  return (
    <>
      <div style={CELL}>
        <div className="recycle-thumb">
          {entry.has_thumbnail ? (
            <img
              src={`/api/recycle-bin/${encodeURIComponent(entry.id)}/thumbnail`}
              alt=""
              loading="lazy"
              onError={(event) => {
                event.currentTarget.style.display = "none"
              }}
            />
          ) : (
            <i className="fa fa-book" aria-hidden="true"></i>
          )}
        </div>
      </div>
      <div style={{ ...CELL, flexDirection: "column", alignItems: "flex-start", gap: 2 }}>
        <div
          style={{
            fontWeight: "bold",
            maxWidth: "100%",
            overflow: "hidden",
            textOverflow: "ellipsis",
            whiteSpace: "nowrap",
          }}
          title={entry.title}
        >
          {entry.title || entry.name}
        </div>
        {!entry.restorable && entry.conflict && (
          <div className="recycle-conflict">
            <i className="fa fa-exclamation-circle" aria-hidden="true"></i> {conflictLabel(entry, t)}
          </div>
        )}
        <div className="recycle-path" style={{ maxWidth: "100%" }} title={entry.original_path}>
          {entry.original_path}
        </div>
      </div>
      <div style={{ ...CELL, whiteSpace: "nowrap" }}>{formatBytes(entry.size)}</div>
      <div style={{ ...CELL, whiteSpace: "nowrap" }} title={deletedAt}>
        {deletedAt}
      </div>
      <div style={{ ...CELL, whiteSpace: "nowrap" }}>{remainingLabel(entry, t)}</div>
      <div style={{ ...CELL, justifyContent: "flex-end", gap: 6 }}>
        <IconButton
          icon={restoring ? "fa fa-spinner fa-spin" : "fa fa-undo"}
          size={26}
          className="stdbtn"
          title={entry.restorable ? t("recycleBin.restore") : conflictLabel(entry, t)}
          onClick={() => onRestore(entry)}
          disabled={!entry.restorable || restoring}
        />
        <IconButton
          icon="fa fa-trash"
          size={26}
          className="stdbtn stdbtn-danger"
          title={t("recycleBin.deletePermanently")}
          onClick={() => onPurge(entry)}
        />
      </div>
    </>
  )
}

/** Read-only member list behind a recycled Tankoubon's expand control. Actions stay on the whole
 * tank: restoring a single volume out of a dissolved grouping is not a coherent operation. */
function TankMemberList({ members }: { members: RecycleEntry["members"] }) {
  const { t } = useTranslation()
  return (
    <ul
      style={{
        listStyle: "none",
        margin: "8px 0 0",
        padding: "8px 0 0 10px",
        borderTop: "1px solid rgba(128,128,128,0.2)",
        display: "flex",
        flexDirection: "column",
        gap: 4,
        fontSize: "8.5pt",
        width: "100%",
      }}
    >
      {members.map((member) => (
        <li key={member.id} style={{ display: "flex", gap: 10, justifyContent: "space-between" }}>
          <span style={{ overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            {member.title || member.id}
          </span>
          <span style={{ opacity: 0.75, whiteSpace: "nowrap" }}>
            {formatBytes(member.size)}
            {!member.restorable && member.conflict
              ? ` · ${conflictText(member.conflict, t)}`
              : ""}
          </span>
        </li>
      ))}
    </ul>
  )
}

function conflictText(
  conflict: NonNullable<RecycleEntry["conflict"]>,
  t: (key: string) => string,
): string {
  switch (conflict) {
    case "identical_archive_exists":
      return t("recycleBin.conflictIdentical")
    case "payload_missing":
      return t("recycleBin.conflictMissing")
    case "payload_size_mismatch":
      return t("recycleBin.conflictChanged")
    case "unsupported_schema":
      return t("recycleBin.conflictUnsupported")
  }
}

function remainingLabel(
  entry: RecycleEntry,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  if (entry.remaining_seconds === null) return t("recycleBin.keptForever")
  const seconds = entry.remaining_seconds
  if (seconds <= 0) return t("recycleBin.expiring")
  const days = Math.floor(seconds / 86_400)
  if (days >= 1) return t("recycleBin.daysLeft", { count: days })
  const hours = Math.max(1, Math.floor(seconds / 3600))
  return t("recycleBin.hoursLeft", { count: hours })
}

function conflictLabel(entry: RecycleEntry, t: (key: string) => string): string {
  return entry.conflict ? conflictText(entry.conflict, t) : ""
}
