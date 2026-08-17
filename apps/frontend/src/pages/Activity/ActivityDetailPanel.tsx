import { useTranslation } from "react-i18next"

import type { ActivityEntry } from "@/api/types"
import { CodeBlock, IpGeoLink, Modal } from "@/components/Display"
import { FONT_SIZE_SM } from "@/theme"

import { actionTypeLabel } from "./activityTarget"
import { MetadataDiff, type MetadataUpdateAfter, type MetadataUpdateBefore } from "./MetadataDiff"
import { OperationDescription } from "./OperationDescription"

/** `archive.metadata_update` gets the real GitHub-commit-style word/tag diff (`MetadataDiff`)
 * instead of the generic raw-JSON `before`/`after` code blocks every other action type falls back
 * to — it's the one action type whose `before`/`after` shape is rich enough (title/summary text,
 * a precomputed tag added/removed set — see `archives.rs::update_archive_metadata`'s own docs) to
 * be worth a dedicated renderer; a raw JSON dump of `{"title": "...", "tags_added": [...]}` reads
 * far worse than the colored diff for what's fundamentally the same information. */
const METADATA_DIFF_ACTION_TYPES = new Set(["archive.metadata_update"])

/** Full detail view for one `ActivityEntry` — before/after diff (when present), the causal chain
 * for an automatic entry (`caused_by`), and every field the row itself only has room to summarize.
 * `onDelete` is optional — omitted when the panel is opened by a caller with no delete affordance
 * of its own (there is none today, but keeping this optional rather than required avoids forcing
 * every future caller to wire up a delete path just to show detail). */
export function ActivityDetailPanel({
  entry,
  onClose,
  onDelete,
}: {
  entry: ActivityEntry
  onClose: () => void
  onDelete?: () => void
}) {
  const { t } = useTranslation()
  const showMetadataDiff = METADATA_DIFF_ACTION_TYPES.has(entry.action_type) && entry.after != null

  return (
    <Modal onClose={onClose} width={640} textAlign="left">
      <div style={{ display: "flex", alignItems: "flex-start", justifyContent: "space-between", gap: 12 }}>
        <h2 style={{ margin: 0 }}>{actionTypeLabel(t, entry.action_type)}</h2>
        {onDelete && (
          <button type="button" className="stdbtn stdbtn-danger" style={{ minWidth: 0, width: "auto", padding: "0 12px" }} onClick={onDelete}>
            {t("common.delete")}
          </button>
        )}
      </div>
      {/* Plain single-line `toLocaleString()`, not the row grid's own `DateTimeStack` — that
          component splits date/time onto two lines specifically to fit a cramped grid column
          (see its own doc comment); this modal has plenty of horizontal room, so splitting here
          just wastes vertical space for no reason. */}
      <div style={{ fontSize: FONT_SIZE_SM, opacity: 0.8, marginBottom: 12 }}>
        {new Date(entry.timestamp * 1000).toLocaleString()}
      </div>

      <dl style={{ display: "grid", gridTemplateColumns: "auto 1fr", columnGap: 12, rowGap: 6, fontSize: FONT_SIZE_SM }}>
        <dt style={{ opacity: 0.65 }}>{t("activity.actor")}</dt>
        <dd>
          {entry.actor.kind === "token" && entry.actor.display_name
            ? entry.actor.display_name
            : entry.actor.kind === "system" && entry.actor.display_name
              ? entry.actor.display_name
              : entry.actor.kind === "session"
                ? t("activity.sessionUser")
                : t("activity.anonymousUser")}
          {/* The row's own chip only shows the token id on hover (a `Tooltip`) — this is already
              the full-detail view, so it's shown directly instead of hidden behind a hover. */}
          {entry.actor.kind === "token" && entry.actor.id && (
            <span style={{ fontFamily: "monospace", opacity: 0.65, marginLeft: 8 }}>({entry.actor.id})</span>
          )}
        </dd>

        <dt style={{ opacity: 0.65 }}>{t("activity.operationContent")}</dt>
        <dd>
          <OperationDescription entry={entry} />
        </dd>

        {entry.client_ip && (
          <>
            <dt style={{ opacity: 0.65 }}>{t("activity.ipAddress")}</dt>
            <dd>
              <IpGeoLink ip={entry.client_ip} />
            </dd>
          </>
        )}

        {entry.caused_by && (
          <>
            <dt style={{ opacity: 0.65 }}>{t("activity.causedBy")}</dt>
            <dd>{entry.caused_by.description}</dd>
          </>
        )}
      </dl>

      {showMetadataDiff ? (
        <div style={{ marginTop: 16 }}>
          <h3 style={{ fontSize: FONT_SIZE_SM, opacity: 0.65 }}>{t("activity.changes")}</h3>
          <MetadataDiff
            before={(entry.before ?? {}) as MetadataUpdateBefore}
            after={entry.after as MetadataUpdateAfter}
          />
        </div>
      ) : (
        <>
          {entry.before != null && (
            <div style={{ marginTop: 16 }}>
              <h3 style={{ fontSize: FONT_SIZE_SM, opacity: 0.65 }}>{t("activity.before")}</h3>
              <CodeBlock code={JSON.stringify(entry.before, null, 2)} language="json" />
            </div>
          )}
          {entry.after != null && (
            <div style={{ marginTop: 16 }}>
              <h3 style={{ fontSize: FONT_SIZE_SM, opacity: 0.65 }}>{t("activity.after")}</h3>
              <CodeBlock code={JSON.stringify(entry.after, null, 2)} language="json" />
            </div>
          )}
        </>
      )}
    </Modal>
  )
}
