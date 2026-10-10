import type { RefObject } from "react";
import { useTranslation } from "react-i18next";

import { PopupMenu, PopupMenuItem } from "@/components/common-ui/Display";
import { Button, IconButtonWithTooltip, Input, InputGroup } from "@/components/common-ui/Form";
import { ClickPopover, SearchSyntaxHelp } from "@/components/Display";
import type { SearchSuggestion } from "@/lib/searchSuggestions";
import { Z_OVERLAY_CONTENT } from "@/theme";

/** i18n keys per suggestion kind — a literal map rather than a template-literal key so a typo is a
 *  type error instead of a missing-translation string rendered in the UI. */
const KIND_LABEL_KEY: Record<SearchSuggestion["kind"], string> = {
  tag: "library.suggestionKindTag",
  title: "library.suggestionKindTitle",
  phrase: "library.suggestionKindPhrase",
};

function suggestionKey(s: SearchSuggestion): string {
  return s.kind === "title" ? `title:${s.arcid}` : `${s.kind}:${s.insertValue}`;
}

export function SearchBar({
  filterInput,
  autocompleteOpen,
  suggestions,
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
  suggestions: SearchSuggestion[];
  multiSelect: boolean;
  searchInputRef: RefObject<HTMLInputElement | null>;
  onFilterInputChange: (value: string, openAutocomplete: boolean) => void;
  onAutocompleteOpenChange: (open: boolean) => void;
  onApplyFilter: () => void;
  onClearFilter: () => void;
  onSuggestionSelect: (suggestion: SearchSuggestion) => void;
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
        {autocompleteOpen && suggestions.length > 0 && (
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
            {suggestions.map((s) => (
              <PopupMenuItem
                key={suggestionKey(s)}
                onMouseDown={(e) => {
                  e.preventDefault();
                  onSuggestionSelect(s);
                  onAutocompleteOpenChange(false);
                  searchInputRef.current?.focus();
                }}
              >
                <span
                  style={{
                    display: "flex",
                    alignItems: "center",
                    gap: 6,
                    minWidth: 0,
                    width: "100%",
                  }}
                >
                  {/* Kind and count are plain opacity, never a new color — a hardcoded color
                      could not adapt across this app's five themes. */}
                  <span style={{ opacity: 0.55, fontSize: 11, flexShrink: 0 }}>
                    {t(KIND_LABEL_KEY[s.kind])}
                  </span>
                  <span
                    style={{
                      flex: 1,
                      minWidth: 0,
                      overflow: "hidden",
                      textOverflow: "ellipsis",
                      whiteSpace: "nowrap",
                      // Phrase rows are a literal filter rewrite; a monospace body makes the quotes
                      // and the `ns:"..."` spelling readable at a glance.
                      fontFamily: s.kind === "phrase" ? "monospace" : undefined,
                    }}
                  >
                    {s.label}
                  </span>
                  {s.kind !== "title" && s.count !== undefined && (
                    <span style={{ opacity: 0.55, fontSize: 11, flexShrink: 0 }}>
                      {/* Two different claims, so two different strings: a tag row's number is how
                          many archives carry that tag library-wide (what the tag cloud shows), a
                          phrase row's is how many results that rewrite yields under the *current*
                          filters. */}
                      {t(
                        s.kind === "tag"
                          ? "library.suggestionTagCount"
                          : "library.suggestionCount",
                        { n: s.count },
                      )}
                    </span>
                  )}
                </span>
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
