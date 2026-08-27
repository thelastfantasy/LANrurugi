import type { RefObject } from "react"
import { useTranslation } from "react-i18next"

import { IconButtonWithTooltip } from "@/components/common-ui/Display"
import { PopupMenu, PopupMenuItem } from "@/components/common-ui/Display"
import { ClickPopover, SearchSyntaxHelp } from "@/components/Display"

interface TagSuggestion { label: string; insertValue: string }
import { Z_OVERLAY_CONTENT } from "@/theme"

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
          label={<SearchSyntaxHelp />}
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
