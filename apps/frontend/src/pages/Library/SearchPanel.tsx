import { Autocomplete } from "@base-ui/react/autocomplete"
import type { ReactNode } from "react"
import { useState } from "react"
import { useTranslation } from "react-i18next"

import { DateTimeField } from "@/components/common-ui/Form/DateTimeField"
import { useMenuPalette } from "@/hooks/useMenuPalette"
import {
  buildDateRangeToken,
  QUICK_FILTERS,
  type QuickFilter,
  quickFilterActive,
  type SearchSuggestion,
  suggestionKey,
} from "@/lib/searchSuggestions"
import { FONT_SIZE_SM } from "@/theme"

/** Section heading inside the search panel — small, muted, and non-interactive: it groups rows,
 *  it is not itself a row (Chrome's own omnibox panel does the same with "Search Google for…"). */
function SectionLabel({ children }: { children: ReactNode }) {
  return (
    <div
      style={{
        // Top padding is the *only* gap between the bar and the first heading now that the panel's
        // own top padding just clears the bar.
        padding: "4px 12px 2px",
        fontSize: 11,
        opacity: 0.6,
        letterSpacing: 0.02,
      }}
    >
      {children}
    </div>
  )
}

/** One suggestion row: kind marker, single-line text, right-aligned hit count. Deliberately *not*
 *  a colored pill — the panel is a list of what you can pick, and the only color in it is the
 *  highlighted row's own accent (see `.search-panel-item-highlighted` in the theme files). */
function SuggestionRow({
  suggestion,
  count,
}: {
  suggestion: SearchSuggestion
  count?: number
}) {
  const { t } = useTranslation()
  const kindLabel =
    suggestion.kind === "tag"
      ? t("library.suggestionKindTag")
      : suggestion.kind === "title"
        ? t("library.suggestionKindTitle")
        : t("library.suggestionKindPhrase")
  // Icon per row, like the favicon an address bar shows: the group heading already names the kind
  // for sighted users, so the icon carries it visually and `title`/`aria-label` carry it for
  // assistive tech rather than a redundant text column.
  const kindIcon =
    suggestion.kind === "tag"
      ? "fa fa-tag"
      : suggestion.kind === "title"
        ? "fa fa-book"
        : "fa fa-quote-left"
  return (
    <span style={{ display: "flex", alignItems: "center", gap: 8, minWidth: 0, width: "100%" }}>
      <span
        title={kindLabel ?? undefined}
        aria-label={kindLabel ?? undefined}
        style={{ opacity: 0.55, fontSize: 11, flexShrink: 0, width: 14, textAlign: "center" }}
      >
        <i className={kindIcon} aria-hidden="true"></i>
      </span>
      <span
        style={{
          flex: 1,
          minWidth: 0,
          overflow: "hidden",
          textOverflow: "ellipsis",
          whiteSpace: "nowrap",
          // Phrase rows are literal filter text; monospace makes the quotes and `ns:"…"` spelling
          // readable at a glance. Titles stay in the UI font — they are content, not syntax.
          fontFamily: suggestion.kind === "phrase" ? "monospace" : undefined,
        }}
      >
        {suggestion.label}
      </span>
      {count !== undefined && (
        <span style={{ opacity: 0.55, fontSize: 11, flexShrink: 0 }}>
          {suggestion.kind === "tag"
            ? t("library.suggestionTagCount", { n: count })
            : t("library.suggestionCount", { n: count })}
        </span>
      )}
    </span>
  )
}

/**
 * The Library search box's address-bar-style panel: suggestions grouped by kind, one-click state
 * filters, and an added-date range that compiles to the engine's own `date_added:>=… <=…` syntax.
 *
 * Rendered inside `Autocomplete.Popup`, so every *pickable* row is an `Autocomplete.Item` and gets
 * Base UI's keyboard/aria handling (↑/↓, Enter, `data-highlighted`) for free; the date-range row is
 * deliberately not an item — it holds real inputs, which need Tab rather than arrow keys.
 */
