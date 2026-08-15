import { useMutation, useQueries, useQuery, useQueryClient } from "@tanstack/react-query"
import { useEffect } from "react"

import { ApiError, fetchJson, fetchText, sendForm, sendJson, sendJsonForBlob } from "./client"
import type {
  AddToQueueItem,
  AddToQueueResponse,
  ApiToken,
  ApiTokenCreateResponse,
  ArchiveFilesResponse,
  ArchiveMetadata,
  BookmarkLinkResponse,
  CategoryMetadata,
  ComparisonResult,
  DownloadQueueItem,
  DownloadQueueListResponse,
  DuplicateGroup,
  ExportPatchInsertion,
  JobRecord,
  JobsResponse,
  LoginStatus,
  PageDimensionsResponse,
  PluginInfo,
  PluginOptions,
  PluginOptionsUpdate,
  PluginSettings,
  PluginSettingsUpdate,
  RandomArchivesResponse,
  SearchResponse,
  ServerInfo,
  Settings,
  ShinobuStatus,
  StampedPagesResponse,
  StampsByPageResponse,
  StatTag,
  TankoubonFullResponse,
  TankoubonListResponse,
  TankoubonMetadata,
  TokenRole,
  UpdateQueueItemBody,
} from "./types"

/** Standard polling frequency for anything that needs "close to live" freshness without a
 * push/SSE mechanism (Shinobu status, log tail) — shared so both agree on the same cadence rather
 * than each hardcoding its own copy of "5 seconds". */
const POLL_INTERVAL_MS = 5000
/** `useJobs()` and the Upload page's download queue both poll faster than the shared default
 * above — an in-progress download's byte progress/speed is the one thing on this whole page a
 * user is likely to be actively watching tick up in real time, so it gets its own, snappier
 * cadence rather than sharing the "good enough for a background status indicator" one. */
const DOWNLOAD_QUEUE_POLL_INTERVAL_MS = 1000
const UPDATE_CHECK_STALE_TIME_MS = 60 * 60 * 1000

export function useArchives() {
  return useQuery({
    queryKey: ["archives"],
    queryFn: () => fetchJson<ArchiveMetadata[]>("/archives"),
  })
}

export function useCategories() {
  return useQuery({
    queryKey: ["categories"],
    queryFn: () => fetchJson<CategoryMetadata[]>("/categories"),
  })
}

/** Legacy's own `Category.addNewCategory(isDynamic)` (`category.js`) — a static category has an
 * empty `search`; a dynamic (smart) one stores its filter expression there instead. Only the
 * Categories management page had this (`Categories.tsx`'s own `handleNewCategory`, ported first);
 * this is the same `PUT /categories` call factored out so the "添加到:" dropdown elsewhere
 * (`ArchiveOverviewOverlay.tsx`, `Upload/index.tsx`) can offer it inline too, without a detour to
 * that separate page (issue #42 — legacy itself never offered this shortcut either; verified
 * against `reader.html.tt2`/`index_contextmenu.js`, whose own "添加到:" `#add-category` button
 * only ever adds the *current* archive to an already-selected category, not creates a new one). */
export function useCreateCategory() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (params: { name: string; isDynamic: boolean; search?: string }) =>
      sendForm<{ category_id: string }>("PUT", "/categories", {
        name: params.name,
        // Falls back to legacy's own "bogus search" placeholder (`category.js`'s
        // `addNewCategory`) when the caller doesn't supply a real predicate up front.
        search: params.isDynamic ? (params.search?.trim() || "language:english") : "",
      }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["categories"] }),
  })
}

export function useTankoubons() {
  return useQuery({
    queryKey: ["tankoubons"],
    queryFn: () => fetchJson<TankoubonListResponse>("/tankoubons?page=-1"),
  })
}

export function useSettings() {
  return useQuery({
    queryKey: ["settings"],
    queryFn: () => fetchJson<Settings>("/settings"),
  })
}

/** Unauthenticated equivalent of `useSettings().data?.theme` — the Login page renders before any
 * session exists, so the auth-gated `/settings` 401s there and `useApplyTheme` would otherwise
 * always fall back to the hardcoded default theme regardless of what's actually saved. Backed by
 * `GET /theme`, a separate public endpoint (see `lanrurugi_api::settings::public_router`). */
export function usePublicTheme(options?: { enabled?: boolean }) {
  return useQuery({
    queryKey: ["theme"],
    queryFn: () => fetchJson<{ theme: string }>("/theme"),
    enabled: options?.enabled,
  })
}

export function useUpdateSettings() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (partial: Partial<Settings>) => sendJson("PUT", "/settings", partial),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["settings"] }),
  })
}

export function usePlugins(kind: string = "all") {
  return useQuery({
    queryKey: ["plugins", kind],
    queryFn: () => fetchJson<PluginInfo[]>(`/plugins/${kind}`),
  })
}

/** Persists a drag-and-drop reorder of one plugin `type` group (`POST /plugins/priority`) —
 * invalidates every cached `usePlugins(...)` variant (`'all'`, the reordered type, and any other
 * already-fetched type) since `'all'`'s own cached list also needs the new order reflected. */
export function useReorderPlugins() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: { type: PluginInfo["type"]; order: string[] }) =>
      sendJson("POST", "/plugins/priority", body),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["plugins"] }),
  })
}

/** `null` (not an error state) when the plugin declares no `pluginOptions()` at all (spec FR-015
 * — a `404` from the endpoint means exactly this, not a real failure) — callers use this to decide
 * whether to render a settings affordance for a given download plugin at all. Pass `''` for a
 * non-download plugin to skip the request entirely (`enabled: false`) rather than firing a
 * guaranteed-404 call with an empty namespace. */
export function usePluginOptions(namespace: string) {
  return useQuery({
    queryKey: ["plugin-options", namespace],
    enabled: namespace !== "",
    queryFn: async () => {
      try {
        return await fetchJson<PluginOptions>(`/plugins/options?namespace=${encodeURIComponent(namespace)}`)
      } catch (e) {
        if (e instanceof ApiError && e.status === 404) return null
        throw e
      }
    },
  })
}

export function useUpdatePluginOptions(namespace: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (update: PluginOptionsUpdate) =>
      sendJson<PluginOptions>("PUT", `/plugins/options?namespace=${encodeURIComponent(namespace)}`, update),
    onSuccess: (data) => queryClient.setQueryData(["plugin-options", namespace], data),
  })
}

