import { useQueryClient } from "@tanstack/react-query";
import type { MouseEvent } from "react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useTranslation } from "react-i18next";
import { useLocation, useNavigate } from "react-router-dom";

import { fetchJson, sendJson } from "@/api/client";
import {
  useCategories,
  useCreateTankoubon,
  useFilterCounts,
  usePreferences,
  useSearch,
  useSearchSuggest,
  useServerInfo,
  useSetArchiveProgress,
  useSettings,
  useStats,
  useTankoubons,
  useUpdatePreferences,
} from "@/api/hooks";
import type { ArchiveMetadata } from "@/api/types";
import { confirmDialog, promptDialog } from "@/dialog";
import {
  NEW_ONLY,
  PAGE_SIZE,
  TANKOUBON_ONLY,
  UNTAGGED_ONLY,
} from "@/lib/constants";
import { routes } from "@/lib/routes";
import {
  phraseSuggestions,
  type SearchSuggestion,
  tagSuggestions,
  titleFilterFragment,
} from "@/lib/searchSuggestions";
import {
  COLUMN_COUNT_KEY,
  CROP_THUMBS_KEY,
  DEFAULT_COLUMN_COUNT,
  GROUP_TANKS_KEY,
  HIDE_COMPLETED_KEY,
  INDEX_ORDER_KEY,
  INDEX_SORT_KEY,
  INDEX_VIEW_MODE_KEY,
  MSM_SELECTION_KEY,
} from "@/lib/storageKeys";
import {
  buildTagList,
  splitTagsByNamespace,
} from "@/lib/tagFormat";
import { isTankoubonId } from "@/lib/utils/isTankoubonId";
import { sortCategories } from "@/lib/utils/sortCategories";
import { type ContextMenuState } from "@/pages/Library/types";
import { recordSearchNavigation } from "@/pages/Reader/crossArchiveNav";
import { useSession } from "@/session/SessionProvider";
import { toast } from "@/toast";

import { useDocumentTitle } from "./useDocumentTitle";

let defaultPasswordToastShownThisPageLoad = false;

