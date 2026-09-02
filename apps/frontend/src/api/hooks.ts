import {
  keepPreviousData,
  useInfiniteQuery,
  useMutation,
  useQueries,
  useQuery,
  useQueryClient,
} from "@tanstack/react-query"
import { useEffect, useSyncExternalStore } from "react"

import { MSM_SELECTION_KEY } from "@/lib/storageKeys"
import { isTankoubonId } from "@/lib/utils/isTankoubonId"
import { clearSearchNavigationState } from "@/pages/Reader/crossArchiveNav"

import {
  ApiError,
  clearLastRefreshTimestamp,
  fetchJson,
  fetchText,
  sendForm,
  sendJson,
  sendJsonForBlob,
} from "./client"
import type {
  ActivityFacets,
  ActivityFilter,
  ActivityPage,
  ActivityRetention,
  AddToQueueItem,
  AddToQueueResponse,
  ApiToken,
  ApiTokenCreateResponse,
  ArchiveFilesResponse,
  ArchiveMetadata,
  BatchDeleteArchivesResponse,
  BookmarkedPageResponse,
  BookmarkSort,
  BookmarksPageResponse,
  CategoryMetadata,
  ComparisonResult,
  DownloadQueueItem,
  DownloadQueueListResponse,
  DuplicateGroup,
  ExportPatchInsertion,
  HoverPageOrderResponse,
  ImportSnapshotMetadata,
  JobRecord,
  JobsResponse,
  LoginStatus,
  OnlyMatchingBookmarksResponse,
  PageDimensionsResponse,
  PluginInfo,
  PluginOptions,
  PluginOptionsUpdate,
  PluginSettings,
  PluginSettingsUpdate,
  PublicThemeSettings,
  RandomArchivesResponse,
  SearchResponse,
  ServerInfo,
  Settings,
  ShinobuStatus,
  StampedPagesResponse,
  StampsByPageResponse,
  StatTag,
  TankBookmarkedPageResponse,
  TankoubonFullResponse,
  TankoubonListResponse,
  TankoubonMetadata,
  TokenRole,
  UpdateQueueItemBody,
  VersionCheckResponse,
} from "./types"

/** Shared polling cadence for background status indicators (Shinobu status, log tail). */
const POLL_INTERVAL_MS = 5000
/** Faster cadence for actively-watched progress (jobs, download queue). */
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

/** A static category has an empty `search`; a dynamic one stores its filter expression there. */
export function useCreateCategory() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (params: { name: string; isDynamic: boolean; search?: string; visibleToGuest?: boolean }) =>
      sendForm<{ category_id: string }>("PUT", "/categories", {
        name: params.name,
        search: params.isDynamic ? (params.search?.trim() || "language:english") : "",
        // Only literal "true"/"false" deserialize; "1"/"0" fail with a 422.
        visible_to_guest: (params.visibleToGuest ?? false) ? "true" : "false",
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

export function useSettings(options?: { enabled?: boolean }) {
  return useQuery({
    queryKey: ["settings"],
    queryFn: () => fetchJson<Settings>("/settings"),
    enabled: options?.enabled,
  })
}

/** Unauthenticated theme/language read via public `GET /theme` (`GET /settings` 401s pre-login).
 * The `with-admin-theme` suffix deliberately bumps the cache key: an older in-memory response
 * from before the endpoint carried `admin_theme` would otherwise survive HMR/fast refresh and
 * make `/login` fall back to the guest theme. */
export function usePublicSettings(options?: { enabled?: boolean }) {
  return useQuery({
    queryKey: ["theme", "with-admin-theme"],
    queryFn: () => fetchJson<PublicThemeSettings>("/theme"),
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

/** Persists a drag-and-drop reorder of one plugin `type` group. */
export function useReorderPlugins() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: { type: PluginInfo["type"]; order: string[] }) =>
      sendJson("POST", "/plugins/priority", body),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["plugins"] }),
  })
}

/** `null` (not an error) when the plugin declares no `pluginOptions()`. `''` skips the request. */
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

