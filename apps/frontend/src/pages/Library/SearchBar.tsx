import { Autocomplete } from "@base-ui/react/autocomplete"
import type { RefObject } from "react";
import { useMemo } from "react";
import { useTranslation } from "react-i18next";

import { Button, IconButtonWithTooltip } from "@/components/common-ui/Form";
import { ClickPopover, SearchSyntaxHelp } from "@/components/Display";
import { useMenuPalette } from "@/hooks/useMenuPalette";
import {
  QUICK_FILTER_TOKENS,
  type SearchSuggestion,
  suggestionSelectionText,
} from "@/lib/searchSuggestions";
import { FLOATING_POPUP_REVEAL_DOWN_CLASSES, FLOATING_POPUP_SHADOW, FONT_SIZE_SM, Z_OVERLAY_CONTENT } from "@/theme";

import { SearchPanel } from "./SearchPanel";

/** Z_OVERLAY_CONTENT is imported for the caller's own overlay stacking; the panel itself rides
 *  Base UI's Positioner, which needs the z-index to clear the legacy layout it floats over. */

export function SearchBar({
  filterInput,
  autocompleteOpen,
  suggestions,
  recentSearches,
  multiSelect,
  searchInputRef,
  onFilterInputChange,
  onAutocompleteOpenChange,
  onApplyFilter,
  onClearFilter,
  onSuggestionSelect,
  onInsertToken,
  onPickRecent,
  onToggleMultiSelect,
  onAiSmartTankoubon,
  loggedIn,
}: {
  filterInput: string;
  autocompleteOpen: boolean;
  suggestions: SearchSuggestion[];
  recentSearches: string[];
  multiSelect: boolean;
  searchInputRef: RefObject<HTMLInputElement | null>;
  onFilterInputChange: (value: string, openAutocomplete: boolean) => void;
  onAutocompleteOpenChange: (open: boolean) => void;
  onApplyFilter: () => void;
  onClearFilter: () => void;
  onSuggestionSelect: (suggestion: SearchSuggestion) => void;
  onInsertToken: (token: string) => void;
  /** Runs a recent-search row as a complete query rather than filling the bar with it. */
  onPickRecent: (filter: string) => void;
  onToggleMultiSelect: () => void;
  onAiSmartTankoubon: () => void;
  /** 007: batch selection and AI tankoubon creation are write/admin workflows — hidden for a
   *  guest visitor, not merely non-functional buttons. */
  loggedIn: boolean;
}) {
  const { t } = useTranslation();
  const palette = useMenuPalette();

  /**
   * "The input text a picked row produces" → "what picking it should actually do".
   *
   * Base UI's Autocomplete writes the picked row's own string into the input (through
   * `itemToStringValue`), and the input is controlled by the caller's filter state. Rather than
   * accept that text — which would be wrong for every row we have: a tag row must rewrite only the
   * span it was built from, a title row must open an archive, and a quick filter must *append* — the
   * change is intercepted here and mapped back to the row's real action. A user who happens to type
   * text identical to a row's own string gets that row's action applied on Enter, which is the same
   * thing the row would have done anyway.
   */
  const selectionActions = useMemo(() => {
    const actions = new Map<string, () => void>()
    for (const suggestion of suggestions) {
      actions.set(suggestionSelectionText(suggestion), () => onSuggestionSelect(suggestion))
    }
    for (const token of QUICK_FILTER_TOKENS) {
      actions.set(token, () => onInsertToken(token))
    }
    // Registered so a pick *runs* the stored query; without an entry here it would fall through to
    // the plain text-edit path and only fill the bar.
    for (const filter of recentSearches) {
      actions.set(filter, () => onPickRecent(filter))
    }
    return actions
  }, [suggestions, recentSearches, onSuggestionSelect, onInsertToken, onPickRecent])

  return (
    <div style={{ display: "flex", flexWrap: "wrap", gap: 4, alignItems: "center", justifyContent: "center" }}>
      <div style={{ position: "relative", flex: "1 1 300px", maxWidth: 450, boxSizing: "border-box" }}>
        <Autocomplete.Root
          // Every selectable row, including the quick filters: Base UI needs the whole value space
          // to keep keyboard highlighting and `itemToStringValue` consistent across both groups.
          items={
            [...suggestions, ...recentSearches, ...QUICK_FILTER_TOKENS] as (
              | SearchSuggestion
              | string
            )[]
          }
          itemToStringValue={(item: SearchSuggestion | string) =>
            typeof item === "string" ? item : suggestionSelectionText(item)
          }
          value={filterInput}
          onValueChange={(next: string) => {
            const action = selectionActions.get(next)
            if (action) {
              action()
              return
            }
            onFilterInputChange(next, true)
          }}
          filter={null}
          open={autocompleteOpen}
          onOpenChange={onAutocompleteOpenChange}
          // Address-bar feel: the panel opens on click, and the first row is already highlighted so
          // Enter alone takes the most likely suggestion.
          openOnInputClick
          autoHighlight
        >
          <Autocomplete.InputGroup
            style={{
              width: "100%",
              // The Chrome effect is "panel behind, bar inside it at its original position", and
              // the panel is portaled to `document.body` (Base UI requires the Portal), so this has
              // to out-rank it by z-index rather than by sibling order. `position: relative` gives
              // this element its own stacking context at `Z_OVERLAY_CONTENT + 1`; any page ancestor
              // that itself creates a stacking context would cap this — verified with
              // `elementFromPoint` at the bar's centre, which must resolve to the input.
              position: "relative",
              zIndex: Z_OVERLAY_CONTENT + 1,
            }}
          >
            <Autocomplete.Input
              id="search-input"
              ref={searchInputRef}
              // `stdinput` is not decoration: the theme stylesheet's whole input look (background,
              // border, hover/focus colors, and the `margin: 4px 1px 0` the slot offset below is
              // derived from) hangs off that class, and our own `Input` wrapper normally prepends it.
              // `search-input-focus` replaces the browser's default blue focus outline with this
              // theme's own accent (see that class in each of the five theme files).
              className="stdinput search search-input-focus"
              style={{
                width: "100%",
                maxWidth: "none",
                paddingRight: 26,
                boxSizing: "border-box",
                // Closed: exactly the legacy control, untouched. Open: the same element re-dressed as
                // the "new address bar" sitting inside the panel — rounded and palette-colored, so it
                // reads as a new field rather than the old square one showing through.
              }}
              onKeyDown={(event: React.KeyboardEvent<HTMLInputElement>) => {
                if (event.key === "Enter" && !event.defaultPrevented) onApplyFilter()
                if (event.key === "Escape") onAutocompleteOpenChange(false)
              }}
              placeholder={t("library.searchTitleArtistSeriesLanguage") ?? undefined}
            />
            <span
              style={{
                position: "absolute",
                // `.stdinput`'s own `margin: 4px 1px 0` puts its box 2px below this wrapper's
                // center, so centering on the wrapper would sit the icon 2px high; +2px corrects it
                // (same offset `common-ui/Form/InputGroup` uses for its own slots).
                top: "calc(50% + 2px)",
                right: 4,
                transform: "translateY(-50%)",
                display: "flex",
                alignItems: "center",
                justifyContent: "center",
                color: "inherit",
              }}
            >
              <ClickPopover
                maxWidth={360}
                label={<SearchSyntaxHelp />}
                trigger={
                  <button
                    type="button"
                    aria-label={t("library.searchSyntaxHelpAria") ?? undefined}
                    title={t("library.searchSyntaxHelp") ?? undefined}
                    style={{
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
            </span>
          </Autocomplete.InputGroup>

          {/*
            No `Autocomplete.Empty` here: with `filter={null}` nothing is ever filtered, so the
            "no results" state is simply an empty suggestions array — the panel then still shows the
            quick filters and the date range, which is exactly the address-bar behavior (the panel
            is useful before you have typed anything).
          */}
          {/*
            `Autocomplete.Portal` is mandatory: Base UI's Positioner asserts it ("<Combobox.Portal>
            is missing"), so the panel is portaled to `document.body` and positioned `fixed` there.
            That means the bar cannot be a *sibling* painted over it — the layering has to be won by
            z-index, which is why the input's own group above carries `Z_OVERLAY_CONTENT + 1`: see
            the comment there, and the check that verified it (`elementFromPoint` at the bar's
            centre must hit the input, not the panel).

            Geometry is Chrome's omnibox, measured against the real control rather than guessed:
            the panel's top edge sits 8px above the bar (`sideOffset`), its top padding then puts the
            first row below the bar, and it is 20px wider with 16px corners.
          */}
          <Autocomplete.Portal>
            <Autocomplete.Positioner
              side="bottom"
              align="start"
              // `side="bottom"` + a *negative* offset is measured from the anchor's **bottom** edge,
              // so "8px above the bar's top" is `-(barHeight + 8)` — read off the anchor's measured
              // rect rather than a literal, because the bar's height comes from each theme's own
              // `.stdinput` font-size (18px in the theme this was measured against).
              sideOffset={({ anchor }) => -(anchor.height + 4)}
              className="outline-none"
              style={{ zIndex: Z_OVERLAY_CONTENT }}
            >
              <Autocomplete.Popup
                // `hide-scrollbar` (index.css): the panel's own scrollbar runs up the right edge
                // into the bar, which is 6px narrower than the panel on each side. Wheel/trackpad
                // scrolling is unaffected; the reveal class is the same constant the other popups'
                // transition pairs with (minus `transformOrigin`, since nothing here scales).
                className={`hide-scrollbar ${FLOATING_POPUP_REVEAL_DOWN_CLASSES}`}
                style={{
                  background: palette.bg,
                  border: `1px solid ${palette.border}`,
                  boxShadow: FLOATING_POPUP_SHADOW,
                  color: palette.text,
                  // A hint of rounding only — the bar it wraps is square (radius 0), so anything
                // larger reads as a different visual language than the legacy controls around it.
                borderRadius: 4,
                // The panel is portaled to `document.body`, and the legacy index page centers its
                // text — without this every row's content renders centered.
                textAlign: "left",
                  // Only what clearing the bar needs — the bar's own height plus the few px the panel
                  // starts above it. The row/heading padding supplies the visible gap, so tightening
                  // that gap is one edit there rather than two here.
                  paddingTop: 26,
                  // 6px of panel on each side of the bar (Chrome's own inset is tighter than the
                  // 10px this started with).
                  width: "calc(var(--anchor-width) + 12px)",
                  marginLeft: -6,
                  maxHeight: "min(70vh, 460px)",
                  overflowY: "auto",
                  overscrollBehavior: "contain",
                  fontSize: FONT_SIZE_SM,
                }}
              >
                <SearchPanel
                  suggestions={suggestions}
                  recentSearches={recentSearches}
                  onInsertToken={onInsertToken}
                />
              </Autocomplete.Popup>
            </Autocomplete.Positioner>
          </Autocomplete.Portal>
        </Autocomplete.Root>
      </div>
      <div className="searchbar-button-row" style={{ display: "flex", flexWrap: "wrap", justifyContent: "center", alignItems: "center" }}>
        <Button id="apply-search" className="searchbtn" style={{ flexGrow: 1 }} onClick={onApplyFilter}>
          {t("library.applyFilter")}
        </Button>
        <Button id="clear-search" className="searchbtn" style={{ flexGrow: 1 }} onClick={onClearFilter}>
          {t("library.clearFilter")}
        </Button>
        {loggedIn && (
          <Button
            id="msm-toggle"
            className={`searchbtn${multiSelect ? " toggled" : ""}`}
            style={{ flexGrow: 1 }}
            onClick={() => void onToggleMultiSelect()}
          >
            {t("batch.selectArchives")}
          </Button>
        )}
        {loggedIn && (
          <IconButtonWithTooltip
            icon="fa fa-robot"
            title={t("library.aiSmartCreateTankoubon")}
            description={t("library.analyzeArchivesNotYetIn")}
            // Not `searchbtn` — its legacy min-width: 100px !important would stretch this icon button wide.
            wrapperStyle={{ alignItems: "center", height: 21 }}
            style={{ marginTop: 4 }}
            onClick={onAiSmartTankoubon}
          />
        )}
      </div>
    </div>
  );
}