export function useLibrary() {
  const { t } = useTranslation();
  const navigate = useNavigate();
  const location = useLocation();
  const info = useServerInfo();
  const categories = useCategories();
  const tankoubons = useTankoubons();
  const createTankoubon = useCreateTankoubon();
  const { isAuthenticated: loggedIn, usingDefaultPassword } = useSession();
  const preferences = usePreferences({ enabled: loggedIn });
  const updatePreferences = useUpdatePreferences();
  const settings = useSettings();
  const stats = useStats(2);
  const queryClient = useQueryClient();

  const urlParams = useMemo(
    () => new URLSearchParams(location.search),
    [location.search],
  );

  const [filterInputOverride, setFilterInputOverride] = useState<string | null>(
    null,
  );
  const appliedFilter = urlParams.get("q") ?? "";
  const filterInput = filterInputOverride ?? appliedFilter;
  useDocumentTitle(appliedFilter || undefined);

  function buildSearch(overrides: {
    page?: number;
    sortby?: string;
    order?: "asc" | "desc";
    appliedFilter?: string;
    selectedCategory?: string;
  }): string {
    const nextPage = overrides.page ?? page;
    const nextSortby = overrides.sortby ?? sortby;
    const nextOrder = overrides.order ?? order;
    const nextFilter = overrides.appliedFilter ?? appliedFilter;
    const nextCategory = overrides.selectedCategory ?? selectedCategory;
    const params = new URLSearchParams();
    if (nextPage !== 0) params.set("p", String(nextPage + 1));
    if (nextSortby !== "title") params.set("sort", nextSortby);
    if (nextOrder !== "asc") params.set("sortdir", nextOrder);
    if (nextFilter) params.set("q", nextFilter);
    if (nextCategory) params.set("c", nextCategory);
    return params.toString();
  }

  const selectedCategory = urlParams.get("c") ?? "";
  const [autocompleteOpen, setAutocompleteOpen] = useState(false);
  const sortby =
    urlParams.get("sort") ??
    preferences.data?.library_sortby ??
    localStorage.getItem(INDEX_SORT_KEY) ??
    "title";
  const order: "asc" | "desc" = (() => {
    const fromUrl = urlParams.get("sortdir");
    if (fromUrl === "asc" || fromUrl === "desc") return fromUrl;
    const fromPreferences = preferences.data?.library_sortdir;
    if (fromPreferences === "asc" || fromPreferences === "desc") {
      return fromPreferences;
    }
    return (
      (localStorage.getItem(INDEX_ORDER_KEY) as "asc" | "desc" | null) ?? "asc"
    );
  })();
  // Every field being changed must go through a single call, since `buildSearch()` reads
  // un-overridden fields from stale closure values — two separate calls would clobber each other.
  function navigateSearch(overrides: {
    page?: number;
    sortby?: string;
    order?: "asc" | "desc";
    appliedFilter?: string;
    selectedCategory?: string;
  }) {
    const nextSortby = overrides.sortby ?? sortby;
    const nextOrder = overrides.order ?? order;
    if (overrides.sortby !== undefined) {
      localStorage.setItem(INDEX_SORT_KEY, overrides.sortby);
    }
    if (overrides.order !== undefined) {
      localStorage.setItem(INDEX_ORDER_KEY, overrides.order);
    }
    if (
      loggedIn &&
      (overrides.sortby !== undefined || overrides.order !== undefined)
    ) {
      updatePreferences.mutate({
        library_sortby: nextSortby,
        library_sortdir: nextOrder,
      });
    }
    navigate({ search: buildSearch(overrides) });
  }
  const [viewMode, setViewModeState] = useState<"thumbnail" | "compact">(() =>
    localStorage.getItem(INDEX_VIEW_MODE_KEY) === "0" ? "compact" : "thumbnail",
  );
  function setViewMode(v: "thumbnail" | "compact") {
    setViewModeState(v);
    localStorage.setItem(INDEX_VIEW_MODE_KEY, v === "compact" ? "0" : "1");
  }
  const [cropThumbs, setCropThumbsState] = useState(
    () => localStorage.getItem(CROP_THUMBS_KEY) !== "false",
  );
  function setCropThumbs(v: boolean) {
    setCropThumbsState(v);
    localStorage.setItem(CROP_THUMBS_KEY, String(v));
  }
  const [hideCompleted, setHideCompletedState] = useState(
    () => localStorage.getItem(HIDE_COMPLETED_KEY) === "true",
  );
  function setHideCompleted(v: boolean) {
    setHideCompletedState(v);
    localStorage.setItem(HIDE_COMPLETED_KEY, String(v));
  }
  const [groupbyTanks, setGroupbyTanksState] = useState(
    () => localStorage.getItem(GROUP_TANKS_KEY) !== "false",
  );
  function setGroupbyTanks(v: boolean) {
    setGroupbyTanksState(v);
    localStorage.setItem(GROUP_TANKS_KEY, String(v));
  }
  const [columns, setColumnsState] = useState(() => {
    const stored = localStorage.getItem(COLUMN_COUNT_KEY);
    const parsed = stored ? Number.parseInt(stored, 10) : NaN;
    return Number.isFinite(parsed) && parsed > 0
      ? parsed
      : DEFAULT_COLUMN_COUNT;
  });
  const setColumns = (value: number) => {
    setColumnsState(value);
    localStorage.setItem(COLUMN_COUNT_KEY, String(value));
  };
  const page = (() => {
    const fromUrl = Number(urlParams.get("p"));
    return Number.isInteger(fromUrl) && fromUrl > 0 ? fromUrl - 1 : 0;
  })();
  const [multiSelect, setMultiSelect] = useState(false);
  const [selectedIds, setSelectedIds] = useState<string[]>([]);
  const [contextMenu, setContextMenu] = useState<ContextMenuState | null>(null);
  const [deleteTarget, setDeleteTarget] = useState<{
    id: string;
    isTank: boolean;
  } | null>(null);
  const searchInputRef = useRef<HTMLInputElement>(null);
  const setArchiveProgress = useSetArchiveProgress();
  function handleSetProgress(archiveId: string, page: number) {
    setArchiveProgress.mutate({ id: archiveId, page });
  }

  useEffect(() => {
    if (
      usingDefaultPassword &&
      !defaultPasswordToastShownThisPageLoad
    ) {
      defaultPasswordToastShownThisPageLoad = true;
      toast({
        heading: t("hooks.youReUsingTheDefault") ?? undefined,
        text: t("hooks.loginWithPasswordKamimamitaAnd") ?? undefined,
        icon: "warning",
        hideAfter: 25000,
        closeOnClick: false,
        draggable: false,
      });
    }
  }, [usingDefaultPassword, t]);

  useEffect(() => {
    const seenKey = "seenContextMenuTutorial";
    if (localStorage.getItem(seenKey)) return;
    localStorage.setItem(seenKey, "1");
    toast({
      heading: t("hooks.tipRightclickAnArchiveFor") ?? undefined,
      icon: "info",
      hideAfter: 8000,
    });
  }, [t]);

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === "/" && (e.target as HTMLElement)?.tagName !== "INPUT") {
        e.preventDefault();
        searchInputRef.current?.focus();
      }
      if (e.key === "Escape") {
        setContextMenu(null);
      }
    }
    document.addEventListener("keydown", onKeyDown);
    return () => document.removeEventListener("keydown", onKeyDown);
  }, []);

  const isBuiltinSelector =
    selectedCategory === NEW_ONLY ||
    selectedCategory === UNTAGGED_ONLY ||
    selectedCategory === TANKOUBON_ONLY;
  /** The filters every search-shaped request shares — the applied search *and* the autocomplete's
   *  own hit counts, so a candidate's stated count is the number the page will actually show. */
  const searchContext = {
    category:
      !isBuiltinSelector && selectedCategory ? selectedCategory : undefined,
    newonly: selectedCategory === NEW_ONLY,
    untaggedonly: selectedCategory === UNTAGGED_ONLY,
    tankonly: selectedCategory === TANKOUBON_ONLY,
    hidecompleted: hideCompleted,
    groupbyTanks,
  };
  const search = useSearch({
    filter: appliedFilter,
    sortby,
    order,
    start: page * PAGE_SIZE,
    ...searchContext,
  });

  const shown = search.data?.data ?? [];
  const totalFiltered = search.data?.recordsFiltered ?? 0;
  const totalRecords = search.data?.recordsTotal ?? 0;
  const pageCount = Math.max(1, Math.ceil(totalFiltered / PAGE_SIZE));
  const rangeStart = totalFiltered === 0 ? 0 : page * PAGE_SIZE + 1;
  const rangeEnd = Math.min(totalFiltered, page * PAGE_SIZE + PAGE_SIZE);

  const sortedCategories = useMemo(
    () => sortCategories(categories.data ?? []),
    [categories.data],
  );

  // Title and phrase suggestions both need the backend for something (a title lookup, a candidate's
  // hit count), so they read a debounced copy of the input — the same 250ms pattern the tankoubon
  // editor's archive search already uses (`TankoubonEdit.tsx`). Tag completion stays instant: it's
  // computed locally off the already-fetched tag stats.
  const [debouncedFilter, setDebouncedFilter] = useState("");
  useEffect(() => {
    const timeout = setTimeout(() => setDebouncedFilter(filterInput), 250);
    return () => clearTimeout(timeout);
  }, [filterInput]);

  const titleFragment = titleFilterFragment(debouncedFilter);
  const titleSuggest = useSearchSuggest(
    { filter: titleFragment ?? "", ...searchContext },
    6,
  );
  const phraseRows = useMemo(
    () => phraseSuggestions(debouncedFilter),
    [debouncedFilter],
  );
  const phraseCounts = useFilterCounts(
    phraseRows.map((p) => p.insertValue),
    searchContext,
  );

  /** Merged autocomplete list: existing tag completions first (unchanged, so nothing a user
   *  already relies on moves), then real archive titles, then the quoted-phrase rewrites for the
   *  bare runs in what they typed — see `lib/searchSuggestions.ts` for why those are only ever
   *  offered as rows and never applied on their own. */
  const suggestions: SearchSuggestion[] = useMemo(() => {
    const tags = tagSuggestions(filterInput, stats.data ?? [], 8);
    const titles: SearchSuggestion[] = (titleSuggest.data?.data ?? []).map((entry) => ({
      kind: "title",
      label: entry.title,
      arcid: entry.arcid,
    }));
    const phrases: SearchSuggestion[] = phraseRows.map((row) => ({
      ...row,
      count: phraseCounts.data?.[row.insertValue],
    }));
    return [...tags, ...titles, ...phrases];
  }, [filterInput, stats.data, titleSuggest.data, phraseRows, phraseCounts.data]);

  /** Applies one autocomplete row. A title row is an archive the input already matches, so it
   *  opens that archive instead of editing the filter; every other row replaces exactly the span it
   *  was built from, leaving the rest of the query (an already-quoted tag, a negation, another
   *  span's wording) untouched. Never applied automatically — only from a picked row. */
  function applySuggestion(suggestion: SearchSuggestion) {
    if (suggestion.kind === "title") {
      setAutocompleteOpen(false);
      navigate(routes.reader(suggestion.arcid));
      return;
    }
    // A phrase row is built from the *debounced* input (its hit count needs a request), so its
    // range can be a keystroke or two behind what is on screen — recompute it against the live
    // input rather than splicing at a stale offset. Matching on `insertValue` keeps this to the
    // same row the user actually clicked.
    const target =
      suggestion.kind === "phrase"
        ? (phraseSuggestions(filterInput).find(
            (row) => row.insertValue === suggestion.insertValue,
          ) ?? suggestion)
        : suggestion;
    const { start, end } = target.replaceRange;
    setFilterInputOverride(
      `${filterInput.slice(0, start)}${target.insertValue}${filterInput.slice(end)}`,
    );
    setAutocompleteOpen(false);
    searchInputRef.current?.focus();
  }

  function toggleCategory(id: string) {
    navigateSearch({
      selectedCategory: selectedCategory === id ? "" : id,
      page: 0,
    });
  }

  function toggleSelected(id: string) {
    setSelectedIds((prev) =>
      prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id],
    );
  }

  function selectAllOnPage() {
    setSelectedIds((prev) => {
      const additions = shown
        .map((a) => a.arcid)
        .filter((id) => !prev.includes(id));
      return [...prev, ...additions];
    });
  }

  function clearSelection() {
    setSelectedIds([]);
  }

  async function handleToggleMultiSelect() {
    if (multiSelect && selectedIds.length > 0) {
      if (!(await confirmDialog(t("hooks.youHaveAnActiveSelection") ?? ""))) {
        return;
      }
    }
    setMultiSelect((v) => !v);
    clearSelection();
  }

  function runBatchOnSelection() {
    if (selectedIds.length === 0) return;
    // `/batch` reads (and clears) the selection from this same localStorage key on load.
    localStorage.setItem(MSM_SELECTION_KEY, JSON.stringify(selectedIds));
    window.open("/batch", "_blank");
  }

  const selectedTankIds = selectedIds.filter(isTankoubonId);
  const canMerge = selectedTankIds.length < 2 && selectedIds.length > 0;

  async function mergeSelectionIntoTankoubon() {
    if (!canMerge) return;
    try {
      if (selectedTankIds.length === 1) {
        const targetTank = selectedTankIds[0];
        const archiveIds = selectedIds.filter((id) => id !== targetTank);
        const existing = tankoubons.data?.result.find(
          (tk) => tk.id === targetTank,
        );
        const merged = [...(existing?.archives ?? []), ...archiveIds];
        await fetch(`/api/tankoubons/${targetTank}`, {
          method: "PUT",
          headers: { "Content-Type": "application/json" },
          body: JSON.stringify({ archives: merged }),
        });
        clearSelection();
        navigate(routes.tankoubonEdit(targetTank));
        return;
      }
      const name = await promptDialog(t("hooks.enterANameForThe") ?? "");
      if (!name?.trim()) return;
      const result = await createTankoubon.mutateAsync(name.trim());
      await fetch(`/api/tankoubons/${result.tankoubon_id}`, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ archives: selectedIds }),
      });
      clearSelection();
      navigate(routes.tankoubonEdit(result.tankoubon_id));
    } catch {
      toast({
        heading: t("hooks.errorCreatingTankoubon") ?? undefined,
        icon: "error",
      });
    }
  }

  async function toggleArchiveCategory(
    categoryId: string,
    archiveId: string,
    currentlyIn: boolean,
  ) {
    await fetch(`/api/categories/${categoryId}/${archiveId}`, {
      method: currentlyIn ? "DELETE" : "PUT",
    });
    await categories.refetch();
  }

  async function updateRating(
    archiveId: string,
    isTank: boolean,
    rating: string | null,
  ) {
    const endpoint = isTank
      ? `/api/tankoubons/${archiveId}`
      : `/api/archives/${archiveId}/metadata`;
    const current = shown.find((a) => a.arcid === archiveId);
    const tagsByNamespace = splitTagsByNamespace(current?.tags ?? "");
    if (rating === null) delete tagsByNamespace.rating;
    else tagsByNamespace.rating = [rating];
    const newTags = buildTagList(tagsByNamespace).join(", ");
    if (isTank) {
      // `PUT /api/tankoubons/{id}` expects `tags` nested under `metadata` — a bare top-level
      // `{ tags }` deserializes as valid with `metadata: None` and silently no-ops instead of erroring.
      await fetch(endpoint, {
        method: "PUT",
        headers: { "Content-Type": "application/json" },
        body: JSON.stringify({ metadata: { tags: newTags } }),
      });
    } else {
      await sendJson(
        "PUT",
        `/archives/${archiveId}/metadata?tags=${encodeURIComponent(newTags)}`,
      );
    }
    queryClient.invalidateQueries({ queryKey: ["archive", archiveId] });
    queryClient.invalidateQueries({ queryKey: ["archives"] });
    queryClient.invalidateQueries({
      predicate: (query) => query.queryKey[0] === "search",
    });
  }

  /** This archive's `source:` tag, from whatever the page already holds. */
  function sourceTagOf(archiveId: string): string | undefined {
    const tags = shown.find((a) => a.arcid === archiveId)?.tags;
    if (!tags) return undefined;
    for (const tag of tags.split(",")) {
      const value = tag.trim();
      if (value.toLowerCase().startsWith("source:")) {
        return value.slice("source:".length).trim();
      }
    }
    return undefined;
  }

  /// `blockResubscribe` defaults to blocking, which is also what doing nothing achieves: a work stays
  /// recorded as seen, so no subscription fetches it again. Only the *opposite* choice needs an
  /// action — telling the subscriptions to reconsider it.
  async function deleteArchive(
    archiveId: string,
    isTank: boolean,
    blockResubscribe = true,
  ) {
    // Read before the delete, since the archive's own tags are what name the source.
    const source = !isTank && !blockResubscribe ? sourceTagOf(archiveId) : undefined;

    if (isTank) {
      await fetch(`/api/tankoubons/${archiveId}`, { method: "DELETE" });
      await tankoubons.refetch();
    } else {
      await fetch(`/api/archives/${archiveId}`, { method: "DELETE" });
    }

    if (source) {
      // After the delete, and failures are swallowed: the archive is already gone, and a failure here
      // only means a subscription will not re-offer the work — not something to surface as a failed
      // delete.
      try {
        const { tracking } = await fetchJson<{ tracking: { id: string }[] }>(
          `/subscriptions/tracking?source=${encodeURIComponent(source)}`,
        );
        await Promise.all(
          tracking.map((s) =>
            sendJson("POST", `/subscriptions/${encodeURIComponent(s.id)}/forget`, {
              sources: [source],
              all: false,
            }),
          ),
        );
      } catch {
        // Nothing to do — the work simply stays recorded as seen.
      }
    }
    queryClient.invalidateQueries({ queryKey: ["archive", archiveId] });
    queryClient.invalidateQueries({ queryKey: ["archives"] });
    await queryClient.invalidateQueries({
      predicate: (query) => query.queryKey[0] === "search",
    });
  }

  function handleContextMenu(
    e: MouseEvent,
    archive: ArchiveMetadata,
    source: "grid" | "carousel" = "grid",
  ) {
    e.preventDefault();
    // Document-relative (clientX/Y + scroll offset), not viewport-relative, so the menu scrolls
    // with the page (paired with `ArchiveContextMenu.tsx`'s `position: "absolute"`).
    setContextMenu({
      archive,
      x: e.clientX + window.scrollX,
      y: e.clientY + window.scrollY,
      source,
    });
  }

  function applyTagSearch(namespacedTag: string) {
    setFilterInputOverride(null);
    navigateSearch({ appliedFilter: namespacedTag, page: 0 });
  }

  function handleOpenArchive(id: string) {
    recordSearchNavigation(
      shown.map((a) => a.arcid),
      page + 1,
      {
        filter: appliedFilter,
        category: selectedCategory,
        sortby,
        order,
        pageSize: PAGE_SIZE,
        groupbyTanks,
        hidecompleted: hideCompleted,
      },
    );
    navigate(routes.reader(id));
  }

  return {
    t,
    navigate,
    location,
    info,
    categories,
    tankoubons,
    createTankoubon,
    settings,
    stats,
    loggedIn,
    appliedFilter,
    selectedCategory,
    sortby,
    order,
    page,
    filterInput,
    filterInputOverride,
    setFilterInputOverride,
    autocompleteOpen,
    setAutocompleteOpen,
    navigateSearch,
    buildSearch,
    viewMode,
    setViewMode,
    cropThumbs,
    setCropThumbs,
    hideCompleted,
    setHideCompleted,
    groupbyTanks,
    setGroupbyTanks,
    columns,
    setColumns,
    multiSelect,
    search,
    shown,
    totalFiltered,
    totalRecords,
    pageCount,
    rangeStart,
    rangeEnd,
    selectedIds,
    setSelectedIds,
    toggleSelected,
    selectAllOnPage,
    clearSelection,
    handleToggleMultiSelect,
    sortedCategories,
    suggestions,
    applySuggestion,
    contextMenu,
    setContextMenu,
    deleteTarget,
    setDeleteTarget,
    handleSetProgress,
    toggleCategory,
    toggleArchiveCategory,
    updateRating,
    deleteArchive,
    handleContextMenu,
    applyTagSearch,
    handleOpenArchive,
    selectedTankIds,
    canMerge,
    runBatchOnSelection,
    mergeSelectionIntoTankoubon,
    searchInputRef,
    setArchiveProgress,
    routes,
  };
}