/** A plugin's own persisted custom-parameter values. Distinct from `usePluginOptions` above. */
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

/** Page dimensions for the first `count` pages — caller gates `enabled` on infinite-scroll resume. */
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

/** Unbound `useUpdateProgress`; also invalidates `['search', ...]` queries via predicate. */
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
  /** `"title"` (default), `"lastread"`, or any tag namespace (e.g. `"date_added"`). */
  sortby?: string
  order?: "asc" | "desc"
  /** Pagination cursor — an index into the result set, not a page number. */
  start?: number
  newonly?: boolean
  untaggedonly?: boolean
  tankonly?: boolean
  hidecompleted?: boolean
  groupbyTanks?: boolean
  /** Defaults to `true`; set `false` to skip firing the request entirely. */
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

/** Renames the archive file; `stem` is the basename without extension (extension is kept). */
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

/** Regenerates the archive's cover thumbnail from the given page. */
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

/** Deletes the sidecar `.patch.zip` and clears the archive's `has_patch` flag. */
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

/** Unbound `useAddTocEntry` — archive id resolved from a Tankoubon-global page at call time. */
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

/** Unbound `useRemoveTocEntry` — archive id resolved at call time. */
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

/** Batch delete in one request — session-only endpoint (token roles get 403). */
export function useBatchDeleteArchives() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (ids: string[]) =>
      sendJson<BatchDeleteArchivesResponse>("DELETE", "/archives", { ids }),
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

/** Full member metadata incl. `pagecount` — for reading a Tankoubon as one concatenated book. */
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
  /** Additional archives only — existing members are never repeated. */
  archive_ids: string[]
  /** Present when adding to an existing Tankoubon instead of creating a new one. */
  existing_tankoubon_id?: string
}

/** Whole-library grouping suggestions; local-model-only (503 `model_not_ready` if not ready). */
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

/** Raw dismissed-suggestion entries; titles resolved client-side. */
export function useIgnoredGroupSuggestions(enabled: boolean) {
  return useQuery({
    queryKey: ["tankoubons", "ai-group-suggestions", "ignored"],
    queryFn: () => fetchJson<{ ignored: IgnoredGroupSuggestion[] }>("/tankoubons/ai-group-suggestions/ignored"),
    enabled,
  })
}

/** Dismisses one suggestion by exact archive-id-set match. */
export function useIgnoreGroupSuggestion() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: { archive_ids: string[]; existing_tankoubon_id?: string }) =>
      sendJson("POST", "/tankoubons/ai-group-suggestions/ignore", body),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["tankoubons", "ai-group-suggestions", "ignored"] }),
  })
}

/** Re-enables a previously-ignored suggestion. */
export function useUnignoreGroupSuggestion() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: { archive_ids: string[]; existing_tankoubon_id?: string }) =>
      sendJson("DELETE", "/tankoubons/ai-group-suggestions/ignore", body),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["tankoubons", "ai-group-suggestions", "ignored"] }),
  })
}

/** `page` is the global page number across all member archives, stored on the Tankoubon itself. */
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

/** Sets a Tankoubon's cover from a member-archive page, by *global* page number. */
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
  isUpstream: boolean
  upstreamPull: boolean
}

/** Server-side version/update check (Zipline-style `/api/version`); resolves `null` on any
 * failure instead of throwing, mirroring the old client-side GitHub fetch behavior. */
export function useUpdateCheck() {
  return useQuery({
    queryKey: ["update-check"],
    queryFn: async (): Promise<UpdateCheckResult | null> => {
      try {
        const result = await fetchJson<VersionCheckResponse>("/version")
        if (!result.enabled || !result.data.latest || result.data.isLatest) return null
        return {
          latestVersion: result.data.latest.tag,
          releaseUrl: result.data.latest.url,
          isUpstream: result.data.isUpstream,
          upstreamPull: result.data.latest.commit?.pull ?? false,
        }
      } catch {
        return null
      }
    },
    staleTime: UPDATE_CHECK_STALE_TIME_MS,
    retry: false,
  })
}