export function useResetPluginOptions(namespace: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () => sendJson<PluginOptions>("DELETE", `/plugins/options?namespace=${encodeURIComponent(namespace)}`),
    onSuccess: (data) => queryClient.setQueryData(["plugin-options", namespace], data),
  })
}

/** A plugin's own persisted custom-parameter values (e.g. E-Hentai login's cookie fields) — see
 * `PluginSettings`'s own docs. Distinct from `usePluginOptions` above (download-specific
 * concurrency/rate-limit/bundling settings). */
export function usePluginSettings(namespace: string) {
  return useQuery({
    queryKey: ["plugin-settings", namespace],
    queryFn: () => fetchJson<PluginSettings>(`/plugins/settings?namespace=${encodeURIComponent(namespace)}`),
  })
}

export function useUpdatePluginSettings(namespace: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (update: PluginSettingsUpdate) =>
      sendJson<void>("PUT", `/plugins/settings?namespace=${encodeURIComponent(namespace)}`, update),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["plugin-settings", namespace] }),
  })
}

export function useStats(minWeight = 1) {
  return useQuery({
    queryKey: ["stats", minWeight],
    queryFn: () =>
      fetchJson<StatTag[]>(
        `/database/stats?minweight=${minWeight}&hide_excluded_namespaces=true`,
      ),
  })
}

export function useArchiveMetadata(id: string | null) {
  return useQuery({
    queryKey: ["archive", id],
    queryFn: () => fetchJson<ArchiveMetadata>(`/archives/${id}/metadata`),
    enabled: id !== null,
  })
}

export function useArchivePages(id: string | null) {
  return useQuery({
    queryKey: ["archive-pages", id],
    queryFn: () => fetchJson<ArchiveFilesResponse>(`/archives/${id}/files`),
    enabled: id !== null,
  })
}

/** Only ever called from the infinite-scroll reader view, and only for `count` = however many
 * pages precede wherever tracked progress is about to resume-scroll to — see the backend's own
 * `read_page_dimensions` docs for why this is a bounded count, not "every page." `enabled` is the
 * caller's job (gate it on infinite-scroll actually being on and a real target being known) rather
 * than baked in here, same pattern as `useArchiveMetadata`'s own `id !== null` gate. */
export function usePageDimensions(id: string | null, count: number, enabled: boolean) {
  return useQuery({
    queryKey: ["archive-page-dimensions", id, count],
    queryFn: () => fetchJson<PageDimensionsResponse>(`/archives/${id}/page-dimensions?count=${count}`),
    enabled: enabled && id !== null && count > 0,
  })
}

export function useUpdateProgress(id: string | null) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (page: number) => sendJson("PUT", `/archives/${id}/progress/${page}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["archive", id] })
      queryClient.invalidateQueries({ queryKey: ["archives"] })
    },
  })
}

/** Same endpoint as `useUpdateProgress`, but not bound to one archive at mount time — used by the
 * Library grid's "Mark as Read"/"Mark as Unread" context-menu item, which operates on whichever
 * archive was right-clicked rather than a single archive the whole component is scoped to.
 *
 * Also invalidates every `['search', ...]` query, not just `['archives']` — the Library page's
 * own main grid and "On Deck"/etc. filters go through `useSearch` (query key `['search',
 * options]`, one distinct key per options object), not `useArchives`'s plain `['archives']`; a
 * real, live-confirmed bug (this mutation originally only invalidated `['archives']`, matching
 * `useUpdateProgress`'s own pre-existing pattern above — but nothing in the Library page actually
 * queries under that key, so the grid's displayed progress silently kept showing the pre-mutation
 * value until an unrelated refetch happened to occur). A `predicate` matching on the query key's
 * first element is needed since the second element (the full `SearchOptions` object) varies per
 * distinct search/filter/sort combination and there's no single exact key to invalidate. */
export function useSetArchiveProgress() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, page }: { id: string; page: number }) => sendJson("PUT", `/archives/${id}/progress/${page}`),
    onSuccess: (_data, { id }) => {
      queryClient.invalidateQueries({ queryKey: ["archive", id] })
      queryClient.invalidateQueries({ queryKey: ["archives"] })
      queryClient.invalidateQueries({ predicate: (query) => query.queryKey[0] === "search" })
    },
  })
}

export interface SearchOptions {
  filter?: string
  category?: string
  /** `"title"` (default), `"lastread"`, or any tag namespace (e.g. `"date_added"`) — matches
   * `lanrurugi-search::engine::sort_ids`'s own three branches: an indexed sort for `"title"`, a
   * per-archive-field scan for `"lastread"`, and a generic "sort by this tag namespace's value"
   * fallback for everything else, which is what makes `"date_added"` (the other option legacy's
   * own index page dropdown offers) work with no dedicated backend support of its own. */
  sortby?: string
  order?: "asc" | "desc"
  /** Pagination cursor — the *index* into the filtered+sorted result set, not a page number
   * (`lanrurugi-api::search`'s own fixed 100-per-page `PAGE_SIZE`). */
  start?: number
  /** The two hardcoded quick-filter categories (`NEW_ONLY`/`UNTAGGED_ONLY` in legacy's own
   * `index.js`) are intercepted client-side before ever reaching `category`, and become these
   * two flags instead — matches `LANraragi::Controller::Api::Search::handle_databases`. */
  newonly?: boolean
  untaggedonly?: boolean
  tankonly?: boolean
  /** Index-settings-menu toggles (`localStorage.hidecompleted`/`grouptanks` in legacy), sent on
   * every search so both the main grid and (if built) the carousel stay in sync with them. */
  hidecompleted?: boolean
  groupbyTanks?: boolean
  /** Defaults to `true`. Set `false` to skip firing the request entirely — e.g. a live
   * search-as-you-type dropdown (`TankoubonEdit.tsx`'s archive picker) that shouldn't query the
   * whole library while its input is empty. */
  enabled?: boolean
}

export function useSearch(options: SearchOptions) {
  const params = new URLSearchParams()
  if (options.filter) params.set("filter", options.filter)
  if (options.category) params.set("category", options.category)
  if (options.sortby) params.set("sortby", options.sortby)
  if (options.order) params.set("order", options.order)
  if (options.start !== undefined) params.set("start", String(options.start))
  if (options.newonly) params.set("newonly", "true")
  if (options.untaggedonly) params.set("untaggedonly", "true")
  if (options.tankonly) params.set("tankonly", "true")
  if (options.hidecompleted) params.set("hidecompleted", "true")
  if (options.groupbyTanks === false) params.set("groupby_tanks", "false")
  return useQuery({
    queryKey: ["search", options],
    queryFn: () => fetchJson<SearchResponse>(`/search?${params.toString()}`),
    enabled: options.enabled ?? true,
  })
}

export function useUpdateArchiveMetadata(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (fields: { title?: string; summary?: string; tags?: string }) => {
      const query = new URLSearchParams(
        Object.entries(fields).filter(([, v]) => v !== undefined) as [string, string][],
      )
      return sendJson("PUT", `/archives/${id}/metadata?${query}`)
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["archive", id] })
      queryClient.invalidateQueries({ queryKey: ["archives"] })
      queryClient.invalidateQueries({ queryKey: ["search"] })
    },
  })
}

/** Edit page's filename-rename affordance (additive, no legacy equivalent — see
 * `archives.rs::rename_archive`'s own docs). `stem` is the desired basename *without* its
 * extension — the backend always keeps the archive's existing extension and also renames the
 * sidecar `.patch.zip`, if any, alongside it. */
export function useRenameArchive(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (stem: string) =>
      sendJson<{ filename: string }>("PUT", `/archives/${id}/rename`, { stem }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["archive", id] })
      queryClient.invalidateQueries({ queryKey: ["archives"] })
      queryClient.invalidateQueries({ queryKey: ["search"] })
    },
  })
}

/** Reader overview overlay's "set as thumbnail" hover icon (legacy `.set-thumbnail`) — regenerates
 * the archive's cover thumbnail from the given page (`PUT /archives/{id}/thumbnail?page=N`). */
export function useSetArchiveThumbnail(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (page: number) => sendJson("PUT", `/archives/${id}/thumbnail?page=${page}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["archive", id] })
      queryClient.invalidateQueries({ queryKey: ["archives"] })
    },
  })
}

