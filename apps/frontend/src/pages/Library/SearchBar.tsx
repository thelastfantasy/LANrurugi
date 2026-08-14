import type { RefObject } from "react"
import { useTranslation } from "react-i18next"

import { IconButtonWithTooltip, PopupMenu, PopupMenuItem } from "@/components/Display"

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
          style={{ width: "100%", maxWidth: "none" }}
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