export function useLogin() {
  return useMutation({
    mutationFn: (password: string) => sendForm("POST", "/login", { password }),
  })
}

/** On top of the server-side session teardown, also clears local state that must not leak into
 * whatever session (a different admin login, or a guest) starts next on this browser: the
 * in-memory query cache (otherwise a page that doesn't remount — client-side `navigate`, not a
 * reload — keeps serving the outgoing session's cached `["settings"]`/etc. responses until each
 * query happens to refetch on its own), content-derived browsing state (last search, cross-archive
 * navigation), and the unconsumed Library→Batch multi-select handoff. Deliberately leaves every
 * pure display-preference key alone (theme, sort order, reader layout, ...) — those aren't a
 * privacy or correctness concern, and wiping them would just make the next login worse for no
 * benefit. */
export function useLogout() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () => sendJson("POST", "/logout"),
    onSuccess: () => {
      queryClient.clear()
      clearSearchNavigationState()
      clearLastRefreshTimestamp()
      localStorage.removeItem(MSM_SELECTION_KEY)
    },
  })
}

/** Session state — gates admin-only reader UI and progress persistence. */
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

// --- API token management (session-protected) ---

export function useApiTokens() {
  return useQuery({
    queryKey: ["api-tokens"],
    queryFn: () => fetchJson<ApiToken[]>("/tokens"),
  })
}

/** The raw token is present in this one response only — show it to the user immediately.
 *  `expiresInSecs: undefined` issues a permanent token. */
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

// --- LANraragi import rollback snapshots ---

export function useImportSnapshots() {
  return useQuery({
    queryKey: ["import-snapshots"],
    queryFn: () => fetchJson<ImportSnapshotMetadata[]>("/database/import-snapshots"),
  })
}

/** Completed-import count; the Backup page warns on a 2nd-or-later import. */
export function useImportLegacyCount() {
  return useQuery({
    queryKey: ["import-legacy-count"],
    queryFn: () => fetchJson<{ import_count: number }>("/database/import-legacy/count"),
  })
}