/** Deletes the sidecar `.patch.zip` for an archive and clears its `has_patch` flag —
 * `DELETE /archives/{id}/patch`. */
export function useDeletePatch(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () => sendJson("DELETE", `/archives/${id}/patch`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["archive", id] })
      queryClient.invalidateQueries({ queryKey: ["archive-pages", id] })
      queryClient.invalidateQueries({ queryKey: ["archives"] })
    },
  })
}

/** Reader overview overlay's "add chapter" hover icon (legacy `.add-toc`, `addTocSection`). */
export function useAddTocEntry(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ page, title }: { page: number; title: string }) =>
      sendJson("PUT", `/archives/${id}/toc?page=${page}&title=${encodeURIComponent(title)}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["archive", id] })
      queryClient.invalidateQueries({ queryKey: ["archives"] })
    },
  })
}

/** Chapter selector's edit/delete actions (legacy `.edit-toc`/`.remove-toc`). */
export function useRemoveTocEntry(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (page: number) => sendJson("DELETE", `/archives/${id}/toc?page=${page}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["archive", id] })
      queryClient.invalidateQueries({ queryKey: ["archives"] })
    },
  })
}

/** Unbound counterpart to `useAddTocEntry` — the target archive id is only known once a
 * Tankoubon-wide global page number has been resolved back to its real member archive at call
 * time (`ArchiveOverviewOverlay`'s own `resolvePage` prop), not fixed at mount like the bound
 * version above. Matches `useUpdateProgress`/`useSetArchiveProgress`'s own bound/unbound pairing.
 * (Setting a *cover* thumbnail has no equivalent unbound variant: Tankoubon-mode "set as cover"
 * always targets the Tankoubon's own cover via `useSetTankoubonThumbnail`, never a specific
 * member archive's — matches legacy's own `reader_archive_overlay.js`.) */
export function useAddTocEntryForId() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, page, title }: { id: string; page: number; title: string }) =>
      sendJson("PUT", `/archives/${id}/toc?page=${page}&title=${encodeURIComponent(title)}`),
    onSuccess: (_data, { id }) => {
      queryClient.invalidateQueries({ queryKey: ["archive", id] })
      queryClient.invalidateQueries({ queryKey: ["archives"] })
    },
  })
}

/** Unbound counterpart to `useRemoveTocEntry` — see `useSetArchiveThumbnailForId`'s own docs. */
export function useRemoveTocEntryForId() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, page }: { id: string; page: number }) => sendJson("DELETE", `/archives/${id}/toc?page=${page}`),
    onSuccess: (_data, { id }) => {
      queryClient.invalidateQueries({ queryKey: ["archive", id] })
      queryClient.invalidateQueries({ queryKey: ["archives"] })
    },
  })
}

export function useDeleteArchive() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => sendJson("DELETE", `/archives/${id}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["archives"] }),
  })
}

export function useTankoubon(id: string) {
  return useQuery({
    queryKey: ["tankoubon", id],
    queryFn: () => fetchJson<TankoubonMetadata>(`/tankoubons/${id}`),
    enabled: id !== "",
  })
}

export function useCreateTankoubon() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (name: string) => sendForm<{ tankoubon_id: string }>("PUT", "/tankoubons", { name }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["tankoubons"] }),
  })
}

export function useUpdateTankoubon(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: {
      archives?: string[]
      metadata?: { name?: string; summary?: string; tags?: string; chapter_names?: { id: string; name: string }[] }
    }) => sendJson("PUT", `/tankoubons/${id}`, body),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["tankoubon", id] })
      queryClient.invalidateQueries({ queryKey: ["tankoubon-full", id] })
      queryClient.invalidateQueries({ queryKey: ["tankoubons"] })
    },
  })
}

export function useDeleteTankoubon() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => sendJson("DELETE", `/tankoubons/${id}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["tankoubons"] }),
  })
}

