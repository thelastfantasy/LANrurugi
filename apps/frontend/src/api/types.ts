// Shapes match lanrurugi-api's JSON responses, mirroring legacy's openapi.yaml schemas.

/** One table-of-contents entry. */
export interface TocEntry {
  name: string
  page: number
  /** `true` for auto-generated archive-boundary markers in Tankoubon mode — not real ToC
   *  data, not editable or deletable. */
  synthetic?: boolean
}

export interface ArchiveMetadata {
  arcid: string
  title: string
  filename: string
  tags: string
  summary: string | null
  isnew: boolean
  extension: string
  progress: number
  pagecount: number
  lastreadtime: number
  size: number
  toc: TocEntry[]
  /** Only present on a synthetic Tankoubon entry (`arcid` starting with `TANK_`); `null`
   * otherwise. */
  archive_count: number | null
  /** Whether a sidecar `.patch.zip` exists; not present on a synthetic Tankoubon entry. */
  has_patch?: boolean
}

/** `DELETE /archives` — per-id outcome of a batch delete. */
export interface BatchDeleteResult {
  id: string
  success: boolean
  filename?: string
  error?: string
}

export interface BatchDeleteArchivesResponse {
  deleted: number
  total: number
  results: BatchDeleteResult[]
}

export interface CategoryMetadata {
  id: string
  name: string
  pinned: number
  /** Whether a guest visitor can see this category's archives (`0`/`1`), when guest mode is on. */
  visible_to_guest: number
  search: string | null
  archives: string[]
}

export interface TankoubonMetadata {
  id: string
  name: string
  summary: string
  tags: string
  archives: string[]
  progress: number
  chapter_names: { id: string; name: string }[]
}

export interface TankoubonListResponse {
  result: TankoubonMetadata[]
  total: number
  filtered: number
}

/** `GET /tankoubons/{id}/full` — like `TankoubonMetadata` plus `full_data`. `full_data.length`
 * can be `< archives.length` (stale entries dropped); match by `arcid`, not index. */
export interface TankoubonFullResponse {
  result: TankoubonMetadata & { full_data: ArchiveMetadata[] }
  total: number
  filtered: number
}

export interface Settings {
  theme: string
  /** Theme shown to an unauthenticated `guest_visitor`, independent of the admin's own `theme`. */
  guest_theme: string
  language: string
  htmltitle: string
  motd: string
  excludednamespaces: string
  tagrules: string
  /** IANA timezone id (e.g. `"Asia/Tokyo"`) used to render date-tag values and build
   *  same-day search URLs. */
  timezone: string
  /** How long a "new" badge stays visible: `until_opened`, `until_finished`, or `3d`/`7d`/`10d`. */
  newbadgemode: string
  /** Whether a DeepSeek API key is configured; the real key is never sent to the frontend. */
  llm_api_key_set: boolean
  /** Reader recommendation-cache precision (`"low"`/`"medium"`/`"high"`); changing it triggers
   *  a full background rebuild. */
  recommendprecision: string
  /** JWT access-token lifetime in seconds; an expired request transparently refreshes. */
  access_token_lifetime_secs: number
  /** Refresh-token lifetime in seconds — the longer-lived, rotating, revocable cookie. */
  refresh_token_lifetime_secs: number
  pagesize: number
  tempmaxsize: number
  sizethreshold: number
  readerquality: number
  webpquality: number
  enablecors: boolean
  localprogress: boolean
  authprogress: boolean
  /** Placing a stamp on a not-yet-bookmarked page also bookmarks it. */
  stampautobookmark: boolean
  /** Removing a page's last stamp also removes its bookmark (only while `stampautobookmark` is on). */
  stampautounbookmark: boolean
  /** Site-wide guest-mode master switch; see `Category.visible_to_guest` for the per-category half. */
  guestmode: boolean
  enableresize: boolean
  hqthumbpages: boolean
  enablewebp: boolean
  replacedupe: boolean
  tagruleson: boolean
  usedateadded: boolean
  usedatemodified: boolean
  replacetitles: boolean
}