export function useDeleteImportSnapshot() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) =>
      sendJson("DELETE", `/database/import-snapshots/${encodeURIComponent(id)}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["import-snapshots"] }),
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

/** Clears one archive's "new" badge (fired on reader load, not on completion). */
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

export function useStampedPages(id: string | null) {
  return useQuery({
    queryKey: ["stamped-pages", id],
    queryFn: () => fetchJson<StampedPagesResponse>(`/archives/${id}/stamps`),
    enabled: id !== null,
  })
}

/** Parallel `useStampedPages` over many archives; shares its query keys/cache. Caller converts
 * local page numbers to Tankoubon-global numbering. */
export function useStampedPagesForArchives(ids: string[]) {
  return useQueries({
    queries: ids.map((id) => ({
      queryKey: ["stamped-pages", id],
      queryFn: () => fetchJson<StampedPagesResponse>(`/archives/${id}/stamps`),
    })),
  })
}

/** `id` may be an archive id or `TANK_xxx`; the id decides which route the request takes. */
export function useStampsForPage(id: string | null, page: number) {
  return useQuery({
    queryKey: ["stamps", id, page],
    queryFn: () =>
      fetchJson<StampsByPageResponse>(
        id !== null && isTankoubonId(id)
          ? `/tankoubons/${id}/stamps/${page}`
          : `/archives/${id}/stamps/${page}`,
      ),
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
      const base = isTankoubonId(id) ? `/tankoubons/${id}/stamps` : `/archives/${id}/stamps`
      return sendJson<{ stamp_id: string }>("PUT", `${base}/${page}?${params.toString()}`)
    },
    onSuccess: (_data, { page }) => {
      queryClient.invalidateQueries({ queryKey: ["stamps", id, page] })
      queryClient.invalidateQueries({ queryKey: ["stamped-pages", id] })
      // Stamp adds can silently auto-bookmark the page server-side — keep bookmarks in sync.
      queryClient.invalidateQueries({ queryKey: ["bookmarks"] })
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
      // Deleting the last stamp can silently auto-remove the page's bookmark.
      queryClient.invalidateQueries({ queryKey: ["bookmarks"] })
    },
  })
}

// --- Reader: thumbnail generation trigger (completes synchronously; no job polling) ---

export function useGenerateThumbnails(id: string) {
  return useMutation({
    mutationFn: () => sendJson("POST", `/archives/${id}/files/thumbnails`),
  })
}

/** Unbound `useGenerateThumbnails` — for Tankoubon reads spanning N member archives. */
export function useGenerateThumbnailsForArchives() {
  return useMutation({
    mutationFn: (ids: string[]) =>
      Promise.all(ids.map((id) => sendJson("POST", `/archives/${id}/files/thumbnails`))),
  })
}

// --- Reader: random archive (not cached — every call must be fresh) ---
export async function fetchRandomArchiveId(): Promise<string | null> {
  const result = await fetchJson<RandomArchivesResponse>("/search/random?count=1")
  return result.data[0]?.arcid ?? null
}

// --- Reader: page-level bookmarks (all query keys share the "bookmarks" prefix) ---

/** This archive's bookmarked pages with resolved filenames. Always a real archive id. */
export function useBookmarksForArchive(archiveId: string | null) {
  return useQuery({
    queryKey: ["bookmarks", "archive", archiveId],
    queryFn: () => fetchJson<BookmarkedPageResponse[]>(`/archives/${archiveId}/bookmarks`),
    enabled: archiveId !== null,
  })
}

/** A Tankoubon's bookmarks merged across members, in global page numbering. */
export function useBookmarksForTankoubon(tankId: string | null) {
  return useQuery({
    queryKey: ["bookmarks", "tankoubon", tankId],
    queryFn: () => fetchJson<TankBookmarkedPageResponse[]>(`/tankoubons/${tankId}/bookmarks`),
    enabled: tankId !== null,
  })
}

/** Paginated `/bookmarks` listing; `sort`/`q` are in the query key. Caller must debounce `q` —
 * this hook does not. */
export function useInfiniteBookmarks(sort: BookmarkSort, q?: string) {
  return useInfiniteQuery({
    queryKey: ["bookmarks", "page", sort, q ?? ""],
    queryFn: ({ pageParam }: { pageParam: string | undefined }) => {
      const params = new URLSearchParams({ sort })
      if (pageParam) params.set("cursor", pageParam)
      if (q) params.set("q", q)
      return fetchJson<BookmarksPageResponse>(`/bookmarks?${params.toString()}`)
    },
    initialPageParam: undefined as string | undefined,
    getNextPageParam: (last: BookmarksPageResponse) => last.next_cursor ?? undefined,
  })
}

/** `archiveId` may be an archive id or `TANK_xxx`; the id decides the route (Tankoubon-global
 * page is resolved to member+local page server-side). */
export function useAddBookmark() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ archiveId, page }: { archiveId: string; page: number }) =>
      sendJson(
        "POST",
        isTankoubonId(archiveId)
          ? `/tankoubons/${archiveId}/bookmarks/${page}`
          : `/archives/${archiveId}/bookmarks/${page}`,
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["bookmarks"] }),
  })
}

/** Same id-decides-the-route shape as `useAddBookmark`. */
export function useRemoveBookmark() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ archiveId, page }: { archiveId: string; page: number }) =>
      sendJson(
        "DELETE",
        isTankoubonId(archiveId)
          ? `/tankoubons/${archiveId}/bookmarks/${page}`
          : `/archives/${archiveId}/bookmarks/${page}`,
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["bookmarks"] }),
  })
}

/** Sets a bookmark's name; `null` or empty/whitespace clears it. */
export function useSetBookmarkName() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ archiveId, page, name }: { archiveId: string; page: number; name: string | null }) =>
      sendJson(
        "PUT",
        isTankoubonId(archiveId)
          ? `/tankoubons/${archiveId}/bookmarks/${page}/name`
          : `/archives/${archiveId}/bookmarks/${page}/name`,
        { name },
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["bookmarks"] }),
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

export const LOG_CATEGORIES = ["general", "shinobu", "plugins", "redis", "http"] as const

export type LogCategory = (typeof LOG_CATEGORIES)[number]

export function useLogLines(category: LogCategory, lines = 100) {
  return useQuery({
    queryKey: ["logs", category, lines],
    queryFn: () => fetchText(`/logs/${category}?lines=${lines}`),
    refetchInterval: POLL_INTERVAL_MS,
  })
}

// --- Background Job Console (native /api/jobs endpoints) ---

function jobsRefetchInterval(query: { state: { data?: JobsResponse } }) {
  const jobs = query.state.data?.jobs ?? []
  const active = jobs.some((job) => job.state === "active")
  return active ? DOWNLOAD_QUEUE_POLL_INTERVAL_MS : POLL_INTERVAL_MS
}

export function useJobs() {
  return useQuery({
    queryKey: ["jobs"],
    queryFn: () => fetchJson<JobsResponse>("/jobs"),
    select: (data) => data.jobs as JobRecord[],
    refetchInterval: jobsRefetchInterval,
    refetchIntervalInBackground: true,
  })
}

/** Multi-select clear — one `DELETE /jobs/{id}` per id; reports succeeded/failed counts. */
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

/** Always clears the full server-side finished+failed set, ignoring client-side filters. */
export function useClearFinishedJobs() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () => sendJson<{ cleared: number }>("DELETE", "/jobs?state=finished"),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["jobs"] }),
  })
}

// --- Upload page's persistent download queue (SSE-updated, not polled) ---

/** Partial set-if-present patch for an `update` delta (flat, unlike `add`'s whole `item`). */
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
  | {
      kind: "progress"
      id: string
      job_id: string
      downloaded_bytes: number
      total_bytes: number | null
    }

/** SSE byte progress, keyed by job ID. Must stay OUTSIDE the `["jobs"]` query cache: writing
 * progress into it bumps `dataUpdatedAt` and starves the real `/jobs` poll for the whole download. */
const jobProgressOverrides = new Map<string, { downloaded_bytes: number; total_bytes?: number }>()
const jobProgressListeners = new Set<() => void>()

function setJobProgressOverride(jobId: string, downloaded_bytes: number, total_bytes?: number) {
  jobProgressOverrides.set(jobId, { downloaded_bytes, total_bytes })
  for (const listener of jobProgressListeners) listener()
}

/** Latest SSE-pushed progress for one job; merged over `useJobs()`'s polled values at call site. */
export function useJobProgressOverride(jobId: string | undefined) {
  return useSyncExternalStore(
    (onStoreChange) => {
      jobProgressListeners.add(onStoreChange)
      return () => jobProgressListeners.delete(onStoreChange)
    },
    () => (jobId ? jobProgressOverrides.get(jobId) : undefined),
  )
}

export function useDownloadQueue() {
  const queryClient = useQueryClient()

  // One-shot snapshot; live updates arrive over the SSE stream below.
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
      if (data.kind === "progress") {
        setJobProgressOverride(data.job_id, data.downloaded_bytes, data.total_bytes ?? undefined)
        return
      }
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
            // Replace (never duplicate) if `full` already raced this delta in.
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

/** Manual metadata preview; the updated item arrives via SSE delta, so no invalidation here. */
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
    // Optimistic flip: cancellation is cooperative server-side, so waiting felt laggy.
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

/** Overwrite B with the new download; `insertions` optionally packages B's unique pages into a
 * `.patch.zip` first. */
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

/** Read-only comparison — resolves nothing, so no invalidation. */
export function useCompareQueueItem() {
  return useMutation({
    mutationFn: (id: string) => sendJson<{ result: ComparisonResult }>("POST", `/download_queue/${encodeURIComponent(id)}/compare`),
  })
}

/** Natural-sorted real entry names for one side — anchors are filenames, not indices. */
export function useComparePages(id: string | null, side: "a" | "b") {
  return useQuery({
    queryKey: ["compare-pages", id, side],
    queryFn: () => fetchJson<{ pages: string[] }>(`/download_queue/${encodeURIComponent(id ?? "")}/compare/pages?side=${side}`),
    enabled: id !== null,
  })
}

/** Builds a `.patch.zip` Blob for the user to place manually; installs nothing. */
export function useExportComparePatch() {
  return useMutation({
    mutationFn: ({ id, insertions }: { id: string; insertions: ExportPatchInsertion[] }) =>
      sendJsonForBlob("POST", `/download_queue/${encodeURIComponent(id)}/compare/export-patch`, { insertions }),
  })
}

/** Packs several installed plugins' `.ts` files into one `.zip`. */
export function useExportPluginsBatch() {
  return useMutation({
    mutationFn: (namespaces: string[]) => sendJsonForBlob("POST", "/plugins/export-batch", { namespaces }),
  })
}

/** Keep library archive B, discard the download; always deletes the queue item on success. */
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

/** Cursor-paginated; `keepPreviousData` avoids a flash between pages. */
export function useActivity(filter: ActivityFilter) {
  return useQuery({
    queryKey: ["activity", filter],
    queryFn: () => {
      const params = new URLSearchParams()
      if (filter.cursor) params.set("cursor", filter.cursor)
      if (filter.limit != null) params.set("limit", String(filter.limit))
      if (filter.start_ts != null) params.set("start_ts", String(filter.start_ts))
      if (filter.end_ts != null) params.set("end_ts", String(filter.end_ts))
      if (filter.actors && filter.actors.length > 0) params.set("actor", filter.actors.join(","))
      if (filter.actionTypes && filter.actionTypes.length > 0) {
        params.set("action_type", filter.actionTypes.join(","))
      }
      if (filter.outcomes && filter.outcomes.length > 0) params.set("outcome", filter.outcomes.join(","))
      return fetchJson<ActivityPage>(`/activity?${params.toString()}`)
    },
    placeholderData: keepPreviousData,
  })
}

export function useActivityFacets() {
  return useQuery({
    queryKey: ["activity-facets"],
    queryFn: () => fetchJson<ActivityFacets>("/activity/facets"),
  })
}

export function useDeleteActivityEntry() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => sendJson("DELETE", `/activity/${encodeURIComponent(id)}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["activity"] })
      queryClient.invalidateQueries({ queryKey: ["activity-facets"] })
    },
  })
}

