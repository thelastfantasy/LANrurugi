import type { ReactNode } from "react"
import type { RefObject } from "react"
import { useTranslation } from "react-i18next"

import { ClickPopover, IconButtonWithTooltip, PopupMenu, PopupMenuItem } from "@/components/Display"

interface TagSuggestion { label: string; insertValue: string }
import { FONT_SIZE_SM, Z_OVERLAY_CONTENT } from "@/theme"

/** Inline `<code>`-styled span for a literal search-syntax example (`pages:>100`, `-tag`, …)
 * inside the syntax-help popover — plain JSX children, not `dangerouslySetInnerHTML`: these
 * examples are static string/JSX literals this component itself writes, never user- or
 * translation-supplied markup, so there's no HTML string to parse/inject in the first place. */
function SyntaxExample({ children }: { children: ReactNode }) {
  return (
    <code
      style={{
        fontFamily: "monospace",
        fontSize: FONT_SIZE_SM,
        background: "rgba(128, 128, 128, 0.18)",
        borderRadius: 3,
        padding: "1px 5px",
      }}
    >
      {children}
    </code>
  )
}

export function SearchBar({
  filterInput,
  autocompleteOpen,
  tagSuggestions,
  multiSelect,
  searchInputRef,
  onFilterInputChange,
  onAutocompleteOpenChange,
  onApplyFilter,
  onClearFilter,
  onSuggestionSelect,
  onToggleMultiSelect,
  onAiSmartTankoubon,
}: {
  filterInput: string
  autocompleteOpen: boolean
  tagSuggestions: TagSuggestion[]
  multiSelect: boolean
  searchInputRef: RefObject<HTMLInputElement | null>
  onFilterInputChange: (value: string, openAutocomplete: boolean) => void
  onAutocompleteOpenChange: (open: boolean) => void
  onApplyFilter: () => void
  onClearFilter: () => void
  onSuggestionSelect: (insertValue: string) => void
  onToggleMultiSelect: () => void
  onAiSmartTankoubon: () => void
}) {
  const { t } = useTranslation()

  return (
    <>
      <span style={{ position: "relative", display: "inline-block", width: "80%", maxWidth: 450, boxSizing: "border-box" }}>
        <input
          id="search-input"
          ref={searchInputRef}
          className="search stdinput"
          style={{ width: "100%", maxWidth: "none", paddingRight: 26, boxSizing: "border-box" }}
          value={filterInput}
          autoComplete="off"
          onChange={(e) => onFilterInputChange(e.target.value, true)}
          onFocus={() => onAutocompleteOpenChange(true)}
          onBlur={() => setTimeout(() => onAutocompleteOpenChange(false), 150)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              onApplyFilter()
              onAutocompleteOpenChange(false)
            }
            if (e.key === "Escape") onAutocompleteOpenChange(false)
          }}
          placeholder={t("library.searchTitleArtistSeriesLanguage") ?? undefined}
        />
        {/* End-adornment-style help trigger, nested inside the search box's own relatively
            positioned wrapper (MUI/Chakra `InputAdornment`/`InputRightElement` convention) rather
            than a separate button sitting outside the input — `right: 4` clears the input's own
            border. `top: calc(50% + 2px)` (not a plain `50%`) accounts for `.stdinput`'s own
            asymmetric `margin: 4px 1px 0` (all 5 themes agree on this value) shifting the input's
            visual box 2px below this wrapper `<span>`'s own vertical center — a plain `50%`
            centers against the wrapper, which reads as 2px too high against the input itself
            (confirmed live). `color: "inherit"` (not a hardcoded value) picks up the page's own
            themed text color the surrounding `.idi`/body already carries — matches `.stdinput`'s
            own themed `color` despite not being a CSS-inheritance descendant of it (verified live
            per-theme: both resolve to the same color, e.g. g.css's `rgb(92, 13, 17)`). */}
        <ClickPopover
          maxWidth={360}
          label={
            <div>
              <div style={{ fontWeight: 700, marginBottom: 8 }}>{t("library.searchSyntaxHelp")}</div>
              {/* `lrr.css` (legacy, not editable — see CLAUDE.md) carries a bare `li { list-style:
                  none }` tag-selector rule, which wins against `list-style-type: disc` set only on
                  the parent `<ul>` — `list-style-type` is inherited, but a plain, non-inherited
                  declaration on the element itself (this `li` rule) beats an inherited value in the
                  cascade regardless of where the inherited value came from, so each `<li>` below
                  re-asserts `disc` directly on itself rather than relying on inheriting it from the
                  `<ul>` (confirmed live: the `<ul>`-only version silently rendered with zero
                  bullets). Also plain block-flow, not `display: flex` — a flex container never
                  renders its children's `::marker` at all (the browser's own list-marker box only
                  paints in normal block/list-item flow), which was this component's very first,
                  independent reason the bullets disappeared before the `lrr.css` rule above was
                  even found. `<li>`'s own `marginBottom` reproduces the row-spacing a `gap` would
                  have given a flex layout, without that flex/marker conflict. */}
              <ul style={{ margin: 0, paddingLeft: 18 }}>
                <li style={{ listStyleType: "disc", marginBottom: 6 }}>{t("library.searchSyntaxAndSeparator")}</li>
                <li style={{ listStyleType: "disc", marginBottom: 6 }}>
                  {t("library.searchSyntaxNegation")} <SyntaxExample>-tag</SyntaxExample>
                </li>
                <li style={{ listStyleType: "disc", marginBottom: 6 }}>
                  {t("library.searchSyntaxExactMatch")} <SyntaxExample>&quot;tag&quot;</SyntaxExample> / <SyntaxExample>tag$</SyntaxExample>
                </li>
                {/* Legacy's own tokenizer only ever splits on comma (a bare space just gets
                    skipped, not treated as a delimiter), so `tag with spaces$` was a real, useful
                    legacy idiom — a whole space-containing value, quote-free, made exact by a
                    trailing `$`. This app's own tokenizer (`grammar.rs`, issue #59) made space a
                    real delimiter too, which breaks that idiom: `tag with spaces$` now parses as
                    three independent AND'd tokens (`tag`, `with`, `spaces$`), not one. Worth its
                    own explicit line rather than leaving it to be inferred from the exact-match
                    line above, since a user coming from legacy specifically reaching for that bare
                    `$` idiom would otherwise silently get three loosely-ANDed fuzzy tokens instead
                    of the one exact tag they meant. */}
                <li style={{ listStyleType: "disc", marginBottom: 6 }}>{t("library.searchSyntaxSpaceNeedsQuotes")}</li>
                <li style={{ listStyleType: "disc", marginBottom: 6 }}>
                  {t("library.searchSyntaxQuoteExactCombo")} <SyntaxExample>&quot;tag with spaces&quot;$</SyntaxExample>
                </li>
                <li style={{ listStyleType: "disc", marginBottom: 6 }}>
                  {t("library.searchSyntaxMultiTagExample")}{" "}
                  <SyntaxExample>
                    female:&quot;huge breasts&quot; female:milf
                  </SyntaxExample>
                </li>
                <li style={{ listStyleType: "disc", marginBottom: 6 }}>
                  {t("library.searchSyntaxEscapedQuote")}{" "}
                  <SyntaxExample>artist:&quot;foo\&quot;bar&quot;</SyntaxExample>
                </li>
                <li style={{ listStyleType: "disc", marginBottom: 6 }}>{t("library.searchSyntaxWildcard")}</li>
                <li style={{ listStyleType: "disc", marginBottom: 6 }}>
                  {t("library.searchSyntaxDateAdded")} <SyntaxExample>date_added:2026-08-20</SyntaxExample>
                </li>
                <li style={{ listStyleType: "disc", marginBottom: 6 }}>
                  {t("library.searchSyntaxNumericCompare")} <SyntaxExample>pages:&gt;100</SyntaxExample>, <SyntaxExample>read:&lt;5</SyntaxExample>
                </li>
                <li style={{ listStyleType: "disc" }}>
                  {t("library.searchSyntaxRating")} <SyntaxExample>rating:&gt;=4</SyntaxExample>, <SyntaxExample>rating:&lt;3</SyntaxExample>, <SyntaxExample>rating:=5</SyntaxExample>
                </li>
              </ul>
            </div>
          }
          trigger={
            <button
              type="button"
              aria-label={t("library.searchSyntaxHelpAria") ?? undefined}
              title={t("library.searchSyntaxHelp") ?? undefined}
              style={{
                position: "absolute",
                top: "calc(50% + 2px)",
                right: 4,
                transform: "translateY(-50%)",
                width: 16,
                height: 16,
                minWidth: 16,
                padding: 0,
                border: "none",
                background: "transparent",
                color: "inherit",
                opacity: 0.6,
                fontSize: 14,
                lineHeight: 1,
                cursor: "pointer",
              }}
            >
              <i className="fa fa-question-circle" aria-hidden="true"></i>
            </button>
          }
        />
        {autocompleteOpen && tagSuggestions.length > 0 && (
          <PopupMenu
            portal={false}
            style={{ position: "absolute", top: "100%", left: 0, zIndex: Z_OVERLAY_CONTENT, minWidth: "100%", maxHeight: 220, overflowY: "auto" }}
          >
            {tagSuggestions.map((s) => (
              <PopupMenuItem
                key={s.label}
                onMouseDown={(e) => {
                  e.preventDefault()
                  onSuggestionSelect(s.insertValue)
                  onAutocompleteOpenChange(false)
                  searchInputRef.current?.focus()
                }}
              >
                {s.label}
              </PopupMenuItem>
            ))}
          </PopupMenu>
        )}
      </span>
      <input
        id="apply-search"
        className="searchbtn stdbtn"
        type="button"
        value={t("library.applyFilter") ?? undefined}
        onClick={onApplyFilter}
      />
      <input
        id="clear-search"
        className="searchbtn stdbtn"
        type="button"
        value={t("library.clearFilter") ?? undefined}
        onClick={onClearFilter}
      />
      <input
        id="msm-toggle"
        className={`searchbtn stdbtn${multiSelect ? " toggled" : ""}`}
        type="button"
        value={t("batch.selectArchives") ?? undefined}
        onClick={() => void onToggleMultiSelect()}
      />
      <IconButtonWithTooltip
        icon="fa fa-robot"
        title={t("library.aiSmartCreateTankoubon")}
        description={t("library.analyzeArchivesNotYetIn")}
        // Not `searchbtn` — that class carries a legacy `min-width: 100px !important` (sized for
        // this row's own text buttons like "Apply Filter"), which beats `IconButton`'s inline
        // `minWidth` and silently stretches it back into a wide button. `marginTop: 4` reproduces
        // the one part of `.searchbtn` this row's vertical alignment actually needs.
        className="stdbtn"
        style={{ marginTop: 4 }}
        onClick={onAiSmartTankoubon}
      />
    </>
  )
}