export interface ServerInfo {
  name: string
  motd: string
  version: string
  version_name: string
  version_desc: string
  /** Always `true` — password protection can no longer be disabled; kept for third-party clients. */
  has_password: boolean
  /** Reflects a deploy-time flag, not a Settings-page toggle. */
  debug_mode: boolean
  archives_per_page: number
  server_resizes_images: boolean
  server_tracks_progress: boolean
  authenticated_progress: boolean
  total_pages_read: number
  total_archives: number
  cache_last_cleared: number
  excluded_namespaces: string[]
}

export interface ShinobuStatus {
  is_alive: 0 | 1
  pid: number
}

export interface DuplicateArchive {
  arcid: string
  title: string
  tags: string
  size: number
  group_key: string
}

export type DuplicateGroup = DuplicateArchive[]

export interface PluginInfo {
  namespace: string
  type: "metadata" | "login" | "download" | "script"
  name: string
  author: string
  description: string
  version: string
  icon: string | null
  oneshot_arg: string | null
  login_from: string | null
  /** Regex matched against a candidate URL to trigger a download/metadata fetch. Not for
   * domain-ownership lookups — see `domain_match`. */
  url_pattern: string | null
  /** Bare domains this plugin owns, used only for domain-ownership lookups, never dispatch. */
  domain_match: string[]
  /** Persisted display-order position within its `type` group; `null` when never set. */
  priority: number | null
  parameters: Array<{ name: string; desc: string; type?: string }>
  /** Self-declared by a wizard-generated plugin; absent/false for a hand-written one. */
  generated_by_wizard?: boolean
}

/** One `customargs` element's real type, per `PluginInfo.parameters[i].type`. */
export type CustomArgValue = string | boolean | number

/** `GET/PUT /api/plugins/settings` — a plugin's persisted custom-parameter values plus the
 * "Run Automatically" toggle. Distinct from the download-specific `PluginOptions` below. */
export interface PluginSettings {
  customargs: CustomArgValue[]
  enabled: boolean
}

/** `PUT`'s body is a partial update — an absent field leaves that stored value untouched. */
export interface PluginSettingsUpdate {
  customargs?: CustomArgValue[]
  enabled?: boolean
}

/** `GET/PUT/DELETE /api/plugins/options` — a download plugin's effective concurrency/rate-limit/
 * bundling settings. `source` on each field distinguishes default from user override. */
export type PluginOptionsSource = "plugin_default" | "user_override"

export interface EffectiveDomainRule {
  pattern?: string
  max_concurrent?: number
  max_bytes_per_sec?: number
  description?: string
  source: PluginOptionsSource
}

export interface EffectiveBundleAsArchive {
  value: boolean
  default: boolean
  description: string
  source: PluginOptionsSource
}

export interface EffectiveOverwriteOnDuplicate {
  value: boolean
  default: boolean
  description: string
  source: PluginOptionsSource
}

export interface PluginOptions {
  namespace: string
  domain_rules: EffectiveDomainRule[]
  /** Absent for a single-resource-only plugin. */
  bundle_as_archive?: EffectiveBundleAsArchive
  /** Absent when the plugin has no opinion; falls back to `Settings.replacedupe`. */
  overwrite_on_duplicate?: EffectiveOverwriteOnDuplicate
}

/** `PUT /api/plugins/options`'s body — a partial update; an omitted field keeps its current value. */
export interface PluginOptionsUpdate {
  domain_rules?: Array<{
    pattern?: string
    max_concurrent?: number
    max_bytes_per_sec?: number
  }>
  bundle_as_archive?: boolean
  overwrite_on_duplicate?: boolean
}

export interface StatTag {
  namespace: string | null
  text: string
  weight: number
}

export interface JobStatus {
  task: string
  state: "inactive" | "active" | "finished" | "failed"
  notes: unknown
  error: string | null
}