export function useBulkDeleteActivityEntries() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (ids: string[]) =>
      sendJson<{ deleted_count: number }>("DELETE", "/activity", { ids }),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ["activity"] })
      queryClient.invalidateQueries({ queryKey: ["activity-facets"] })
    },
  })
}

export function useActivityRetention() {
  return useQuery({
    queryKey: ["activity-retention"],
    queryFn: () => fetchJson<ActivityRetention>("/activity/retention"),
  })
}

export function useUpdateActivityRetention() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (retentionSecs: number | null) =>
      sendJson("PUT", "/activity/retention", { retention_secs: retentionSecs }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["activity-retention"] }),
  })
}

export function useHoverPageOrder() {
  return useQuery({
    queryKey: ["bookmark-hover-page-order"],
    queryFn: () => fetchJson<HoverPageOrderResponse>("/bookmarks/hover-page-order"),
  })
}

export function useSetHoverPageOrder() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (order: string) => sendJson("PUT", "/bookmarks/hover-page-order", { order }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["bookmark-hover-page-order"] }),
  })
}

export function useOnlyMatchingBookmarks() {
  return useQuery({
    queryKey: ["bookmark-only-matching"],
    queryFn: () => fetchJson<OnlyMatchingBookmarksResponse>("/bookmarks/only-matching"),
  })
}

export function useSetOnlyMatchingBookmarks() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (onlyMatching: boolean) =>
      sendJson("PUT", "/bookmarks/only-matching", { only_matching: onlyMatching }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ["bookmark-only-matching"] }),
  })
}