export function useAddToTankoubon(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (archiveId: string) => sendJson("PUT", `/tankoubons/${id}/${archiveId}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["tankoubon", id] }),
  })
}

/** `GET /tankoubons/{id}/full` — every member archive's own full metadata (including `pagecount`,
 * needed to build the cumulative page-offset table), not just the summary `useTankoubon` returns.
 * Used by `useTankoubonReading` to read a Tankoubon as one concatenated multi-archive book. */
export function useTankoubonFull(id: string | null) {
  return useQuery({
    queryKey: ["tankoubon-full", id],
    queryFn: () => fetchJson<TankoubonFullResponse>(`/tankoubons/${id}/full?page=-1`),
    enabled: id !== null,
  })
}

export interface TankoubonAiRenameResponse {
  suggestions: {
    tank_name: string
    chapters: { original_index: number; sorted_index: number; name: string }[]
  }[]
  original_member_names: { id: string; title: string; index: number }[]
}

export function useLlmKeyStatus() {
  return useQuery({
    queryKey: ["llm-key-status"],
    queryFn: () => fetchJson<{ configured: boolean }>("/llm/key-status"),
    staleTime: 60_000,
  })
}

export function useAiRenameTankoubon() {
  return useMutation({
    mutationFn: (id: string) =>
      sendJson<TankoubonAiRenameResponse>("POST", `/tankoubons/${id}/ai/rename-suggestions`),
  })
}

export function useAiRenameChapter() {
  return useMutation({
    mutationFn: ({ tankId, archiveIndex }: { tankId: string; archiveIndex: number }) =>
      sendJson<{ name: string }>("POST", `/tankoubons/${tankId}/ai/rename-chapter`, { archive_index: archiveIndex }),
  })
}

export interface AiGroupSuggestion {
  /** New members to add — for an `existing_tankoubon_id` suggestion, these are the *additional*
   * archives only; the Tankoubon's own existing members are never repeated here. */
  archive_ids: string[]
  /** Present when this suggestion is "add these archives to an existing Tankoubon" rather than
   * "group these loose archives into a new one" — see the backend's own `tankoubon_grouping.rs`
   * module docs for how an existing Tankoubon participates in the same clique algorithm as a
   * synthetic node. */
  existing_tankoubon_id?: string
}

/** `POST /tankoubons/ai-group-suggestions` — no id param (unlike `useAiRenameTankoubon`), since
 * this scans the whole library's ungrouped archives rather than operating on one already-existing
 * Tankoubon. Local-model-only (see that endpoint's own module docs) — no `useLlmKeyStatus` gate
 * needed, just the same `model_not_ready` 503 the reader recommendations endpoint can return,
 * surfaced to the caller as a thrown `ApiError` like any other failed mutation.
 *
 * `includeIgnored` mirrors the backend's own `?include_ignored=true` query param (default
 * `false`) — the "Show ignored combinations" checkbox re-requests with this set to `true` rather
 * than filtering a locally-cached result, since the ignored set can change between requests (a
 * user un-ignoring something in a previous session) and the backend is the source of truth for
 * which fingerprints are currently ignored. */
export function useAiGroupSuggestions() {
  return useMutation({
    mutationFn: (includeIgnored?: boolean) =>
      sendJson<{ suggestions: AiGroupSuggestion[] }>(
        "POST",
        `/tankoubons/ai-group-suggestions${includeIgnored ? "?include_ignored=true" : ""}`,
      ),
  })
}

export interface IgnoredGroupSuggestion {
  archive_ids: string[]
  existing_tankoubon_id?: string
  ignored_at: number
}

/** Backs the "Show ignored combinations" checklist in `AiSmartTankoubonModal.tsx` — the raw
 * dismissed-suggestion entries; titles are resolved client-side against the already-loaded
 * archive list (same `titleById` map the main suggestion cards use), not hydrated server-side. */
export function useIgnoredGroupSuggestions(enabled: boolean) {
  return useQuery({
    queryKey: ["tankoubons", "ai-group-suggestions", "ignored"],
    queryFn: () => fetchJson<{ ignored: IgnoredGroupSuggestion[] }>("/tankoubons/ai-group-suggestions/ignored"),
    enabled,
  })
}

/** "Don't suggest this again" — dismisses one specific suggestion (exact archive-id-set +
 * `existing_tankoubon_id` match, see the backend's own `fingerprint` docs) so future
 * `useAiGroupSuggestions()` calls skip it by default. */
export function useIgnoreGroupSuggestion() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: { archive_ids: string[]; existing_tankoubon_id?: string }) =>
      sendJson("POST", "/tankoubons/ai-group-suggestions/ignore", body),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["tankoubons", "ai-group-suggestions", "ignored"] }),
  })
}

/** Re-enables a previously-ignored suggestion (the "Un-ignore" button on the ignored-combinations
 * checklist). */
export function useUnignoreGroupSuggestion() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: { archive_ids: string[]; existing_tankoubon_id?: string }) =>
      sendJson("DELETE", "/tankoubons/ai-group-suggestions/ignore", body),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["tankoubons", "ai-group-suggestions", "ignored"] }),
  })
}

/** Same shape as `useUpdateProgress`, but for a Tankoubon read as one concatenated book — `page`
 * is the *global* page number across every member archive, matching the backend's own
 * `PUT /tankoubons/{id}/progress/{page}` (`update_tankoubon_progress`), which stores it directly
 * on the Tankoubon's own record rather than resolving it back to any one member archive's
 * progress. */
export function useUpdateTankoubonProgress(id: string | null) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (page: number) => sendJson("PUT", `/tankoubons/${id}/progress/${page}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["tankoubon", id] })
      queryClient.invalidateQueries({ queryKey: ["tankoubons"] })
    },
  })
}

/** Sets a Tankoubon's own cover thumbnail from a page within one of its member archives,
 * addressed by *global* page number — the backend (`update_tankoubon_thumbnail`) resolves which
 * member archive that page actually falls in server-side (matching legacy's own
 * `translate_global_page`), so the caller doesn't need to do that resolution itself first. */