/** Native job-console shape, distinct from the legacy-mimicking `JobStatus` above. */
export type JobRecordState = "queued" | "active" | "finished" | "failed"

export interface JobRecord {
  id: string
  name: string
  state: JobRecordState
  progress: number
  /** Present only once a download job's byte transfer has started. */
  downloaded_bytes?: number
  /** May stay absent even with `downloaded_bytes` present — render indeterminate, not 0/NaN. */
  total_bytes?: number
  /** Absent means unlimited (or non-download job). */
  rate_limit_bytes_per_sec?: number
  rate_limit_matched_pattern?: string
  result: unknown | null
  /** `true` when the result exceeded the server's size cap and was dropped (not partially kept). */
  result_truncated: boolean
  error: string | null
}

export interface JobsResponse {
  jobs: JobRecord[]
}

/** Upload page's persistent download queue, backed by Redis so items survive a page refresh. */
export type DownloadQueueState =
  | "queued"
  | "starting"
  | "waiting"
  | "downloading"
  | "done"
  | "error"
  | "cancelled"

/** An interpolation value in a `QueueError`'s `data` map. */
export type PluginErrorValue = string | number

/** Every structured download-queue failure the backend can produce, tagged by `kind` so
 * `QueueErrorText` can map it to a translated, interpolated string. */
export type QueueError =
  | { kind: "plugin_reported"; plugin: string; error_code: string; data: Record<string, PluginErrorValue> }
  | { kind: "plugin_execution_failed"; plugin: string }
  | { kind: "malformed_plugin_response"; plugin: string }
  | { kind: "empty_plugin_result"; plugin: string }
  | { kind: "invalid_url"; url: string }
  | { kind: "invalid_http_method"; method: string }
  | { kind: "http_request_failed"; url: string }
  | { kind: "http_status"; url: string; status: number }
  | { kind: "write_failed" }
  | { kind: "bundle_failed" }
  | { kind: "duplicate_archive"; existing_id: string; reason: "content_hash" | "filename" }
  | { kind: "duplicate_filename"; existing_id: string; filename: string }
  | { kind: "duplicate_filename_cleaned"; existing_id: string; filename: string }
  | { kind: "internal" }
  | { kind: "stale_after_restart" }
  | { kind: "already_patched"; existing_id: string; filename: string }

/** Set on a queue item whose download was blocked by a filename collision (content is new, only
 * the filename collides) and staged pending the user's overwrite/rename choice. */
export interface PendingFilenameConflict {
  temp_path: string
  original_filename: string
  existing_id: string
  crc32: string
}

/** One aligned page pair's sharpness scores. `a`/`b` stay symmetric even though the only current
 * caller assigns `a` = staged download, `b` = existing library archive. */
export interface PageComparison {
  a_page_index: number
  b_page_index: number
  /** This page's own filename inside the archive, distinct from `ComparisonResult`'s archive-level
   * filenames. */
  a_filename: string
  b_filename: string
  /** This page's compressed byte size, distinct from `ComparisonResult`'s archive-level size. */
  a_file_size: number
  b_file_size: number
  a_sharpness: number
  b_sharpness: number
  a_width: number
  a_height: number
  b_width: number
  b_height: number
  /** Maps a point in A's pixel space to B's, accounting for crop/resolution differences.
   * Identity when no reliable alignment was found. */
  crop_alignment: CropAlignment
}

/** Maps a point in A to B in normalized per-dimension coordinates: `b_u = a_u * scale + offset_x`,
 * `b_v = a_v * scale + offset_y`. */
export interface CropAlignment {
  scale: number
  offset_x: number
  offset_y: number
}

export type ComparisonSide = "a" | "b"

/** One raw archive entry (file or directory) from a header-only walk — powers the tree-structure
 * popover in the comparison modal. */
export interface ArchiveEntryInfo {
  name: string
  is_regular_file: boolean
  is_page: boolean
}

/** One page with no counterpart on the other side, plus a default anchor for where a patch
 * inserting it should go. `default_insert_after: null` means insert at the start. */
