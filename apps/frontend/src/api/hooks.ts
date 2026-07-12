import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { fetchJson, fetchText, sendForm, sendJson } from './client'
import type {
  ArchiveFilesResponse,
  ArchiveMetadata,
  BookmarkLinkResponse,
  CategoryMetadata,
  DuplicateGroup,
  JobRecord,
  JobsResponse,
  LoginStatus,
  PluginInfo,
  RandomArchivesResponse,
  SearchResponse,
  ServerInfo,
  Settings,
  ShinobuStatus,
  StampedPagesResponse,
  StampsByPageResponse,
  StatTag,
  TankoubonListResponse,
  TankoubonMetadata,
} from './types'

export function useArchives() {
  return useQuery({
    queryKey: ['archives'],
    queryFn: () => fetchJson<ArchiveMetadata[]>('/archives'),
  })
}

export function useCategories() {
  return useQuery({
    queryKey: ['categories'],
    queryFn: () => fetchJson<CategoryMetadata[]>('/categories'),
  })
}

export function useTankoubons() {
  return useQuery({
    queryKey: ['tankoubons'],
    queryFn: () => fetchJson<TankoubonListResponse>('/tankoubons?page=-1'),
  })
}

export function useSettings() {
  return useQuery({
    queryKey: ['settings'],
    queryFn: () => fetchJson<Settings>('/settings'),
  })
}

export function useUpdateSettings() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (partial: Partial<Settings>) => sendJson('PUT', '/settings', partial),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['settings'] }),
  })
}

export function usePlugins(kind: string = 'all') {
  return useQuery({
    queryKey: ['plugins', kind],
    queryFn: () => fetchJson<PluginInfo[]>(`/plugins/${kind}`),
  })
}

export function useStats(minWeight = 1) {
  return useQuery({
    queryKey: ['stats', minWeight],
    queryFn: () =>
      fetchJson<StatTag[]>(
        `/database/stats?minweight=${minWeight}&hide_excluded_namespaces=true`,
      ),
  })
}

export function useArchiveMetadata(id: string | null) {
  return useQuery({
    queryKey: ['archive', id],
    queryFn: () => fetchJson<ArchiveMetadata>(`/archives/${id}/metadata`),
    enabled: id !== null,
  })
}

export function useArchivePages(id: string | null) {
  return useQuery({
    queryKey: ['archive-pages', id],
    queryFn: () => fetchJson<ArchiveFilesResponse>(`/archives/${id}/files`),
    enabled: id !== null,
  })
}

export function useUpdateProgress(id: string | null) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (page: number) => sendJson('PUT', `/archives/${id}/progress/${page}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['archive', id] })
      queryClient.invalidateQueries({ queryKey: ['archives'] })
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
  order?: 'asc' | 'desc'
  /** Pagination cursor — the *index* into the filtered+sorted result set, not a page number
   * (`lanrurugi-api::search`'s own fixed 100-per-page `PAGE_SIZE`). */
  start?: number
}

export function useSearch(options: SearchOptions) {
  const params = new URLSearchParams()
  if (options.filter) params.set('filter', options.filter)
  if (options.category) params.set('category', options.category)
  if (options.sortby) params.set('sortby', options.sortby)
  if (options.order) params.set('order', options.order)
  if (options.start !== undefined) params.set('start', String(options.start))
  return useQuery({
    queryKey: ['search', options],
    queryFn: () => fetchJson<SearchResponse>(`/search?${params.toString()}`),
  })
}

export function useUpdateArchiveMetadata(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (fields: { title?: string; summary?: string; tags?: string }) => {
      const query = new URLSearchParams(
        Object.entries(fields).filter(([, v]) => v !== undefined) as [string, string][],
      )
      return sendJson('PUT', `/archives/${id}/metadata?${query}`)
    },
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['archive', id] })
      queryClient.invalidateQueries({ queryKey: ['archives'] })
      queryClient.invalidateQueries({ queryKey: ['search'] })
    },
  })
}

export function useDeleteArchive() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => sendJson('DELETE', `/archives/${id}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['archives'] }),
  })
}

export function useTankoubon(id: string) {
  return useQuery({
    queryKey: ['tankoubon', id],
    queryFn: () => fetchJson<TankoubonMetadata>(`/tankoubons/${id}`),
    enabled: id !== '',
  })
}

export function useCreateTankoubon() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (name: string) => sendForm<{ tankid: string }>('PUT', '/tankoubons', { name }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['tankoubons'] }),
  })
}

