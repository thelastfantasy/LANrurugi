import { routes } from "../../routes"
import { ArchiveCard } from "./ArchiveCard"
import { ArchiveContextMenu, DeleteConfirmDialog } from "./ArchiveContextMenu"
import { CategoryBar } from "./CategoryBar"
import { CompactTable } from "./CompactTable"
import { RecentlyAddedCarousel } from "./RecentlyAddedCarousel"
import { ResultInfoAndPager } from "./ResultInfoAndPager"
import { SearchBar } from "./SearchBar"
import { SettingsMenu } from "./SettingsMenu"
import { SortBySelector } from "./SortBySelector"
import { useLibrary } from "./useLibrary"

export function Library() {
  const lib = useLibrary()
  const deleteTarget = lib.deleteTarget

  if (lib.search.isError) {
    return (
      <div className="ido" style={{ textAlign: "center", padding: 40 }}>
        <div id="json-error">
          <h1 style={{ color: "red" }}>
            <i className="fas fa-bomb" aria-hidden="true"></i>{" "}
            {lib.t("I don't know everything, but I sure as hell know this database's busted lads")}{" "}
            <i className="fas fa-bomb" aria-hidden="true"></i>
          </h1>
          <h2>{lib.t("The database cache is corrupt, and as such LANrarugi is unable to display your archive list.")}</h2>
        </div>
      </div>
    )
  }

  return (
    <div className="ido">
      <h1 className="ih">{lib.info.data?.motd}</h1>

      <div id="toppane">
        <div className="idi">
          <CategoryBar
            selectedCategory={lib.selectedCategory}
            sortedCategories={lib.sortedCategories}
            onToggleCategory={lib.toggleCategory}
          />
          <SearchBar
            filterInput={lib.filterInput}
            autocompleteOpen={lib.autocompleteOpen}
            tagSuggestions={lib.tagSuggestions}
            multiSelect={lib.multiSelect}
            searchInputRef={lib.searchInputRef}
            onFilterInputChange={(value, open) => {
              lib.setFilterInputOverride(value)
              if (open) lib.setAutocompleteOpen(true)
            }}
            onAutocompleteOpenChange={lib.setAutocompleteOpen}
            onApplyFilter={() => {
              lib.setFilterInputOverride(null)
              lib.navigateSearch({ appliedFilter: lib.filterInput, page: 0 })
            }}
            onClearFilter={() => {
              lib.setFilterInputOverride(null)
              lib.navigateSearch({ appliedFilter: "", page: 0 })
            }}
            onSuggestionSelect={(insertValue) => {
              const upToCursor = lib.filterInput.replace(/[^,\s-]*$/, "")
              lib.setFilterInputOverride(`${upToCursor}${insertValue}`)
              lib.searchInputRef.current?.focus()
            }}
            onToggleMultiSelect={() => void lib.handleToggleMultiSelect()}
          />
        </div>

        <RecentlyAddedCarousel
          filter={lib.appliedFilter}
          category={lib.selectedCategory}
          hideCompleted={lib.hideCompleted}
          groupbyTanks={lib.groupbyTanks}
          cropThumbs={lib.cropThumbs}
          onContextMenu={lib.handleContextMenu}
          onOpen={lib.handleOpenArchive}
          multiSelect={lib.multiSelect}
          selectedIds={lib.selectedIds}
          onToggleSelected={lib.toggleSelected}
          onReorderSelection={lib.setSelectedIds}
          onSelectPage={lib.selectAllOnPage}
          onClearSelection={lib.clearSelection}
          onRunBatch={lib.runBatchOnSelection}
          onMerge={() => void lib.mergeSelectionIntoTankoubon()}
          canMerge={lib.canMerge}
          onSearchTag={lib.applyTagSearch}
          refreshKey={lib.carouselRefreshKey}
        />
      </div>

      {lib.multiSelect && (
        <div style={{ textAlign: "center", padding: "8px 0" }}>
          <span style={{ marginRight: 12 }}>
            {lib.t("{{n}} selected", { n: lib.selectedIds.length })}
          </span>
          <button type="button" className="stdbtn" onClick={() => lib.selectAllOnPage()}>
            {lib.t("Select All on Page")}
          </button>
          <button type="button" className="stdbtn" onClick={lib.clearSelection}>
            {lib.t("Clear Selection")}
          </button>
          <button type="button" className="stdbtn" onClick={lib.runBatchOnSelection}>
            {lib.t("Batch Operations")}
          </button>
          {lib.canMerge && (
            <button type="button" className="stdbtn" onClick={() => void lib.mergeSelectionIntoTankoubon()}>
              {lib.t("Merge into Tankoubon")}
            </button>
          )}
        </div>
      )}

      <div className="table-options table-options-row">
        {lib.viewMode === "thumbnail" && (
          <SortBySelector
            sortby={lib.sortby}
            order={lib.order}
            stats={lib.stats.data}
            onSortBy={(key) => lib.navigateSearch({ sortby: key, page: 0 })}
            onToggleOrder={() => lib.navigateSearch({ order: lib.order === "asc" ? "desc" : "asc" })}
          />
        )}
        {lib.viewMode === "compact" && (
          <div className="compact-options">
            {lib.t("Columns:")}{" "}
            <select className="favtag-btn" value={lib.columns} onChange={(e) => lib.setColumns(Number(e.target.value))}>
              {Array.from({ length: 20 }, (_, i) => i + 1).map((n) => (
                <option key={n} value={n}>{n}</option>
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
            />
          </div>
        )}
        <div className="table-options-goto">
          <div style={{ display: "flex", alignItems: "center" }}>
            {lib.t("Go to Page:")}{" "}
            <select className="favtag-btn table-options-goto-select" style={{ marginTop: 6, marginBottom: 6 }} value={lib.page} onChange={(e) => lib.navigateSearch({ page: Number(e.target.value) })}>
              {Array.from({ length: lib.pageCount }, (_, i) => i).map((p) => (
                <option key={p} value={p}>{p + 1}</option>
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
        <p>{lib.t("Loading library…")}</p>
      ) : lib.shown.length === 0 ? (
        <div style={{ textAlign: "center", padding: 40 }}>
          <i className="fas fa-sad-cry fa-4x" aria-hidden="true"></i>
          <h1>
            {lib.t("No archives to show you! Try")}{" "}
            <a href={routes.upload()} onClick={(e) => { e.preventDefault(); lib.navigate(routes.upload()) }}>
              {lib.t("uploading some")}
            </a>
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
        <div id="thumbs_container" style={{ textAlign: "center" }}>
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

      {lib.contextMenu && (
        <ArchiveContextMenu
          state={lib.contextMenu}
          categories={lib.categories.data}
          loggedIn={lib.loggedIn}
          liveArchives={lib.shown}
          onClose={() => lib.setContextMenu(null)}
          onToggleCategory={(categoryId, archiveId, currentlyIn) =>
            void lib.toggleArchiveCategory(categoryId, archiveId, currentlyIn)}
          onDelete={(archiveId, isTank) => lib.setDeleteTarget({ id: archiveId, isTank })}
          onOpen={lib.handleOpenArchive}
          onRatingChange={(archiveId, isTank, rating) => void lib.updateRating(archiveId, isTank, rating)}
          onToggleSelection={(id) => {
            if (!lib.multiSelect) lib.handleToggleMultiSelect()
            lib.toggleSelected(id)
          }}
          isSelected={lib.selectedIds.includes(lib.contextMenu.archive.arcid)}
          onSetProgress={lib.handleSetProgress}
        />
      )}

      {deleteTarget && (
        <DeleteConfirmDialog
          isTank={deleteTarget.isTank}
          onCancel={() => lib.setDeleteTarget(null)}
          onConfirm={() => {
            void lib.deleteArchive(deleteTarget.id, deleteTarget.isTank)
            lib.setDeleteTarget(null)
          }}
        />
      )}
    </div>
  )
}