export interface UnmatchedPage {
  page_index: number
  default_insert_after: number | null
}

/** `likely_different_language` is a separate signal from `recommendation` being absent — "two
 * legitimate editions" and "genuine conflict" need different UI treatment. */
export interface ComparisonResult {
  aligned_pairs: number
  a_total_pages: number
  b_total_pages: number
  likely_different_language: boolean
  recommendation: ComparisonSide | null
  samples: PageComparison[]
  a_filename: string
  b_filename: string
  a_file_size: number
  b_file_size: number
  /** Every entry (files and directories) inside each archive. */
  a_entries: ArchiveEntryInfo[]
  b_entries: ArchiveEntryInfo[]
  /** A's own pages with no counterpart in B — material for the "keep extra pages as patch" flow. */
  a_unmatched_pages: UnmatchedPage[]
  /** The B-side mirror of `a_unmatched_pages`. */
  b_unmatched_pages: UnmatchedPage[]
}

/** Which pass produced a `CompareEvent::Sample` — `"coarse"` opens the result view immediately;
 * `"precise"` streams in a pixel-accurate replacement afterward. */
export type ComparePhase = "coarse" | "precise"

/** One compare-stream SSE message. `sample_index` lets a `"precise"` event replace its matching
 * `"coarse"` one in place; `done` fires once as the stream-end marker. */
export type CompareEvent =
  | { type: "sample"; sample_index: number; phase: ComparePhase; sample: PageComparison }
  | {
      type: "done"
      aligned_pairs: number
      a_total_pages: number
      b_total_pages: number
      a_entries: ArchiveEntryInfo[]
      b_entries: ArchiveEntryInfo[]
      likely_different_language: boolean
      recommendation: ComparisonSide | null
      a_filename: string
      b_filename: string
      a_file_size: number
      b_file_size: number
      a_unmatched_pages: UnmatchedPage[]
      b_unmatched_pages: UnmatchedPage[]
    }

/** One insertion group for the patch/overwrite/keep-b endpoints. `after_filename`/`before_filename`
 * anchor into the target's page list; `page_indices` are the source side's unmatched pages to insert. */
export interface ExportPatchInsertion {
  after_filename?: string | null
  before_filename?: string | null
  page_indices: number[]
}

/** Absent on records written before this field existed, which always means `'download'`. */
export type QueueItemOrigin = "download" | "local_upload"

export interface DownloadQueueItem {
  id: string
  origin?: QueueItemOrigin
  /** Source URL for a download, or the uploaded filename for a local upload. */
  url: string
  /** Resolved once at add-to-queue time. `'local_upload'` placeholder for a local upload. */
  plugin_namespace: string
  /** Known at creation for a local upload; a download reports size via the linked job instead. */
  file_size?: number | null
  category: string | null
  auto_fetch_metadata: boolean
  overwrite_on_duplicate: boolean
  state: DownloadQueueState
  job_id: string | null
  /** Set once a managed download completes, so the reader link survives a server restart. */
  archive_ids?: string[] | null
  title: string | null
  /** The metadata plugin's full response, untyped since `tags` vocabulary varies per plugin. */
  metadata_preview: Record<string, unknown> | null
  metadata_preview_at: number | null
  error: QueueError | null
  pending_filename_conflict?: PendingFilenameConflict | null
  created_at: number
}

export interface DownloadQueueListResponse {
  items: DownloadQueueItem[]
}

export interface AddToQueueItem {
  url: string
  plugin_namespace: string
  category?: string
  auto_fetch_metadata: boolean
  overwrite_on_duplicate: boolean
}

export interface AddToQueueResponse {
  added: DownloadQueueItem[]
  rejected: Array<{ url: string; reason: string }>
}

export interface UpdateQueueItemBody {
  title?: string
  metadata_preview?: Record<string, unknown>
  auto_fetch_metadata?: boolean
  overwrite_on_duplicate?: boolean
}