export function useSetTankoubonThumbnail(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (page: number) => sendJson("PUT", `/tankoubons/${id}/thumbnail?page=${page}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["tankoubon", id] }),
  })
}

export function useServerInfo() {
  return useQuery({
    queryKey: ["info"],
    queryFn: () => fetchJson<ServerInfo>("/info"),
  })
}

export interface UpdateCheckResult {
  latestVersion: string
  releaseUrl: string
}

function extractVersionNumbers(raw: string): number[] {
  return (raw.match(/\d+/g) ?? []).map(Number)
}

/** Numeric, position-by-position comparison (`1.10.0` > `1.9.0`) — legacy's own version of this
 * check (`~/LANraragi/public/js/mod/index.js::checkVersion`) instead concatenates each version's
 * digit groups into one string and compares those lexicographically, which silently breaks the
 * moment either version reaches a two-digit component (`"1.10.0"` → `"1100"` sorts *before*
 * `"1.9.0"` → `"190"`). Not carrying that bug forward into a from-scratch implementation. */
function compareVersions(a: number[], b: number[]): number {
  const len = Math.max(a.length, b.length)
  for (let i = 0; i < len; i++) {
    const diff = (a[i] ?? 0) - (b[i] ?? 0)
    if (diff !== 0) return diff
  }
  return 0
}

/** Mirrors legacy's own client-side update check (`checkVersion` in the file above) — hits
 * GitHub's public releases API directly from the browser (no backend involvement needed;
 * `/api/info` already tells us our own running version) and is skipped entirely in debug mode,
 * same as legacy. Resolves to `null` on *any* failure (no release published yet, rate-limited,
 * offline) rather than throwing — this is a "nice to have" notification, never something that
 * should surface as an error to the user. */
export function useUpdateCheck(currentVersion: string | undefined, debugMode: boolean) {
  return useQuery({
    queryKey: ["update-check", currentVersion],
    queryFn: async (): Promise<UpdateCheckResult | null> => {
      const response = await fetch(
        "https://api.github.com/repos/thelastfantasy/LANrurugi/releases/latest",
      )
      if (!response.ok) return null
      const data = (await response.json()) as { tag_name?: string; html_url?: string }
      if (!data.tag_name || !data.html_url) return null

      const latest = extractVersionNumbers(data.tag_name)
      const current = extractVersionNumbers(currentVersion ?? "")
      if (compareVersions(latest, current) <= 0) return null

      return { latestVersion: data.tag_name, releaseUrl: data.html_url }
    },
    enabled: !debugMode && !!currentVersion,
    staleTime: UPDATE_CHECK_STALE_TIME_MS,
    retry: false,
  })
}

export function useLogin() {
  return useMutation({
    mutationFn: (password: string) => sendForm("POST", "/login", { password }),
  })
}

export function useLogout() {
  return useMutation({
    mutationFn: () => sendJson("POST", "/logout"),
  })
}

/** Legacy's `userlogged` (`enable_pass == 0 || session('is_logged')`) — gates admin-only reader
 * UI (Clean Archive Cache, Archive Overview's edit/delete/category panel) and picks which side of
 * the progress-persistence decision tree applies. See `crates/lanrurugi-api/src/login.rs::status`
 * for why this is its own endpoint rather than a field on `ServerInfo`. */
export function useLoginStatus() {
  return useQuery({
    queryKey: ["login-status"],
    queryFn: () => fetchJson<LoginStatus>("/login/status"),
  })
}

export function useChangePassword() {
  return useMutation({
    mutationFn: (password: string) => sendForm("POST", "/settings/password", { password }),
  })
}

// --- API token management (issue #54) -----------------------------------------------------------
// Replaces legacy's single fixed `apikey` — see `crates/lanrurugi-api/src/api_tokens.rs`'s own
// module docs. Mounted in the protected router, so these all require an already-valid session.

export function useApiTokens() {
  return useQuery({
    queryKey: ["api-tokens"],
    queryFn: () => fetchJson<ApiToken[]>("/tokens"),
  })
}

/** Response's `data.token` is the raw value — present in this one response only, never
 *  retrievable again afterward. Callers must show it to the user immediately (see
 *  `Settings/ApiTokensSection.tsx`'s "new token" flow) since neither this hook nor any later
 *  `useApiTokens()` refetch will ever see it again.
 *
 *  `expiresInSecs: undefined` (omitted) issues a permanent token — matches the backend's own
 *  `expires_in_secs: Option<i64>` (`None` = permanent), so this hook forwards `undefined` as-is
 *  rather than substituting a sentinel the server would have to special-case. */
export function useCreateApiToken() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ name, role, expiresInSecs }: { name: string; role: TokenRole; expiresInSecs?: number }) =>
      sendJson<{ data: ApiTokenCreateResponse }>("POST", "/tokens", {
        name,
        role,
        expires_in_secs: expiresInSecs,
      }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["api-tokens"] }),
  })
}

export function useDeleteApiToken() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => sendJson("DELETE", `/tokens/${encodeURIComponent(id)}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["api-tokens"] }),
  })
}

export function useRenameApiToken() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, name }: { id: string; name: string }) =>
      sendJson<{ data: ApiToken }>("PATCH", `/tokens/${encodeURIComponent(id)}`, { name }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["api-tokens"] }),
  })
}

export function useShinobuStatus() {
  return useQuery({
    queryKey: ["shinobu"],
    queryFn: () => fetchJson<ShinobuStatus>("/shinobu"),
    refetchInterval: POLL_INTERVAL_MS,
  })
}

