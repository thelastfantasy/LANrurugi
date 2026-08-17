import { useTranslation } from "react-i18next"

import { useApiTokens } from "@/api/hooks"
import type { ActivityEntry } from "@/api/types"
import { DateTimeStack, IconButton, Tooltip } from "@/components/Display"

import { ActivityChip } from "./ActivityChip"
import { actionTypeColor, actorKindColor } from "./activityColors"
import { actionTypeLabel } from "./activityTarget"
import { OperationDescription } from "./OperationDescription"

/** One row of the Activity list. In wide mode this is a CSS grid row (`display: contents` — see
 * `Settings/ApiTokensSection.tsx`'s own precedent for this pattern: the parent owns
 * `display: grid`, each row just yields its cells into that same grid rather than nesting a
 * table-within-a-table). Below the narrow-viewport breakpoint there isn't room for a 6-column grid
 * at all — rather than keep cramming cells into a shared grid (which is what hid the actor/
 * action-type chips entirely in an earlier version of this row), narrow mode renders each entry as
 * its own self-contained card instead: no column alignment with a header row, so a chip can wrap
 * onto a second line without dragging every other row's cells out of alignment with it.
 *
 * `automatic` entries get the `.activity-row-automatic` theme-adaptive background (defined
 * per-theme in each of the 5 real theme files) so a scanner/metadata-plugin entry visually reads
 * as distinct from a human-triggered one at a glance, without needing to read the actor column
 * first.
 *
 * Actor and action-type both render as the same colored `ActivityChip` pills the filter
 * Comboboxes use (`activityColors.ts`) — a consistent visual language between "what you can filter
 * by" and "what a row actually shows", rather than plain text in the row and colored chips only in
 * the filter UI. A token actor's chip also carries the same id/role hover `Tooltip` the Combobox
 * gives it, for the same reason (a display name alone doesn't disambiguate two same-named or
 * revoked tokens). */