export function useUpdateTankoubon(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: {
      archives?: string[]
      metadata?: { name?: string; summary?: string; tags?: string }
    }) => sendJson('PUT', `/tankoubons/${id}`, body),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['tankoubon', id] })
      queryClient.invalidateQueries({ queryKey: ['tankoubons'] })
    },
  })
}

export function useDeleteTankoubon() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (id: string) => sendJson('DELETE', `/tankoubons/${id}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['tankoubons'] }),
  })
}

export function useAddToTankoubon(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (archiveId: string) => sendJson('PUT', `/tankoubons/${id}/${archiveId}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['tankoubon', id] }),
  })
}

export function useServerInfo() {
  return useQuery({
    queryKey: ['info'],
    queryFn: () => fetchJson<ServerInfo>('/info'),
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
    queryKey: ['update-check', currentVersion],
    queryFn: async (): Promise<UpdateCheckResult | null> => {
      const response = await fetch(
        'https://api.github.com/repos/thelastfantasy/LANrurugi/releases/latest',
      )
      if (!response.ok) return null
      const data = (await response.json()) as { tag_name?: string; html_url?: string }
      if (!data.tag_name || !data.html_url) return null

      const latest = extractVersionNumbers(data.tag_name)
      const current = extractVersionNumbers(currentVersion ?? '')
      if (compareVersions(latest, current) <= 0) return null

      return { latestVersion: data.tag_name, releaseUrl: data.html_url }
    },
    enabled: !debugMode && !!currentVersion,
    staleTime: 60 * 60 * 1000,
    retry: false,
  })
}

export function useLogin() {
  return useMutation({
    mutationFn: (password: string) => sendForm('POST', '/login', { password }),
  })
}

export function useLogout() {
  return useMutation({
    mutationFn: () => sendJson('POST', '/logout'),
  })
}

/** Legacy's `userlogged` (`enable_pass == 0 || session('is_logged')`) — gates admin-only reader
 * UI (Clean Archive Cache, Archive Overview's edit/delete/category panel) and picks which side of
 * the progress-persistence decision tree applies. See `crates/lanrurugi-api/src/login.rs::status`
 * for why this is its own endpoint rather than a field on `ServerInfo`. */
export function useLoginStatus() {
  return useQuery({
    queryKey: ['login-status'],
    queryFn: () => fetchJson<LoginStatus>('/login/status'),
  })
}

export function useChangePassword() {
  return useMutation({
    mutationFn: (password: string) => sendForm('POST', '/settings/password', { password }),
  })
}

export function useShinobuStatus() {
  return useQuery({
    queryKey: ['shinobu'],
    queryFn: () => fetchJson<ShinobuStatus>('/shinobu'),
    refetchInterval: 5000,
  })
}