export interface ArchivePage {
  url: string
  is_patch: boolean
}

export interface ArchiveFilesResponse {
  job: number
  pages: ArchivePage[]
}

export interface PageDimensionsResponse {
  dimensions: ({ width: number; height: number } | null)[]
}

export interface SearchResponse {
  data: ArchiveMetadata[]
  recordsTotal: number
  recordsFiltered: number
}

/** `GET /login/status` — deliberately not a field on `ServerInfo`, which mirrors legacy's
 * third-party OpenAPI schema and has no place for "am I logged in right now". */
export interface LoginStatus {
  /** Means "a valid administrator session exists" — password login can't be disabled. */
  logged_in: boolean
  /** Drives the homepage's "you're using the default password" warning toast. */
  using_default_password: boolean
  /** Whether guest mode is on; doesn't alone guarantee a guest sees anything (needs a
   *  `visible_to_guest` category too). */
  guest_mode_enabled: boolean
}

/** First-party API token management, replacing legacy's single fixed `apikey`. Never carries the
 * token's raw value except immediately after creation (`ApiTokenCreateResponse` below). */
export type TokenRole = "admin" | "guest"

export interface ApiToken {
  id: string
  name: string
  created_at: number
  role: TokenRole
  /** Absolute Unix timestamp this token stops working, or `null` for a permanent token. */
  expires_at: number | null
  last_used_at: number | null
  last_used_ip: string | null
}

/** `POST /api/tokens`'s response — same as `ApiToken` plus the raw token value, present only once. */
export interface ApiTokenCreateResponse extends ApiToken {
  token: string
}

/** `position` is a single "x,y" string, not a pre-split pair, matching legacy's storage shape. */
export interface StampJson {
  id: string
  position: string
  content: string
  /** A literal emoji, or a Font Awesome class prefixed `fa:`. Empty means no custom icon. */
  icon: string
  /** `"x,y,width,height,anchor,color"` (percent x/y/w/h, 8-way anchor, outline color); empty
   * means a plain point with no selection rectangle. */
  rect: string
}

export interface StampedPagesResponse {
  result: string[]
}

export interface StampsByPageResponse {
  result: StampJson[]
}

export interface RandomArchivesResponse {
  data: ArchiveMetadata[]
  recordsTotal: number
}

/** `pages` is ascending; the first entry is treated as the cover-aligned thumbnail. */
export interface BookmarkedArchiveResponse {
  archive: ArchiveMetadata
  pages: number[]
}

export type BookmarkSort = "bookmarked_at" | "title" | "date_added"

/** Paginated envelope; `next_cursor: null` means this was the last page. */
export interface BookmarksPageResponse {
  entries: BookmarkedArchiveResponse[]
  next_cursor: string | null
}

/** `GET /archives/{id}/bookmarks` — one archive's bookmarked pages, `filename: null` if the page
 * no longer corresponds to a real entry. */
export interface BookmarkedPageResponse {
  page: number
  filename: string | null
  /** Unix seconds this page was bookmarked. */
  bookmarked_at: number
  /** How many stamps currently sit on this page — `0` when none. */
  stamp_count: number
  /** Optional user-given name; `null` if never named. */
  name: string | null
}

/** `GET /tankoubons/{id}/bookmarks` — every bookmark across every member archive, translated
 * into the Tankoubon's global page numbering. */
export interface TankBookmarkedPageResponse {
  /** Tankoubon-global page number, not this member's local page. */
  page: number
  /** This member archive's local page number that `page` resolves to. */
  local_page: number
  local_pagecount: number
  /** 0-based position of the member archive in the Tankoubon's reading order. */
  archive_index: number
  archive_id: string
  filename: string | null
  bookmarked_at: number
  stamp_count: number
  /** Optional user-given name; `null` if never named. */
  name: string | null
}

/** Structured, persisted operator activity records, distinct from the unstructured request log. */
export type ActivityActorKind = "session" | "token" | "system" | "anonymous"