export function SearchPanel({
  suggestions,
  recentSearches,
  filter,
  onToggleToken,
  onInsertToken,
}: {
  suggestions: SearchSuggestion[]
  /** Previously applied filters, most recent first — the panel's own "history" rows, shown before
   *  you have typed anything (an address bar's equivalent of recent sites). */
  recentSearches: string[]
  /** The query as it stands, so a quick-filter chip can show whether it is already on. */
  filter: string
  /** Turns one quick-filter chip on or off in the current query. */
  onToggleToken: (chip: QuickFilter) => void
  /** Appends a token to the current filter — used by the date-range row, the only control here
   *  that isn't an `Autocomplete.Item` (rows dispatch through the input's `onValueChange`). */
  onInsertToken: (token: string) => void
}) {
  const { t } = useTranslation()
  // Only for the sticky footer's own opaque background/border below — the fields themselves keep
  // the legacy `stdinput` look.
  const palette = useMenuPalette()

  const [from, setFrom] = useState("")
  const [to, setTo] = useState("")

  const byKind = (kind: SearchSuggestion["kind"]) =>
    suggestions.filter((suggestion) => suggestion.kind === kind)

  const itemStyle: React.CSSProperties = {
    display: "flex",
    alignItems: "center",
    padding: "6px 12px",
    cursor: "pointer",
    fontSize: FONT_SIZE_SM,
    gap: 8,
  }

  /**
   * The date fields deliberately carry the same legacy `stdinput` class as the bar itself rather
   * than an invented look: legacy already defines one input appearance (background, border color,
   * font size, margins) for the whole app, and a hand-rolled border/color here was visibly a
   * different control next to the `.stdbtn` beside it. Only the outer box is pinned — `height: 21px`
   * box-border is exactly `.stdbtn`'s own height, so the two sit level — plus a width, because
   * `.stdinput`'s own `width: 80%` would stretch across the row.
   */

  return (
    <div style={{ padding: "2px 0 0" }}>
      {/*
        One `Autocomplete.List`, with one `Autocomplete.Group` per section — the documented
        composition (a separate `List` per section would register several lists for one popup and
        leave arrow-key traversal undefined across them).
      */}
      <Autocomplete.List>
        {(
          [
            ["phrase", "library.suggestionGroupPhrase"],
            ["title", "library.suggestionGroupTitle"],
            ["tag", "library.suggestionGroupTag"],
          ] as const
        ).map(([kind, labelKey]) => {
          const rows = byKind(kind)
          if (rows.length === 0) return null
          return (
            <Autocomplete.Group key={kind} items={rows}>
              <Autocomplete.GroupLabel>
                <SectionLabel>{t(labelKey)}</SectionLabel>
              </Autocomplete.GroupLabel>
              {rows.map((suggestion) => (
                <Autocomplete.Item
                  key={suggestionKey(suggestion)}
                  value={suggestion}
                  style={itemStyle}
                  className="search-panel-item-highlighted"
                >
                  <SuggestionRow
                    suggestion={suggestion}
                    count={suggestion.kind === "title" ? undefined : suggestion.count}
                  />
                </Autocomplete.Item>
              ))}
            </Autocomplete.Group>
          )
        })}

        {recentSearches.length > 0 && (
          <Autocomplete.Group items={recentSearches}>
            <Autocomplete.GroupLabel>
              <SectionLabel>{t("library.searchPanelRecent")}</SectionLabel>
            </Autocomplete.GroupLabel>
            {recentSearches.map((filter) => (
              // No entry in the search bar's own action map: picking a history row *replaces* the
              // query with it, which is exactly what the input's default `onValueChange` path does.
              <Autocomplete.Item
                key={`recent:${filter}`}
                value={filter}
                style={itemStyle}
                className="search-panel-item-highlighted"
              >
                <span style={{ opacity: 0.55, fontSize: 11, flexShrink: 0, width: 14, textAlign: "center" }}>
                  <i className="fa fa-history" aria-hidden="true"></i>
                </span>
                <span
                  style={{
                    fontFamily: "monospace",
                    flex: 1,
                    overflow: "hidden",
                    textOverflow: "ellipsis",
                    whiteSpace: "nowrap",
                  }}
                >
                  {filter}
                </span>
              </Autocomplete.Item>
            ))}
          </Autocomplete.Group>
        )}

      </Autocomplete.List>

      {/*
        Quick filters as toggleable chips rather than one row each. They are *outside* the list on
        purpose: a button inside a `role="listbox"` is invalid aria, and these are toggles that keep
        the panel open (stack several, then press Enter) rather than rows that complete a query — the
        same reason the date range below lives outside it. Wrapping lets a dozen of them fit in two
        or three lines instead of a dozen.
      */}
      <div style={{ borderTop: `1px solid ${palette.border}`, paddingTop: 2 }}>
        <SectionLabel>{t("library.searchPanelQuickFilters")}</SectionLabel>
        <div style={{ display: "flex", flexWrap: "wrap", gap: 4, padding: "2px 12px 6px" }}>
          {QUICK_FILTERS.map((chip) => {
            const on = quickFilterActive(filter, chip)
            // A chip that inserts a placeholder says what to do next rather than just naming the
            // token: "add bookmark:"" " is not advice anyone can act on.
            const hint =
              chip.caret !== undefined
                ? t("library.searchPanelValueFilter", { token: chip.namespace ?? chip.token })
                : t(on ? "library.searchPanelRemoveFilter" : "library.searchPanelAddFilter", {
                    token: chip.token,
                  })
            return (
              <button
                key={chip.token}
                type="button"
                // The panel stays open on click, so the chip's own state (not a selection) is what
                // `aria-pressed` describes.
                aria-pressed={on}
                title={hint}
                onClick={() => onToggleToken(chip)}
                // Layout only: the "on" fill/ink come from `.search-chip-on` (a real class per
                // theme, so each one fills with its own accent — see those rules' own note), and
                // the "off" look is the palette's plain outline. Emitting the border inline only
                // when off keeps the class's `border-color` from being overridden by it.
                className={on ? "search-chip-on" : undefined}
                style={{
                  display: "inline-flex",
                  alignItems: "center",
                  gap: 6,
                  padding: "1px 6px",
                  borderRadius: 4,
                  cursor: "pointer",
                  fontFamily: "monospace",
                  fontSize: 11,
                  lineHeight: "16px",
                  ...(on
                    ? { opacity: 1 }
                    : {
                        background: "transparent",
                        color: palette.text,
                        border: `1px solid ${palette.border}`,
                        opacity: 0.75,
                      }),
                }}
              >
                {chip.label ?? chip.token}
                <i
                  className={on ? "fa fa-minus" : "fa fa-plus"}
                  aria-hidden="true"
                  style={{ fontSize: 9, opacity: 0.85 }}
                ></i>
              </button>
            )
          })}
        </div>
      </div>

      {/*
        Sticky footer: deliberately outside the list (it holds real inputs, so it takes Tab, not
        arrows) *and* pinned to the panel's bottom edge, so the date range stays reachable while the
        suggestion/filter rows scroll behind it. The opaque background is what hides those rows —
        it is the same palette colour the panel itself paints.
      */}
      <div
        style={{
          position: "sticky",
          bottom: 0,
          background: palette.bg,
          borderTop: `1px solid ${palette.border}`,
          // Was the root's own bottom padding; carried here instead so the footer's stuck position
          // and its natural end position are the same box (otherwise the last 2px of scroll unhook
          // it — visible as a jitter right at the bottom).
          paddingBottom: 2,
        }}
      >
        <SectionLabel>{t("library.searchPanelDateAdded")}</SectionLabel>
        <div style={{ display: "flex", alignItems: "center", gap: 6, padding: "2px 12px 8px" }}>
        <DateTimeField
          mode="date"
          value={from}
          onChange={setFrom}
          style={{ width: 116, minWidth: 0 }}
        />
        <span style={{ opacity: 0.6 }}>–</span>
        <DateTimeField
          mode="date"
          value={to}
          onChange={setTo}
          style={{ width: 116, minWidth: 0 }}
        />
        <button
          type="button"
          className="stdbtn"
          // Same fixed outer box as the date fields above (`.stdbtn` already *is* 21px tall with an
          // 8pt font, so this only makes the box model explicit on both sides of the row).
          // `.stdbtn` carries `margin: 4px 1px 0`; under the row's `align-items: center` that margin
          // box is what gets centred, leaving the button 2px lower than the fields beside it.
          style={{ height: 21, boxSizing: "border-box", margin: 0 }}
          disabled={buildDateRangeToken(from, to) === null}
          onClick={() => {
            const token = buildDateRangeToken(from, to)
            if (token) onInsertToken(token)
          }}
        >
          {t("library.searchPanelDateApply")}
        </button>
        </div>
      </div>
    </div>
  )
}