export function useShinobuAction() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (action: "stop" | "restart" | "rescan") => sendJson("POST", `/shinobu/${action}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["shinobu"] }),
  })
}

export function useClearNewFlags() {
  return useMutation({
    mutationFn: () => sendJson("DELETE", "/database/isnew"),
  })
}

/** Clears a single archive's own "new" badge — legacy's reader fires the equivalent
 * `DELETE /api/archives/{id}/isnew` the moment it loads (`reader_common.js`'s init sequence,
 * unconditionally, not tied to finishing the archive or any elapsed time), which is what actually
 * makes the badge disappear after a first read rather than staying "new" forever. */
export function useClearArchiveNew() {
  return useMutation({
    mutationFn: (id: string) => sendJson("DELETE", `/archives/${id}/isnew`),
  })
}

export function useRegenThumbnails() {
  return useMutation({
    mutationFn: (force: boolean) => sendJson("POST", `/regen_thumbs?force=${force}`),
  })
}

export function useDiscardSearchCache() {
  return useMutation({
    mutationFn: () => sendJson("DELETE", "/search/cache"),
  })
}

// --- Reader: stamps (page annotations) ---
// `crates/lanrurugi-api/src/stamps.rs` — full CRUD, verified against legacy's `Model/Stamp.pm`.

export function useStampedPages(id: string | null) {
  return useQuery({
    queryKey: ["stamped-pages", id],
    queryFn: () => fetchJson<StampedPagesResponse>(`/archives/${id}/stamps`),
    enabled: id !== null,
  })
}

/** Fetches stamped-page lists for several archives in parallel — used by
 * `ArchiveOverviewOverlay`'s "filter stamped pages" toggle in Tankoubon-read mode, where the
 * stamped-pages indicator has to cover every member archive, not just one. Query keys match
 * `useStampedPages`'s own (`['stamped-pages', id]`), so this shares cache with a single-archive
 * read of the same archive elsewhere. Returns the raw per-archive results in `ids`' own order —
 * the caller (which already has each archive's own global-page offset) is what converts each
 * entry's local page numbers into the Tankoubon's global page numbering. */
export function useStampedPagesForArchives(ids: string[]) {
  return useQueries({
    queries: ids.map((id) => ({
      queryKey: ["stamped-pages", id],
      queryFn: () => fetchJson<StampedPagesResponse>(`/archives/${id}/stamps`),
    })),
  })
}

export function useStampsForPage(id: string | null, page: number) {
  return useQuery({
    queryKey: ["stamps", id, page],
    queryFn: () => fetchJson<StampsByPageResponse>(`/archives/${id}/stamps/${page}`),
    enabled: id !== null && page > 0,
  })
}

export function useAddStamp(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({
      page,
      content,
      position,
      icon,
      rect,
    }: {
      page: number
      content: string
      position: string
      icon?: string
      rect?: string
    }) => {
      const params = new URLSearchParams({ content, position })
      if (icon) params.set("icon", icon)
      if (rect) params.set("rect", rect)
      // The response's own `stamp_id` is the new stamp's real ID (`crates/lanrurugi-api::stamps::
      // add_stamp` — matches legacy's own `add_stamp` response shape) — declared here (not left
      // as the untyped default) so a call site can read it back out of `mutate`'s own `onSuccess`
      // callback, e.g. to select a stamp immediately after creating it via Ctrl+drag copy.
      return sendJson<{ stamp_id: string }>("PUT", `/archives/${id}/stamps/${page}?${params.toString()}`)
    },
    onSuccess: (_data, { page }) => {
      queryClient.invalidateQueries({ queryKey: ["stamps", id, page] })
      queryClient.invalidateQueries({ queryKey: ["stamped-pages", id] })
    },
  })
}

export function useUpdateStamp() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({
      stampId,
      content,
      position,
      icon,
      rect,
    }: {
      stampId: string
      content?: string
      position?: string
      icon?: string
      rect?: string
    }) => {
      const params = new URLSearchParams()
      if (content !== undefined) params.set("content", content)
      if (position !== undefined) params.set("position", position)
      if (icon !== undefined) params.set("icon", icon)
      if (rect !== undefined) params.set("rect", rect)
      return sendJson("PUT", `/stamps/${stampId}?${params.toString()}`)
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["stamps"] }),
  })
}

