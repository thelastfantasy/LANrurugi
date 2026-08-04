import type { RefObject } from "react"
import { useTranslation } from "react-i18next"

import { PopupMenu, PopupMenuItem } from "@/components/PopupMenu"

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
          placeholder={t("Search Title, Artist, Series, Language or Tags") ?? undefined}
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
        value={t("Apply Filter") ?? undefined}
        onClick={onApplyFilter}
      />
      <input
        id="clear-search"
        className="searchbtn stdbtn"
        type="button"
        value={t("Clear Filter") ?? undefined}
        onClick={onClearFilter}
      />
      <input
        id="msm-toggle"
        className={`searchbtn stdbtn${multiSelect ? " toggled" : ""}`}
        type="button"
        value={t("Select Archives") ?? undefined}
        onClick={() => void onToggleMultiSelect()}
      />
    </>
  )
}