export interface ActivityActor {
  kind: ActivityActorKind
  id: string | null
  display_name: string | null
}

export interface ActivityTarget {
  id: string | null
  label: string | null
  kind: string | null
  /** Whether this target still exists (checked live); only set for `archive`/`tankoubon` kinds.
   * `false` renders the row's title struck through and no longer a live link. */
  exists?: boolean
}

export interface ActivityCausedBy {
  reason: string
  source_entry_id: string | null
  description: string
}

/** A discriminated union on `status` — `reason` is only reachable after narrowing to `"failure"`. */
export type ActivityOutcome = { status: "success" } | { status: "failure"; reason: string }

export interface ActivityEntry {
  id: string
  timestamp: number
  actor: ActivityActor
  auto_or_manual: "manual" | "automatic"
  action_type: string
  target: ActivityTarget
  outcome: ActivityOutcome
  client_ip: string | null
  before: unknown | null
  after: unknown | null
  caused_by: ActivityCausedBy | null
}

export interface ActivityPage {
  entries: ActivityEntry[]
  next_cursor: string | null
  total_estimate: number | null
}

export interface ActivityFacetActionType {
  value: string
  count: number
}

export interface ActivityFacetActor {
  kind: ActivityActorKind
  id: string | null
  display_name: string
  count: number
}

export interface ActivityFacets {
  action_types: ActivityFacetActionType[]
  actors: ActivityFacetActor[]
}

export interface ActivityFilter {
  cursor?: string
  limit?: number
  start_ts?: number
  end_ts?: number
  /** OR'd together — a single entry has exactly one actor. */
  actors?: string[]
  /** OR'd together. */
  actionTypes?: string[]
  /** `"success"` | `"failure"`, OR'd together. */
  outcomes?: string[]
}

export interface ActivityRetention {
  retention_secs: number | null
}

export interface HoverPageOrderResponse {
  order: string | null
}

export interface OnlyMatchingBookmarksResponse {
  only_matching: boolean
}

/** How to reconcile an archive already present with its record in the imported backup JSON;
 *  `"merge"` is the default (least information loss). */
export type ImportConflictMode = "overwrite" | "merge" | "skip"

/** Response shape for `POST /database/import-legacy`, a real file upload not a remote Redis
 *  connection (the official LANraragi Docker image never exposes Redis externally). */
export interface ImportLegacyResult {
  archives_updated: number
  archives_skipped_already_exists: number
  archives_skipped_no_match: number
  /** A legacy record's basename matched more than one archive — excluded rather than guessed. */
  archives_ambiguous_match: number
  /** More than one distinct legacy record resolved to the same archive — informational only. */
  archives_multiple_legacy_records_same_target: number
  titles_mojibake_repaired: number
  categories_restored: number
  categories_skipped_already_exists: number
  tankoubons_restored: number
  tankoubons_skipped_already_exists: number
  stamps_restored: number
  stamps_skipped_already_exists: number
  /** ids of brand-new records this import created — the rollback snapshot can't undo these. */
  new_category_ids: string[]
  new_tankoubon_ids: string[]
  new_stamp_ids: string[]
  /** Absent when nothing was written — the backend skips the rebuild in that case. */
  rebuild?: {
    rekeyed: number
    unchanged: number
    missing_file: number
    scanned: number
    newly_catalogued: number
    errors: number
    pagecount_heal: {
      checked: number
      healed: number
      failed: number
      skipped_known_failed: number
    }
  }
  /** `true` if the import failed partway through and this reflects a partial result. */
  partial?: boolean
  /** Running count of LANraragi imports this instance has ever completed, including this one. */
  import_count: number
}

/** One row of `GET /database/import-snapshots` — an automatic rollback point capturing the
 *  pre-write state of every overwritten record, listed newest first. */
export interface ImportSnapshotMetadata {
  id: string
  created_at: number
  archive_count: number
  category_count: number
  tankoubon_count: number
  stamp_count: number
}