export function ActivityRow({
  entry,
  selected,
  selectable,
  narrow,
  onToggleSelect,
  onOpenDetail,
  onDelete,
}: {
  entry: ActivityEntry
  selected: boolean
  selectable: boolean
  /** Switches between the wide-mode grid-row rendering and the narrow-mode card rendering — see
   * this component's own doc comment above. */
  narrow: boolean
  onToggleSelect: () => void
  onOpenDetail: () => void
  onDelete: () => void
}) {
  const { t } = useTranslation()
  const apiTokens = useApiTokens()
  const token = entry.actor.kind === "token" && entry.actor.id ? apiTokens.data?.find((tk) => tk.id === entry.actor.id) : undefined

  // `fa-user-shield` matches `Settings/ApiTokensSection.tsx`'s own `RoleMarker` — the same glyph
  // that component already uses for a token's "full access" admin role, reused here since a
  // Session actor is exactly that same full-access case (see `AuthMethod::Session`'s own docs:
  // "a real human who's already proven the admin password, never subject to a Guest-role
  // restriction") rather than inventing a second icon for the same concept. A token whose id no
  // longer resolves (`!token`, looked up live against `apiTokens.data`) gets the same
  // `fa-user-slash` treatment `ActivityFilterCombobox.tsx`'s own facet dropdown uses — this row's
  // `entry.actor.display_name` is the real name snapshotted at write time (still legible after
  // revocation, per `Actor::display_name`'s own docs), so unlike that dropdown's own bare-UUID
  // fallback the icon is the only signal here that the token is actually gone now.
  const actorLabel =
    entry.actor.kind === "token" && entry.actor.id && !token
      ? (
          <>
            <i className="fas fa-user-slash activity-chip-danger-icon" aria-hidden="true"></i>{" "}
            {entry.actor.display_name ?? entry.actor.id}
          </>
        )
      : entry.actor.kind === "token" || entry.actor.kind === "system"
        ? (entry.actor.display_name ?? entry.actor.id ?? entry.actor.kind)
        : entry.actor.kind === "session"
          ? (
              <>
                <i className="fas fa-user-shield" aria-hidden="true"></i> {t("activity.sessionUser")}
              </>
            )
          : t("activity.anonymousUser")

  const actorColor = actorKindColor(entry.actor.kind, entry.actor.id ?? undefined)
  const actorTooltip =
    entry.actor.kind === "token" && entry.actor.id ? (
      <>
        <div style={{ fontFamily: "monospace" }}>{entry.actor.id}</div>
        {token && <div>{t(token.role === "guest" ? "Guest" : "Admin")}</div>}
        {!token && <div>{t("activity.revoked")}</div>}
      </>
    ) : undefined

  const actorChip = (
    <ActivityChip color={actorColor}>
      {actorTooltip ? (
        <Tooltip label={actorTooltip} wrapperStyle={{ alignItems: "center" }}>
          {actorLabel}
        </Tooltip>
      ) : (
        actorLabel
      )}
    </ActivityChip>
  )
  const actionChip = <ActivityChip color={actionTypeColor(entry.action_type)}>{actionTypeLabel(t, entry.action_type)}</ActivityChip>

  if (narrow) {
    return (
      <div
        className={entry.auto_or_manual === "automatic" ? "activity-row-automatic" : undefined}
        style={{ display: "flex", alignItems: "center", gap: 6, padding: "10px 4px", cursor: "pointer", minWidth: 0 }}
        onClick={onOpenDetail}
      >
        {/* Its own dedicated column, matching the delete button's own column on the other end of
            the card — not inline with the time/chips row (an earlier version mixed it into that
            wrapped row), so it stays put at a fixed position regardless of how many chips wrap onto
            a second line, and both end columns share the same vertical-centering treatment
            (`alignItems: "center"` on this card's own root, not `"flex-start"`) instead of pinning
            to the card's top edge. */}
        {selectable && (
          <div style={{ flexShrink: 0 }} onClick={(e) => e.stopPropagation()}>
            <input
              type="checkbox"
              aria-label={t("activity.selectEntry") ?? undefined}
              checked={selected}
              onChange={onToggleSelect}
            />
          </div>
        )}
        {/* `minWidth: 0` on this flex child is required for its own children's `overflow: hidden`/
            `textOverflow: ellipsis` (below) to ever take effect — see the wide-mode row's own note
            on this same CSS gotcha. The delete button lives OUTSIDE this flex child instead of
            inside its wrapping first line (an earlier version used `marginLeft: auto` within the
            wrapped chip row) — a `margin-left: auto` item doesn't reliably wrap onto its own new
            line once the preceding wrapped content nearly fills the row's width; it just overflows
            the row instead, confirmed live as the actual cause of horizontal clipping on a real
            390px-wide viewport. A sibling flex column, sized by its own content, can't be pushed
            off-screen this way. */}
        <div style={{ display: "flex", flexDirection: "column", gap: 6, flex: 1, minWidth: 0 }}>
          <div style={{ display: "flex", alignItems: "center", flexWrap: "wrap", gap: 6 }}>
            <span style={{ whiteSpace: "nowrap" }}>
              <DateTimeStack epochSeconds={entry.timestamp} />
            </span>
            {actorChip}
            {actionChip}
          </div>
          <div style={{ textAlign: "left", minWidth: 0, overflow: "hidden", textOverflow: "ellipsis", whiteSpace: "nowrap" }}>
            <OperationDescription entry={entry} />
          </div>
        </div>
        {selectable && (
          <div style={{ flexShrink: 0 }} onClick={(e) => e.stopPropagation()}>
            <IconButton
              icon="fas fa-trash"
              className="stdbtn stdbtn-danger"
              size="medium"
              title={t("common.delete") ?? undefined}
              onClick={onDelete}
            />
          </div>
        )}
      </div>
    )
  }

  // `.activity-row` + a per-theme `.activity-row:hover > *` rule (see each theme CSS file's own
  // docs) is what makes a hover highlight paint across every cell of this row — the row itself is
  // `display: contents` (per this component's own top doc comment: it yields cells straight into
  // the parent grid rather than owning a box of its own), so a plain `:hover` background on the
  // row element itself would have no box to paint onto at all; targeting its children via the
  // child combinator is the only way to get a full-row highlight out of this layout.
  //
  // Every cell below stretches to the grid row's own full track height (no `alignSelf` override —
  // `"stretch"` is grid's own default) instead of the `"center"` an earlier version used:
  // centering left each cell's own box shorter than its row (down to whichever sibling's content
  // happened to be tallest that render), with visible empty space above/below within the row track
  // itself — a per-cell hover highlight on a box that short could never read as one continuous
  // band, confirmed live: it still showed broken segments after bridging the *column* gaps alone.
  // Cell content is centered within the now full-height cell instead, via `display: flex,
  // alignItems: center` on the cell itself.
  //
  // `.activity-row-delete-btn` is hidden by default and only shown on `.activity-row:hover` (same
  // per-theme rule) — a delete affordance visible on every single row at all times read as far
  // more cluttered than showing it only for the row the user is actually looking at.
  return (
    <div
      className={`activity-row${entry.auto_or_manual === "automatic" ? " activity-row-automatic" : ""}`}
      style={{ display: "contents", cursor: "pointer" }}
      onClick={onOpenDetail}
    >
      {/* This first cell's own negative margin is right-only (`"0 -6px 0 0"`), not symmetric — a
          symmetric `-6px` on both sides would also pull its *left* edge past the grid's own outer
          boundary (there's no sibling cell to its left to meet in the middle of a gap with),
          widening the whole table 6px past its container and forcing a horizontal scrollbar,
          confirmed live. Every internal cell still gets the full symmetric `-6px` (its gaps to
          both neighbors need bridging); the row's very last cell (the delete button, below) gets
          the same left-only treatment for the same reason on its own outer edge. */}
      <div
        style={{ display: "flex", alignItems: "center", justifyContent: "center", padding: "8px 6px", margin: "0 -6px 0 0" }}
        onClick={(e) => e.stopPropagation()}
      >
        {selectable && (
          <input type="checkbox" aria-label={t("activity.selectEntry") ?? undefined} checked={selected} onChange={onToggleSelect} />
        )}
      </div>
      <div style={{ display: "flex", alignItems: "center", padding: "8px 6px", margin: "0 -6px", whiteSpace: "nowrap" }}>
        <DateTimeStack epochSeconds={entry.timestamp} />
      </div>
      <div style={{ display: "flex", alignItems: "center", padding: "8px 6px", margin: "0 -6px" }}>{actorChip}</div>
      <div style={{ display: "flex", alignItems: "center", padding: "8px 6px", margin: "0 -6px" }}>{actionChip}</div>
      {/* `minWidth: 0` overrides a CSS grid item's own implicit `min-width: auto` (grid/flex items
          default to never shrinking below their content's own intrinsic width, unlike a normal
          block element) — without it, `overflow`/`textOverflow` below has no effect at all: a
          long unbroken string (a UUID target id/label with no spaces to wrap at) just keeps
          pushing this column, and the whole grid past it, wider than the viewport instead of
          ellipsizing, confirmed live as the real cause of the narrow-viewport horizontal overflow
          reported against an earlier version of this row. `whiteSpace: nowrap` is the third
          required piece — `text-overflow: ellipsis` is a no-op on text that's still wrapping
          across multiple lines. `activity-row-content` gets its own independent hover effect (same
          per-theme rule) since it's independently clickable — opens the same detail modal the rest
          of the row's own click already does, but calling that out visually (rather than relying
          solely on the whole-row highlight) signals it's the specific part carrying that
          affordance, same reasoning a link inside an otherwise plain paragraph gets its own
          underline instead of relying on the paragraph's own hover state alone. */}
      <div
        className="activity-row-content"
        style={{
          display: "flex",
          alignItems: "center",
          padding: "8px 6px",
          margin: "0 -6px",
          minWidth: 0,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
          borderRadius: 4,
        }}
      >
        <OperationDescription entry={entry} />
      </div>
      {selectable && (
        // Left-only negative margin (`"0 0 0 -6px"`) — this is the row's very last cell, so a
        // symmetric `-6px` would also push its *right* edge past the grid's own outer boundary
        // (see the first cell's own matching note above) and force a horizontal scrollbar.
        // `justifyContent: "center"` matches every other cell's own horizontal centering (the
        // delete button previously sat flush against whichever edge its own flex layout happened
        // to collapse to, confirmed live as visibly off-center compared to the checkbox column).
        <div
          style={{ display: "flex", alignItems: "center", justifyContent: "center", padding: "8px 6px", margin: "0 0 0 -6px" }}
          onClick={(e) => e.stopPropagation()}
        >
          <IconButton
            icon="fas fa-trash"
            className="stdbtn stdbtn-danger activity-row-delete-btn"
            size="medium"
            title={t("common.delete") ?? undefined}
            onClick={onDelete}
          />
        </div>
      )}
    </div>
  )
}