export function useDeleteStamp() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (stampId: string) => sendJson("DELETE", `/stamps/${stampId}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["stamps"] })
      queryClient.invalidateQueries({ queryKey: ["stamped-pages"] })
    },
  })
}

// --- Reader: thumbnail generation trigger ---
// `POST /archives/{id}/files/thumbnails` always completes synchronously (pages are decoded on
// demand, not pre-extracted — see the doc comment on `generate_page_thumbnails`), so this is a
// plain mutation with no job-polling needed.

export function useGenerateThumbnails(id: string) {
  return useMutation({
    mutationFn: () => sendJson("POST", `/archives/${id}/files/thumbnails`),
  })
}

/** Same endpoint as `useGenerateThumbnails`, but not bound to one archive at mount time — used by
 * the reader's "Clean Archive Cache" link when reading a Tankoubon as one concatenated book
 * (`useTankoubonReading`'s own `chapters`), where "the archive" is actually N member archives
 * decided at render time, not a single id `Reader.tsx` is mounted with. Matches
 * `useUpdateProgress`/`useSetArchiveProgress`'s own bound/unbound pairing. */
export function useGenerateThumbnailsForArchives() {
  return useMutation({
    mutationFn: (ids: string[]) =>
      Promise.all(ids.map((id) => sendJson("POST", `/archives/${id}/files/thumbnails`))),
  })
}

// --- Reader: random archive ---
// Legacy's reader just links `<a href="/random">` (plain navigation). Not a react-query hook —
// every call must pick a fresh random archive, there's nothing to cache.
export async function fetchRandomArchiveId(): Promise<string | null> {
  const result = await fetchJson<RandomArchivesResponse>("/search/random?count=1")
  return result.data[0]?.arcid ?? null
}

// --- Reader: bookmark link ---
// `crates/lanrurugi-api/src/categories.rs` — the static category (if any) the reader's bookmark
// icon (`B` key) toggles archive membership in, backed by `LRR_CONFIG`'s `bookmark_link` field.

export function useBookmarkLink() {
  return useQuery({
    queryKey: ["bookmark-link"],
    queryFn: () => fetchJson<BookmarkLinkResponse>("/categories/bookmark_link"),
  })
}

export function useCleanTempfolder() {
  return useMutation({
    mutationFn: () => sendJson("DELETE", "/tempfolder"),
  })
}

export function useCleanDatabase() {
  return useMutation({
    mutationFn: () => sendJson<{ deleted: number; unlinked: number }>("POST", "/database/clean"),
  })
}

export function useDropDatabase() {
  return useMutation({
    mutationFn: () => sendJson("POST", "/database/drop"),
  })
}

export function useDuplicates() {
  return useQuery({
    queryKey: ["duplicates"],
    queryFn: () => fetchJson<DuplicateGroup[]>("/database/duplicates"),
  })
}

export function useScanDuplicates() {
  return useMutation({
    mutationFn: (threshold: number) =>
      sendJson<{ job: string }>("POST", `/database/duplicates/scan?threshold=${threshold}`),
  })
}

export function useClearDuplicates() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () => sendJson("DELETE", "/database/duplicates"),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["duplicates"] }),
  })
}

export const LOG_CATEGORIES = ["general", "shinobu", "plugins", "redis", "mojo"] as const

export type LogCategory = (typeof LOG_CATEGORIES)[number]

export function useLogLines(category: LogCategory, lines = 100) {
  return useQuery({
    queryKey: ["logs", category, lines],
    queryFn: () => fetchText(`/logs/${category}?lines=${lines}`),
    refetchInterval: POLL_INTERVAL_MS,
  })
}

// --- Background Job Console (specs/002-job-console) --------------------------------------------
// Native `/api/jobs` endpoints, additive over the legacy-mimicking `/api/minion/*` contract
// (research.md §1).

function jobsRefetchInterval(query: { state: { data?: JobsResponse } }) {
  const jobs = query.state.data?.jobs ?? []
  const active = jobs.some((job) => job.state === "active")
  return active ? DOWNLOAD_QUEUE_POLL_INTERVAL_MS : POLL_INTERVAL_MS
}

export function useJobs() {
  return useQuery({
    queryKey: ["jobs"],
    queryFn: () => fetchJson<JobsResponse>("/jobs"),
    // `select` unwraps the `{ jobs: [...] }` envelope so consumers get the array directly.
    select: (data) => data.jobs as JobRecord[],
    // Fast cadence only while a job is actually active — see `downloadQueueRefetchInterval`'s own
    // reasoning above.
    refetchInterval: jobsRefetchInterval,
    refetchIntervalInBackground: true,
  })
}

// Multi-select clear: fires one `DELETE /jobs/{id}` per selected job (no batch-by-ids endpoint
// exists by design — research.md §5 keeps the backend minimal). Only terminal jobs are ever
// selectable in the UI (FR-004), so every selected id is clearable; per-id try/catch still
// tolerates a job vanishing between select and click. Reports how many actually got removed.
export function useClearJobs() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async (ids: string[]) => {
      let succeeded = 0
      let failed = 0
      await Promise.all(
        ids.map(async (id) => {
          try {
            await sendJson("DELETE", `/jobs/${encodeURIComponent(id)}`)
            succeeded += 1
          } catch {
            failed += 1
          }
        }),
      )
      return { succeeded, failed }
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["jobs"] }),
  })
}

// `useClearFinishedJobs()` is deliberately unscoped (research.md §5 / FR-004): it always targets
// the full server-side set of finished+failed jobs, regardless of any client-side state/name filter
// the admin happens to have applied to the displayed list.
export function useClearFinishedJobs() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () => sendJson<{ cleared: number }>("DELETE", "/jobs?state=finished"),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["jobs"] }),
  })
}

// --- Upload page's persistent download queue ----------------------------------------------------
// Additive `/download_queue*` endpoints (no legacy equivalent) — see `DownloadQueueItem`'s own
// docs. Live updates arrive over SSE (`GET /download_queue/stream`), not polling — see
// `useDownloadQueue`'s own EventSource wiring below.

/** Partial fields an `update` delta carries — set-if-present merged over the existing item.
 * Flat on the event itself (not nested under an `item` key) because every backend broadcast site
 * (`update_queue_item_state` in `plugins.rs`, the `update`-kind sends in `download_queue.rs`)
 * spreads these directly onto the JSON object alongside `kind` — unlike the `add` case below,
 * whose payload really is the *whole* record under `item` (a freshly created queue entry, not a
 * partial patch). */
interface DownloadQueueItemPatch {
  id: string
  state?: DownloadQueueItem["state"]
  job_id?: string | null
  archive_ids?: string[] | null
  title?: string | null
  metadata_preview?: Record<string, unknown> | null
  error?: DownloadQueueItem["error"]
}

/** The `delta` event payload — `kind` decides how the client folds it into the cached list. */
type DownloadQueueDelta =
  | ({ kind: "update" } & DownloadQueueItemPatch)
  | { kind: "add"; item: DownloadQueueItem }
  | { kind: "remove"; id: string }

export function useDownloadQueue() {
  const queryClient = useQueryClient()

  // Initial snapshot via a plain one-shot fetch — no polling. Live updates arrive over the
  // SSE stream below (`full` replaces the whole list, `delta` patches one item in place).
  const query = useQuery({
    queryKey: ["download-queue"],
    queryFn: () => fetchJson<DownloadQueueListResponse>("/download_queue"),
    select: (data) => data.items,
  })

  useEffect(() => {
    const source = new EventSource("/api/download_queue/stream")
    source.addEventListener("full", (e) => {
      const data = JSON.parse((e as MessageEvent<string>).data) as {
        type: "full"
        items: DownloadQueueItem[]
      }
      if (data.type !== "full") return
      queryClient.setQueryData(["download-queue"], (old: DownloadQueueListResponse | undefined) => ({
        ...(old ?? { recordsTotal: 0, recordsFiltered: 0 }),
        items: data.items,
      }))
    })
    source.addEventListener("delta", (e) => {
      const data = JSON.parse((e as MessageEvent<string>).data) as {
        type: "delta"
      } & DownloadQueueDelta
      if (data.type !== "delta") return
      queryClient.setQueryData(["download-queue"], (old: DownloadQueueListResponse | undefined) => {
        if (!old) return old
        let items = old.items
        switch (data.kind) {
          case "update": {
            const { type: _type, kind: _kind, ...patch } = data
            items = items.map((it) => (it.id === patch.id ? { ...it, ...patch } : it))
            break
          }
          case "add":
            // Append unless the id is somehow already present (e.g. the `full` bootstrap
            // raced ahead of this delta) — a replace then, never a duplicate row.
            items = items.some((it) => it.id === data.item.id)
              ? items.map((it) => (it.id === data.item.id ? data.item : it))
              : [...items, data.item]
            break
          case "remove":
            items = items.filter((it) => it.id !== data.id)
            break
        }
        return { ...old, items }
      })
    })
    return () => source.close()
  }, [queryClient])

  return query
}

export function useAddToQueue() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (items: AddToQueueItem[]) =>
      sendJson<AddToQueueResponse>("POST", "/download_queue", { items }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["download-queue"] }),
  })
}

export function useUpdateQueueItem() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, ...update }: UpdateQueueItemBody & { id: string }) =>
      sendJson<{ item: DownloadQueueItem }>("PATCH", `/download_queue/${encodeURIComponent(id)}`, update),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["download-queue"] }),
  })
}

/** "Fetch Metadata" button — runs the backend's plugin-`execMetadata` + 10-min-cache path
 * (`POST /download_queue/{id}/fetch-metadata`), not a direct `/plugins/use` call, so a manual
 * preview shares the same cache the post-download auto-fetch uses. No `onSuccess` invalidation —
 * the updated item arrives via the `/download_queue/stream` SSE delta the backend broadcasts. */
export function useFetchQueueItemMetadata() {
  return useMutation({
    mutationFn: (id: string) =>
      sendJson<{ success: number; metadata_preview?: Record<string, unknown> }>(
        "POST",
        `/download_queue/${encodeURIComponent(id)}/fetch-metadata`,
      ),
  })
}

