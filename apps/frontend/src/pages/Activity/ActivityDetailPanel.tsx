import { useTranslation } from "react-i18next"

import { useApiTokens } from "@/api/hooks"
import type { ActivityEntry } from "@/api/types"
import { CodeBlock, IpGeoLink, Modal, Tooltip } from "@/components/Display"
import { FONT_SIZE_SM } from "@/theme"

import { actorChipParts } from "./activityActor"
import { ActivityChip } from "./ActivityChip"
import { outcomeColor } from "./activityColors"
import { actionTypeLabel, outcomeLabel } from "./activityTarget"
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
  const apiTokens = useApiTokens()
  const token = entry.actor.kind === "token" && entry.actor.id ? apiTokens.data?.find((tk) => tk.id === entry.actor.id) : undefined
  const { label: actorLabel, color: actorColor, tooltip: actorTooltip } = actorChipParts(t, entry, token)
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

      {/* `alignItems: "baseline"` — grid's own default (`"stretch"`) stretches each `dt`/`dd` cell
          to the row's full track height and top-aligns its content inside that box, which reads as
          visually misaligned whenever the two cells' content has different line-height/font
          metrics (a plain-text `dt` next to a `dd` wrapping an `ActivityChip`'s own padded pill,
          e.g.) even though their boxes' own top edges are pixel-identical — confirmed live via
          `getBoundingClientRect()`. Baseline alignment instead lines up each row's actual text
          baselines, which is what "aligned" visually means here. Every `dd` below also gets an
          explicit `margin: 0` — the `<dl>`/`<dd>` UA stylesheet default is a 40px *inline-start*
          margin (a browser default meant for a plain unstyled definition list, not a CSS Grid
          layout), which stacked on top of this grid's own 12px `columnGap` to leave a much wider
          gap between the two columns than either value alone would suggest. */}
      <dl
        style={{
          display: "grid",
          gridTemplateColumns: "auto 1fr",
          alignItems: "baseline",
          columnGap: 12,
          rowGap: 6,
          fontSize: FONT_SIZE_SM,
        }}
      >
        <dt style={{ opacity: 0.65 }}>{t("activity.outcome")}</dt>
        <dd style={{ margin: 0 }}>
          <ActivityChip color={outcomeColor(entry.outcome.status)}>{outcomeLabel(t, entry.outcome.status)}</ActivityChip>
        </dd>

        <dt style={{ opacity: 0.65 }}>{t("activity.actor")}</dt>
        <dd style={{ margin: 0 }}>
          <ActivityChip color={actorColor}>
            {actorTooltip ? (
              <Tooltip label={actorTooltip} wrapperStyle={{ alignItems: "center" }}>
                {actorLabel}
              </Tooltip>
            ) : (
              actorLabel
            )}
          </ActivityChip>
        </dd>

        <dt style={{ opacity: 0.65 }}>{t("activity.operationContent")}</dt>
        <dd style={{ margin: 0 }}>
          <OperationDescription entry={entry} />
        </dd>

        {entry.outcome.status === "failure" && (
          <>
            <dt style={{ opacity: 0.65 }}>{t("activity.failureReason")}</dt>
            <dd style={{ margin: 0, color: outcomeColor("failure").bg }}>{entry.outcome.reason}</dd>
          </>
        )}

        {entry.client_ip && (
          <>
            <dt style={{ opacity: 0.65 }}>{t("activity.ipAddress")}</dt>
            <dd style={{ margin: 0 }}>
              <IpGeoLink ip={entry.client_ip} />
            </dd>
          </>
        )}

        {entry.caused_by && (
          <>
            <dt style={{ opacity: 0.65 }}>{t("activity.causedBy")}</dt>
            <dd style={{ margin: 0 }}>{entry.caused_by.description}</dd>
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
