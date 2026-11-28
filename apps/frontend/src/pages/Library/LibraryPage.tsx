import { lazy, Suspense, useEffect, useRef, useState } from "react";

import { ApiError } from "@/api/client";
import { useArchiveMetadata } from "@/api/hooks";
import { Link } from "@/components/common-ui/Link";
import { useLibrary } from "@/hooks/useLibrary";
import { sourceTagFrom } from "@/lib/deleteArchive";
import { routes } from "@/lib/routes";
import { isTankoubonId } from "@/lib/utils/isTankoubonId";

import { AiSmartTankoubonDrawer } from "./AiSmartTankoubonDrawer";
import { ArchiveCard } from "./ArchiveCard";
import { ArchiveContextMenu } from "./ArchiveContextMenu";
import { CategoryBar } from "./CategoryBar";
import { CompactTable } from "./CompactTable";
import { DeleteConfirmDialog } from "./DeleteConfirmDialog";
import { LibraryShortcutsOverlay } from "./LibraryShortcutsOverlay";
import { RecentlyAddedCarousel } from "./RecentlyAddedCarousel";
import { ResultInfoAndPager } from "./ResultInfoAndPager";
import { SearchBar } from "./SearchBar";
import { SelectionDrawer, SelectionFab } from "./SelectionDrawer";
import { SettingsMenu } from "./SettingsMenu";
import { SortBySelector } from "./SortBySelector";

// Loaded only when someone shares, keeping the share dialog and QR code out of the entry chunk.
const ShareDialog = lazy(() => import("@/share/ShareDialog").then((m) => ({ default: m.ShareDialog })));

/** The archive's `source:` tag, read from the already-loaded list rather than fetched again. Absent
 *  for a hand-uploaded file, which is how the delete dialog knows not to offer the subscription
 *  option at all. */
function sourceOf(
  shown: { arcid: string; tags?: string | null }[],
  id: string,
): string | undefined {
  return sourceTagFrom(shown.find((a) => a.arcid === id)?.tags)
}