export function useDeleteQueueItem() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => sendJson("DELETE", `/download_queue/${encodeURIComponent(id)}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["download-queue"] }),
  })
}

export function useStartQueueItem() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) =>
      sendJson<{ job: string }>("POST", `/download_queue/${encodeURIComponent(id)}/start`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["download-queue"] })
      queryClient.invalidateQueries({ queryKey: ["jobs"] })
    },
  })
}

export function useStopQueueItem() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) =>
      sendJson("POST", `/download_queue/${encodeURIComponent(id)}/stop`),
    // Optimistic: cancellation is cooperative (the download task has to notice the token and
    // clean up before server-side state actually flips), so waiting for a poll round-trip made the
    // button swap feel laggy. Flips `state` to `cancelled` in the cache synchronously on click; the
    // eventual real poll overwrites this with the server's matching value regardless. `cancelled`
    // is a real, persisted queue state, so this also survives a page refresh.
    onMutate: async (id: string) => {
      await queryClient.cancelQueries({ queryKey: ["download-queue"] })
      const previous = queryClient.getQueryData<DownloadQueueListResponse>(["download-queue"])
      queryClient.setQueryData<DownloadQueueListResponse>(["download-queue"], (data) =>
        data
          ? {
              items: data.items.map((item) =>
                item.id === id ? { ...item, state: "cancelled" } : item,
              ),
            }
          : data,
      )
      return { previous }
    },
    onError: (_err, _id, context) => {
      if (context?.previous) queryClient.setQueryData(["download-queue"], context.previous)
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["download-queue"] })
      queryClient.invalidateQueries({ queryKey: ["jobs"] })
    },
  })
}

/** `insertions`, when given, packages B's own unique pages (`ComparisonResult.b_unmatched_pages`,
 * issue #77's own follow-on design) into a `.patch.zip` for the new A that's replacing B —
 * omitted for every ordinary overwrite that never went through the AI comparison flow, which
 * remains a plain `{ id }` call exactly as before this parameter existed. */
export function useOverwriteQueueItem() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, insertions }: { id: string; insertions?: ExportPatchInsertion[] }) =>
      sendJson<{ archive_id: string }>("POST", `/download_queue/${encodeURIComponent(id)}/overwrite`, {
        insertions: insertions ?? [],
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["download-queue"] })
      queryClient.invalidateQueries({ queryKey: ["jobs"] })
    },
  })
}

export function useRenameQueueItem() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, filename }: { id: string; filename: string }) =>
      sendJson<{ archive_id: string }>("POST", `/download_queue/${encodeURIComponent(id)}/rename`, {
        filename,
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["download-queue"] })
      queryClient.invalidateQueries({ queryKey: ["jobs"] })
    },
  })
}

/** Read-only — doesn't invalidate any query (unlike overwrite/rename above), since a comparison
 * resolves nothing about the conflict itself, just informs the user's own choice of which
 * resolution to pick next. */
export function useCompareQueueItem() {
  return useMutation({
    mutationFn: (id: string) => sendJson<{ result: ComparisonResult }>("POST", `/download_queue/${encodeURIComponent(id)}/compare`),
  })
}

/** B's own real entry-name list, natural-sorted (issue #77's own follow-on design) — the "insert
 * after/before this page" anchor picker needs real filenames to send to `useExportComparePatch`,
 * not just the numeric indices every other part of the comparison UI deals in (see
 * `ExportPatchInsertion`'s own docs for why `patch.rs`'s JSON schema uses filenames, not indices,
 * as its anchor). Only ever fetched once the user actually opens the picker (`enabled`), not
 * eagerly alongside the comparison result itself. */
export function useComparePages(id: string | null, side: "a" | "b") {
  return useQuery({
    queryKey: ["compare-pages", id, side],
    queryFn: () => fetchJson<{ pages: string[] }>(`/download_queue/${encodeURIComponent(id ?? "")}/compare/pages?side=${side}`),
    enabled: id !== null,
  })
}

/** Builds a `.patch.zip` from a user-picked selection of A's own unique pages
 * (`ComparisonResult.a_unmatched_pages`, issue #77's own follow-on design) and returns it as a
 * `Blob` ready for the caller to trigger a real browser download with — this mutation doesn't
 * install the patch anywhere itself (confirmed design: the user places it next to the target
 * archive on disk manually), so there's nothing here to invalidate either, same as
 * `useCompareQueueItem` above. */
export function useExportComparePatch() {
  return useMutation({
    mutationFn: ({ id, insertions }: { id: string; insertions: ExportPatchInsertion[] }) =>
      sendJsonForBlob("POST", `/download_queue/${encodeURIComponent(id)}/compare/export-patch`, { insertions }),
  })
}

/** "Keep the existing library archive (B), discard this download (A)" — issue #77's own follow-on
 * design. `insertions`, when given, packages A's own unique pages straight onto B's own sidecar
 * `.patch.zip` server-side (unlike `useExportComparePatch` above, this doesn't hand back a file
 * for the user to place themselves — this flow's own frontend already walked them through picking
 * insertion points as part of *choosing* to keep B). Always deletes the queue item on success
 * (nothing is left for it to track — see the endpoint's own docs), so this invalidates the queue
 * list same as delete/overwrite/rename. */
export function useKeepSideB() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ id, insertions }: { id: string; insertions?: ExportPatchInsertion[] }) =>
      sendJson<{ success: number }>("POST", `/download_queue/${encodeURIComponent(id)}/compare/keep-b`, {
        insertions: insertions ?? [],
      }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["download-queue"] })
      queryClient.invalidateQueries({ queryKey: ["jobs"] })
    },
  })
}

export function useStartSelectedQueue() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (ids: string[]) => sendJson("POST", "/download_queue/start_selected", { ids }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["download-queue"] })
      queryClient.invalidateQueries({ queryKey: ["jobs"] })
    },
  })
}

export function useClearCompletedQueue() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () => sendJson<{ cleared: number }>("POST", "/download_queue/clear_completed"),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["download-queue"] }),
  })
}

export function useDeleteSelectedQueue() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (ids: string[]) =>
      sendJson<{ deleted: string[] }>("POST", "/download_queue/delete_selected", { ids }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["download-queue"] }),
  })
}