export function useShinobuAction() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (action: 'stop' | 'restart' | 'rescan') => sendJson('POST', `/shinobu/${action}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['shinobu'] }),
  })
}

export function useClearNewFlags() {
  return useMutation({
    mutationFn: () => sendJson('DELETE', '/database/isnew'),
  })
}

export function useRegenThumbnails() {
  return useMutation({
    mutationFn: (force: boolean) => sendJson('POST', `/regen_thumbs?force=${force}`),
  })
}

export function useDiscardSearchCache() {
  return useMutation({
    mutationFn: () => sendJson('DELETE', '/search/cache'),
  })
}

// --- Reader: stamps (page annotations) ---
// `crates/lanrurugi-api/src/stamps.rs` — full CRUD, verified against legacy's `Model/Stamp.pm`.

export function useStampedPages(id: string | null) {
  return useQuery({
    queryKey: ['stamped-pages', id],
    queryFn: () => fetchJson<StampedPagesResponse>(`/archives/${id}/stamps`),
    enabled: id !== null,
  })
}

export function useStampsForPage(id: string | null, page: number) {
  return useQuery({
    queryKey: ['stamps', id, page],
    queryFn: () => fetchJson<StampsByPageResponse>(`/archives/${id}/stamps/${page}`),
    enabled: id !== null && page > 0,
  })
}

export function useAddStamp(id: string) {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ page, content, position }: { page: number; content: string; position: string }) =>
      sendJson(
        'PUT',
        `/archives/${id}/stamps/${page}?content=${encodeURIComponent(content)}&position=${encodeURIComponent(position)}`,
      ),
    onSuccess: (_data, { page }) => {
      queryClient.invalidateQueries({ queryKey: ['stamps', id, page] })
      queryClient.invalidateQueries({ queryKey: ['stamped-pages', id] })
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
    }: {
      stampId: string
      content?: string
      position?: string
    }) => {
      const params = new URLSearchParams()
      if (content !== undefined) params.set('content', content)
      if (position !== undefined) params.set('position', position)
      return sendJson('PUT', `/stamps/${stampId}?${params.toString()}`)
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['stamps'] }),
  })
}

export function useDeleteStamp() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (stampId: string) => sendJson('DELETE', `/stamps/${stampId}`),
    onSuccess: () => {
      queryClient.invalidateQueries({ queryKey: ['stamps'] })
      queryClient.invalidateQueries({ queryKey: ['stamped-pages'] })
    },
  })
}

// --- Reader: thumbnail generation trigger ---
// `POST /archives/{id}/files/thumbnails` always completes synchronously (pages are decoded on
// demand, not pre-extracted — see the doc comment on `generate_page_thumbnails`), so this is a
// plain mutation with no job-polling needed.

export function useGenerateThumbnails(id: string) {
  return useMutation({
    mutationFn: () => sendJson('POST', `/archives/${id}/files/thumbnails`),
  })
}

// --- Reader: random archive ---
// Legacy's reader just links `<a href="/random">` (plain navigation). Not a react-query hook —
// every call must pick a fresh random archive, there's nothing to cache.
export async function fetchRandomArchiveId(): Promise<string | null> {
  const result = await fetchJson<RandomArchivesResponse>('/search/random?count=1')
  return result.data[0]?.arcid ?? null
}

// --- Reader: bookmark link ---
// `crates/lanrurugi-api/src/categories.rs` — the static category (if any) the reader's bookmark
// icon (`B` key) toggles archive membership in, backed by `LRR_CONFIG`'s `bookmark_link` field.

export function useBookmarkLink() {
  return useQuery({
    queryKey: ['bookmark-link'],
    queryFn: () => fetchJson<BookmarkLinkResponse>('/categories/bookmark_link'),
  })
}

export function useCleanTempfolder() {
  return useMutation({
    mutationFn: () => sendJson('DELETE', '/tempfolder'),
  })
}

export function useCleanDatabase() {
  return useMutation({
    mutationFn: () => sendJson<{ deleted: number; unlinked: number }>('POST', '/database/clean'),
  })
}

export function useDropDatabase() {
  return useMutation({
    mutationFn: () => sendJson('POST', '/database/drop'),
  })
}

export function useDuplicates() {
  return useQuery({
    queryKey: ['duplicates'],
    queryFn: () => fetchJson<DuplicateGroup[]>('/database/duplicates'),
  })
}

export function useScanDuplicates() {
  return useMutation({
    mutationFn: (threshold: number) =>
      sendJson<{ job: string }>('POST', `/database/duplicates/scan?threshold=${threshold}`),
  })
}

export function useClearDuplicates() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () => sendJson('DELETE', '/database/duplicates'),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['duplicates'] }),
  })
}

export const LOG_CATEGORIES = ['general', 'shinobu', 'plugins', 'redis', 'mojo'] as const

export type LogCategory = (typeof LOG_CATEGORIES)[number]

export function useLogLines(category: LogCategory, lines = 100) {
  return useQuery({
    queryKey: ['logs', category, lines],
    queryFn: () => fetchText(`/logs/${category}?lines=${lines}`),
    refetchInterval: 5000,
  })
}

// --- Background Job Console (specs/002-job-console) --------------------------------------------
// Native `/api/jobs` endpoints, additive over the legacy-mimicking `/api/minion/*` contract
// (research.md §1). Polling interval matches the `useShinobuStatus`/`useLogLines` convention
// (research.md §3).

export function useJobs() {
  return useQuery({
    queryKey: ['jobs'],
    queryFn: () => fetchJson<JobsResponse>('/jobs'),
    // `select` unwraps the `{ jobs: [...] }` envelope so consumers get the array directly.
    select: (data) => data.jobs as JobRecord[],
    refetchInterval: 5000,
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
            await sendJson('DELETE', `/jobs/${encodeURIComponent(id)}`)
            succeeded += 1
          } catch {
            failed += 1
          }
        }),
      )
      return { succeeded, failed }
    },
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['jobs'] }),
  })
}

// `useClearFinishedJobs()` is deliberately unscoped (research.md §5 / FR-004): it always targets
// the full server-side set of finished+failed jobs, regardless of any client-side state/name filter
// the admin happens to have applied to the displayed list.
export function useClearFinishedJobs() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () => sendJson<{ cleared: number }>('DELETE', '/jobs?state=finished'),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: ['jobs'] }),
  })
}