export function Library() {
  const lib = useLibrary();
  const deleteTarget = lib.deleteTarget;
  // A selection outlives the page it was picked on, so the archive being deleted may not be among
  // the cards on screen; its own metadata then supplies the source the dialog checks subscriptions for.
  const shownSource = deleteTarget ? sourceOf(lib.shown, deleteTarget.id) : undefined;
  const deleteTargetMetadata = useArchiveMetadata(
    deleteTarget?.id ?? null,
    Boolean(deleteTarget) && !deleteTarget?.isTank && !lib.shown.some((a) => a.arcid === deleteTarget?.id),
  );
  const deleteTargetSource = shownSource ?? sourceTagFrom(deleteTargetMetadata.data?.tags);
  const [aiTankoubonDrawerOpen, setAiTankoubonDrawerOpen] = useState(false);
  const [shareIds, setShareIds] = useState<string[] | null>(null);
  // Selection mode starts minimized, so the grid is free for picking.
  const [selectionDrawerOpen, setSelectionDrawerOpen] = useState(false);
  const [shortcutsOpen, setShortcutsOpen] = useState(false);
  // The bottom pager scrolls back up to this bar, so a new page is read from its top.
  const optionsRef = useRef<HTMLDivElement>(null);

  // `?` opens the shortcut sheet everywhere except while typing in a field; the archive context
  // menu's own Escape handling stays in useLibrary.
  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      const target = e.target as HTMLElement | null;
      if (
        target &&
        (target.tagName === "INPUT" ||
          target.tagName === "TEXTAREA" ||
          target.tagName === "SELECT" ||
          target.isContentEditable)
      ) {
        return;
      }
      if (e.key === "?") {
        e.preventDefault();
        // The suggestion panel is a layer of its own; leaving it open under the sheet makes the
        // next Escape close the wrong thing.
        lib.setAutocompleteOpen(false);
        setShortcutsOpen((prev) => !prev);
        return;
      }
      if (e.key === "Escape" && shortcutsOpen) {
        setShortcutsOpen(false);
        return;
      }
      // Escape leaves selection mode whether or not the drawer is open (the drawer alone would only
      // minimize). A dialog or menu above it closes first: its own Escape is the one meant.
      if (e.key === "Escape" && lib.multiSelect && !lib.contextMenu && !lib.autocompleteOpen) {
        const dialogs = document.querySelectorAll('[role="dialog"], [role="alertdialog"]').length;
        if (dialogs > (selectionDrawerOpen ? 1 : 0)) return;
        e.preventDefault();
        e.stopPropagation();
        setSelectionDrawerOpen(false);
        void lib.handleToggleMultiSelect();
      }
    }
    window.addEventListener("keydown", onKeyDown, true);
    return () => window.removeEventListener("keydown", onKeyDown, true);
  }, [lib, shortcutsOpen, selectionDrawerOpen]);

  if (lib.search.isError) {
    // 401 means the session just expired mid-view; RequireAuth is already navigating to /login.
    if (lib.search.error instanceof ApiError && lib.search.error.status === 401)
      return null;

    return (
      <div className="ido" style={{ textAlign: "center", padding: 40 }}>
        <div id="json-error">
          <h1 style={{ color: "red" }}>
            <i className="fas fa-bomb" aria-hidden="true"></i>{" "}
            {lib.t(
              "I don't know everything, but I sure as hell know this database's busted lads",
            )}{" "}
            <i className="fas fa-bomb" aria-hidden="true"></i>
          </h1>
          <h2>
            {lib.t(
              "The database cache is corrupt, and as such LANrarugi is unable to display your archive list.",
            )}
          </h2>
        </div>
      </div>
    );
  }

  return (
    <div className="ido">
      <h1 className="ih">{lib.info.data?.name}</h1>

      <div id="toppane">
        <div className="idi">
          <CategoryBar
            selectedCategory={lib.selectedCategory}
            sortedCategories={lib.sortedCategories}
            hasTankoubons={(lib.tankoubons.data?.total ?? 0) > 0}
            onToggleCategory={lib.toggleCategory}
          />
          <SearchBar
            filterInput={lib.filterInput}
            autocompleteOpen={lib.autocompleteOpen}
            suggestions={lib.suggestions}
            recentSearches={lib.recentSearches}
            multiSelect={lib.multiSelect}
            canSelect={lib.search.isLoading || lib.shown.length > 0}
            searchInputRef={lib.searchInputRef}
            onFilterInputChange={(value, open) => {
              lib.setFilterInputOverride(value);
              if (open) lib.setAutocompleteOpen(true);
            }}
            onAutocompleteOpenChange={lib.setAutocompleteOpen}
            onApplyFilter={() => {
              lib.setFilterInputOverride(null);
              lib.navigateSearch({ appliedFilter: lib.filterInput, page: 0 });
            }}
            onClearFilter={() => {
              lib.setFilterInputOverride(null);
              lib.navigateSearch({ appliedFilter: "", page: 0 });
            }}
            onSuggestionSelect={lib.applySuggestion}
            onInsertToken={lib.insertToken}
            onToggleToken={lib.toggleToken}
            onPickRecent={lib.applyRecentFilter}
            onRemoveRecent={lib.removeRecentFilter}
            onToggleMultiSelect={() => {
              // Entering: bring the results into view, since that is where the picking happens.
              if (!lib.multiSelect) optionsRef.current?.scrollIntoView({ behavior: "smooth", block: "start" });
              void lib.handleToggleMultiSelect();
            }}
            onAiSmartTankoubon={() => setAiTankoubonDrawerOpen(true)}
            loggedIn={lib.loggedIn}
          />
        </div>

        <RecentlyAddedCarousel
          filter={lib.appliedFilter}
          category={lib.selectedCategory}
          hideCompleted={lib.hideCompleted}
          groupbyTanks={lib.groupbyTanks}
          cropThumbs={lib.cropThumbs}
          loggedIn={lib.loggedIn}
          onContextMenu={lib.handleContextMenu}
          onOpen={lib.handleOpenArchive}
          onSearchTag={lib.applyTagSearch}
        />
      </div>

      <div ref={optionsRef} className="table-options table-options-row">
        {lib.viewMode === "thumbnail" && (
          <SortBySelector
            sortby={lib.sortby}
            order={lib.order}
            stats={lib.stats.data}
            onSortBy={(key) => lib.navigateSearch({ sortby: key, page: 0 })}
            onToggleOrder={() =>
              lib.navigateSearch({
                order: lib.order === "asc" ? "desc" : "asc",
              })
            }
          />
        )}
        {lib.viewMode === "compact" && (
          <div className="compact-options">
            {lib.t("Columns:")}{" "}
            <select
              className="favtag-btn"
              value={lib.columns}
              onChange={(e) => lib.setColumns(Number(e.target.value))}
            >
              {Array.from({ length: 20 }, (_, i) => i + 1).map((n) => (
                <option key={n} value={n}>
                  {n}
                </option>
              ))}
            </select>
          </div>
        )}
        {lib.totalFiltered > 0 && (
          <div className="table-options-result-unit">
            <ResultInfoAndPager
              rangeStart={lib.rangeStart}
              rangeEnd={lib.rangeEnd}
              totalFiltered={lib.totalFiltered}
              totalRecords={lib.totalRecords}
              page={lib.page}
              pageCount={lib.pageCount}
              onPage={(p) => lib.navigateSearch({ page: p })}
              buildHref={(p) => `/?${lib.buildSearch({ page: p })}`}
            />
          </div>
        )}
        <div className="table-options-goto">
          <div style={{ display: "flex", alignItems: "center" }}>
            {lib.t("Go to Page:")}{" "}
            <select
              className="favtag-btn table-options-goto-select"
              style={{ marginTop: 6, marginBottom: 6 }}
              value={lib.page}
              onChange={(e) =>
                lib.navigateSearch({ page: Number(e.target.value) })
              }
            >
              {Array.from({ length: lib.pageCount }, (_, i) => i).map((p) => (
                <option key={p} value={p}>
                  {p + 1}
                </option>
              ))}
            </select>
          </div>
          <div style={{ display: "flex", alignItems: "center", gap: 4 }}>
            <SettingsMenu
              viewMode={lib.viewMode}
              setViewMode={lib.setViewMode}
              cropThumbs={lib.cropThumbs}
              setCropThumbs={lib.setCropThumbs}
              hideCompleted={lib.hideCompleted}
              setHideCompleted={lib.setHideCompleted}
              groupbyTanks={lib.groupbyTanks}
              setGroupbyTanks={lib.setGroupbyTanks}
            />
          </div>
        </div>
      </div>

      {lib.search.isLoading ? (
        <p>{lib.t("common.loadingLibrary")}</p>
      ) : lib.shown.length === 0 ? (
        <div style={{ textAlign: "center", padding: 40 }}>
          <i className="fas fa-sad-cry fa-4x" aria-hidden="true"></i>
          <h1>
            {lib.t("No archives to show you! Try")}{" "}
            <Link to={routes.upload()}>{lib.t("uploading some")}</Link>
            ?
          </h1>
        </div>
      ) : lib.viewMode === "compact" ? (
        <CompactTable
          shown={lib.shown}
          columns={lib.columns}
          selectedIds={lib.selectedIds}
          multiSelect={lib.multiSelect}
          sortby={lib.sortby}
          order={lib.order}
          onSort={(key) => lib.navigateSearch({ sortby: key, page: 0 })}
          onSearchTag={lib.applyTagSearch}
          onToggleSelected={lib.toggleSelected}
          onOpen={lib.handleOpenArchive}
          onContextMenu={lib.handleContextMenu}
        />
      ) : (
        <div id="thumbs_container" className="library-thumb-grid" style={{ textAlign: "center" }}>
          {lib.shown.map((a) => (
            <ArchiveCard
              key={a.arcid}
              archive={a}
              multiSelect={lib.multiSelect}
              selected={lib.selectedIds.includes(a.arcid)}
              cropThumbs={lib.cropThumbs}
              onToggleSelect={lib.toggleSelected}
              onContextMenu={lib.handleContextMenu}
              onOpen={lib.handleOpenArchive}
              onSearchTag={lib.applyTagSearch}
            />
          ))}
        </div>
      )}

      {lib.shown.length > 0 && lib.pageCount > 1 && (
        <div className="library-bottom-pager">
          <ResultInfoAndPager
            rangeStart={lib.rangeStart}
            rangeEnd={lib.rangeEnd}
            totalFiltered={lib.totalFiltered}
            totalRecords={lib.totalRecords}
            page={lib.page}
            pageCount={lib.pageCount}
            onPage={(p) => {
              lib.navigateSearch({ page: p });
              optionsRef.current?.scrollIntoView({ block: "start" });
            }}
            buildHref={(p) => `/?${lib.buildSearch({ page: p })}`}
          />
        </div>
      )}

      {lib.contextMenu && (
        <ArchiveContextMenu
          state={lib.contextMenu}
          categories={lib.categories.data}
          loggedIn={lib.loggedIn}
          liveArchives={lib.shown}
          onClose={() => lib.setContextMenu(null)}
          onToggleCategory={(categoryId, archiveId, currentlyIn) =>
            void lib.toggleArchiveCategory(categoryId, archiveId, currentlyIn)
          }
          onDelete={(archiveId, isTank) =>
            lib.setDeleteTarget({ id: archiveId, isTank })
          }
          onOpen={lib.handleOpenArchive}
          onRatingChange={(archiveId, isTank, rating) =>
            void lib.updateRating(archiveId, isTank, rating)
          }
          onToggleSelection={(id) => {
            if (!lib.multiSelect) lib.handleToggleMultiSelect();
            lib.toggleSelected(id);
          }}
          isSelected={lib.selectedIds.includes(lib.contextMenu.archive.arcid)}
          onShare={(id) =>
            setShareIds(
              lib.multiSelect && lib.selectedIds.includes(id) ? lib.selectedIds : [id],
            )
          }
          onSetProgress={lib.handleSetProgress}
        />
      )}

      {deleteTarget && (
        <DeleteConfirmDialog
          isTank={deleteTarget.isTank}
          source={deleteTargetSource}
          onCancel={() => lib.setDeleteTarget(null)}
          onConfirm={(blockResubscribe, tankMode) => {
            void lib.deleteArchive(
              deleteTarget.id,
              deleteTarget.isTank,
              blockResubscribe,
              tankMode,
              deleteTargetSource,
            );
            lib.setDeleteTarget(null);
          }}
        />
      )}

      {lib.multiSelect && (
        <>
          <SelectionDrawer
            open={selectionDrawerOpen}
            selectedIds={lib.selectedIds}
            filteredCount={lib.filteredCount}
            canMerge={lib.canMerge}
            onMinimize={() => setSelectionDrawerOpen(false)}
            onExit={() => {
              setSelectionDrawerOpen(false);
              void lib.handleToggleMultiSelect();
            }}
            onReorder={lib.setSelectedIds}
            onRemove={lib.toggleSelected}
            onSelectPage={() => lib.selectAllOnPage()}
            onSelectResults={lib.selectAllResults}
            selectingResults={lib.selectingAllResults}
            onSortSelection={(mode) => void lib.sortSelection(mode)}
            sortingSelection={lib.sortingSelection}
            onClear={lib.clearSelection}
            onBatchRemoved={(ids) => {
              const gone = new Set(ids);
              lib.setSelectedIds((prev) => prev.filter((id) => !gone.has(id)));
            }}
            onDeleteOne={(id) => lib.setDeleteTarget({ id, isTank: isTankoubonId(id) })}
            onShare={(ids) => {
              setSelectionDrawerOpen(false);
              setShareIds(ids);
            }}
            onMerge={() => void lib.mergeSelectionIntoTankoubon()}
          />
          {!selectionDrawerOpen && (
            <SelectionFab count={lib.selectedIds.length} onOpen={() => setSelectionDrawerOpen(true)} />
          )}
        </>
      )}

      <AiSmartTankoubonDrawer open={aiTankoubonDrawerOpen} onClose={() => setAiTankoubonDrawerOpen(false)} />

      {shortcutsOpen && <LibraryShortcutsOverlay onClose={() => setShortcutsOpen(false)} />}

      {shareIds && (
        <Suspense fallback={null}>
          <ShareDialog
            items={shareIds.map((id) =>
              isTankoubonId(id) ? { kind: "tankoubon", id } : { kind: "archive", id },
            )}
            onClose={() => setShareIds(null)}
          />
        </Suspense>
      )}
    </div>
  );
}
