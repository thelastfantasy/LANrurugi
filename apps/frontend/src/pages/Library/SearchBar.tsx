import type { RefObject } from "react";
import { useTranslation } from "react-i18next";

import { PopupMenu, PopupMenuItem } from "@/components/common-ui/Display";
import { Button, IconButtonWithTooltip, Input, InputGroup } from "@/components/common-ui/Form";
import { ClickPopover, SearchSyntaxHelp } from "@/components/Display";

interface TagSuggestion {
  label: string;
  insertValue: string;
}
import { Z_OVERLAY_CONTENT } from "@/theme";

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
  loggedIn,
}: {
  filterInput: string;
  autocompleteOpen: boolean;
  tagSuggestions: TagSuggestion[];
  multiSelect: boolean;
  searchInputRef: RefObject<HTMLInputElement | null>;
  onFilterInputChange: (value: string, openAutocomplete: boolean) => void;
  onAutocompleteOpenChange: (open: boolean) => void;
  onApplyFilter: () => void;
  onClearFilter: () => void;
  onSuggestionSelect: (insertValue: string) => void;
  onToggleMultiSelect: () => void;
  onAiSmartTankoubon: () => void;
  /** 007: batch selection and AI tankoubon creation are write/admin workflows — hidden for a
   *  guest visitor, not merely non-functional buttons. */
  loggedIn: boolean;
}) {
  const { t } = useTranslation();

  return (
    <div style={{ display: "flex", flexWrap: "wrap", gap: 4, alignItems: "center", justifyContent: "center" }}>
      <div style={{ position: "relative", flex: "1 1 300px", maxWidth: 450, boxSizing: "border-box" }}>
        <InputGroup
          style={{ width: "100%" }}
          endElement={
            <ClickPopover
              maxWidth={360}
              label={<SearchSyntaxHelp />}
              trigger={
                <button
                  type="button"
                  aria-label={t("library.searchSyntaxHelpAria") ?? undefined}
                  title={t("library.searchSyntaxHelp") ?? undefined}
                  style={{
                    width: "100%",
                    height: "100%",
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
          }
        >
          <Input
            id="search-input"
            ref={searchInputRef}
            className="search"
            style={{
              width: "100%",
              maxWidth: "none",
              paddingRight: 26,
              boxSizing: "border-box",
            }}
            value={filterInput}
            autoComplete="off"
            onChange={(e) => onFilterInputChange(e.target.value, true)}
            onFocus={() => onAutocompleteOpenChange(true)}
            onBlur={() => setTimeout(() => onAutocompleteOpenChange(false), 150)}
            onKeyDown={(e) => {
              if (e.key === "Enter") {
                onApplyFilter();
                onAutocompleteOpenChange(false);
              }
              if (e.key === "Escape") onAutocompleteOpenChange(false);
            }}
            placeholder={
              t("library.searchTitleArtistSeriesLanguage") ?? undefined
            }
          />
        </InputGroup>
        {autocompleteOpen && tagSuggestions.length > 0 && (
          <PopupMenu
            portal={false}
            style={{
              position: "absolute",
              top: "100%",
              left: 0,
              zIndex: Z_OVERLAY_CONTENT,
              minWidth: "100%",
              maxHeight: 220,
              overflowY: "auto",
            }}
          >
            {tagSuggestions.map((s) => (
              <PopupMenuItem
                key={s.label}
                onMouseDown={(e) => {
                  e.preventDefault();
                  onSuggestionSelect(s.insertValue);
                  onAutocompleteOpenChange(false);
                  searchInputRef.current?.focus();
                }}
              >
                {s.label}
              </PopupMenuItem>
            ))}
          </PopupMenu>
        )}
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
