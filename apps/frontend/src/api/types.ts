// Shapes match lanrurugi-api's JSON responses, which in turn match the legacy
// `tools/openapi.yaml` schemas (ArchiveMetadataJson / CategoryMetadataJson / TankoubonMetadataJson)
// — constitution Principle II.

export interface TocEntry {
  name: string
  page: number
  /** `true` for auto-generated "archive boundary" entries in Tankoubon mode — these
   *  are NOT real ToC data from the member archive, just visual markers showing where
   *  each member starts. They should not be editable or deletable. */
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
  /** Only present on a synthetic Tankoubon entry within search results (`arcid` starting with
   * `TANK_`, `extension: ".tank"`) — `null` for a real archive. Mirrors legacy's own
   * `build_tank_json` aggregate shape (`~/LANraragi/lib/LANraragi/Utils/Database.pm`). */
  archive_count: number | null
}

export interface CategoryMetadata {
  id: string
  name: string
  pinned: number
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

/** `GET /tankoubons/{id}/full` — same fields as `TankoubonMetadata` plus `full_data`, one
 * `ArchiveMetadata` per entry in `archives`, in the same (reading) order — used to build the
 * concatenated multi-archive page list a Tankoubon "read" session needs (`useTankoubonReading`).
 * `full_data.length` can be `< archives.length`: the backend silently drops any member archive
 * missing from the archive repository (a stale grouping entry) rather than erroring, so a
 * consumer must match entries by their own `arcid`, not assume index alignment with `archives`. */
export interface TankoubonFullResponse {
  result: TankoubonMetadata & { full_data: ArchiveMetadata[] }
  total: number
  filtered: number
}

export interface Settings {
  theme: string
  language: string
  htmltitle: string
  motd: string
  apikey: string
  excludednamespaces: string
  tagrules: string
  /** IANA timezone identifier (e.g. `"Asia/Tokyo"`, `"UTC"`) — see `lanrurugi_search::engine`'s
   * `date_added` date-range handling. Used by the frontend to render `date_added`/`timestamp` tag
   * values as `yyyy-mm-dd` in this timezone (via `Intl.DateTimeFormat({ timeZone })`, which the
   * browser implements natively for any IANA id) and to build same-day search URLs. */
  timezone: string
  /** How long an archive's "new" badge stays visible: `until_opened` (legacy — cleared when the
   * reader loads), `until_finished` (cleared once read to the last page), or `3d`/`7d`/`10d`
   * (a time window from `date_added`). See `lanrurugi_api::archives::effective_isnew`. */
  newbadgemode: string
  /** Whether a DeepSeek API key has been configured (Redis or DEEPSEEK_API_KEY env var).
   *  The real key is never sent to the frontend. */
  llm_api_key_set: boolean
  pagesize: number
  tempmaxsize: number
  sizethreshold: number
  readerquality: number
  webpquality: number
  enablepass: boolean
  nofunmode: boolean
  enablecors: boolean
  localprogress: boolean
  authprogress: boolean
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
  has_password: boolean
  debug_mode: boolean
  nofun_mode: boolean
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
  // Case-insensitive regex (source only, no delimiters) matched against a full candidate URL —
  // used by the Upload page's URL queue to group pasted URLs by download plugin, and (for a
  // metadata plugin) to find the one applicable plugin for a metadata-preview-by-URL action.
  // `null` when this plugin has no meaningful URL-based routing.
  url_pattern: string | null
  // Persisted display-order position within its own `type` group — `null` when never explicitly
  // set. `GET /plugins/{type}` already returns the list pre-sorted by this, so a caller only needs
  // this field to render a drag handle's current position, not to re-derive the sort.
  priority: number | null
  parameters: Array<{ name: string; desc: string; type?: string }>
}

// `GET/PUT /api/plugins/settings?namespace=...` — a plugin's own persisted custom-parameter
// values (e.g. E-Hentai login's cookie fields), positionally matching `PluginInfo.parameters`,
// plus (metadata plugins only) legacy's "Run Automatically" toggle. Distinct from the
// download-specific `PluginOptions` below (concurrency/rate-limit/bundling).
export interface PluginSettings {
  customargs: string[]
  enabled: boolean
}

// `PUT`'s own body is a genuine partial update — an absent field leaves that stored value
// untouched, so toggling "Run Automatically" doesn't require resending the current parameter
// values just to avoid clobbering them (and vice versa).
export interface PluginSettingsUpdate {
  customargs?: string[]
  enabled?: boolean
}

// `GET/PUT/DELETE /api/plugins/options?namespace=...` (specs/005-download-plugin-progress,
// contracts/download-settings-api.md) — a download plugin's effective concurrency/rate-limit/
// bundling settings (its own `pluginOptions()` declared defaults merged with any persisted user
// override). `source` on each field distinguishes an inherited default from a user customization.
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
  // Absent entirely for a single-resource-only plugin (contract: "only meaningful for a plugin
  // whose execDownload can return more than one downloads[] element").
  bundle_as_archive?: EffectiveBundleAsArchive
  // Absent when this plugin declares no opinion on overwrite-on-duplicate — the effective
  // behavior then falls back to the global `Settings.replacedupe` value.
  overwrite_on_duplicate?: EffectiveOverwriteOnDuplicate
}

// `PUT /api/plugins/options`'s request body — a partial update; an omitted field leaves that
// setting at its current effective value (plugin default, or a previously-set override).
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

// Native job-console shape (specs/002-job-console `GET /api/jobs`), distinct from the legacy-
// mimicking `JobStatus` above: real field names straight off `lanrurugi_core::jobs::JobStatus`
// (`serde(rename_all = "snake_case")`), no legacy translation layer (contracts/jobs-api.md).
export type JobRecordState = "queued" | "active" | "finished" | "failed"

export interface JobRecord {
  id: string
  name: string
  state: JobRecordState
  progress: number
  // Present only for a real download-type job once its byte transfer has actually started
  // (specs/005-download-plugin-progress) — genuinely absent from the JSON response, not `null`,
  // for every other job and before that point (contracts/download-settings-api.md's extended job
  // shape). `total_bytes` may stay absent even once `downloaded_bytes` is present (the server
  // didn't report a size) — render an indeterminate indicator in that case, not a 0/NaN percentage.
  downloaded_bytes?: number
  total_bytes?: number
  // Present only for a download-type job once it has started (issue #2), mirroring the backend's
  // `rate_limit_bytes_per_sec`/`rate_limit_matched_pattern`. `rate_limit_bytes_per_sec` absent
  // means unlimited (no matching rule, or the rule declared no cap); both absent for every non-
  // download job. The frontend highlights the speed label and shows a tooltip when a cap is set.
  rate_limit_bytes_per_sec?: number
  rate_limit_matched_pattern?: string
  result: unknown | null
  error: string | null
}

export interface JobsResponse {
  jobs: JobRecord[]
}

// Upload page's persistent, plugin-grouped download queue (additive, no legacy equivalent) —
// mirrors `lanrurugi_storage::download_queue::DownloadQueueItem` field-for-field. Backed by
// Redis so a queued/in-progress item survives a page refresh or a different browser tab; the
// actual download itself is a `JobRecord` (`job_id` below links the two once started).
export type DownloadQueueState = "queued" | "starting" | "downloading" | "done" | "error" | "cancelled"

/** An interpolation value in a `QueueError`'s `data` map — mirrors
 * `lanrurugi_core::queue_error::PluginErrorValue`'s untagged `String | Number` union. */
export type PluginErrorValue = string | number

/** Mirrors `lanrurugi_core::queue_error::QueueError` field-for-field, including its `#[serde(tag
 * = "kind", rename_all = "snake_case")]` wire shape — every download-queue failure the backend
 * can produce, structured (no free-text `detail`) so `QueueErrorText` (`components/`) can map
 * `kind` to a translated string and interpolate each variant's own fields into it. */
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

/** Mirrors `lanrurugi_storage::download_queue::PendingFilenameConflict` — set on a queue item
 * whose download was blocked by a `Filename` collision (content is genuinely new, only the
 * resolved filename collides with an existing archive) and staged to `temp_dir` rather than being
 * discarded, awaiting the user's choice via `POST /download_queue/{id}/overwrite` or `.../rename`.
 * See that Rust type's own docs for why this is distinct from `QueueError::DuplicateArchive`'s
 * `content_hash` case, which never reaches this state at all. */
export interface PendingFilenameConflict {
  temp_path: string
  original_filename: string
  existing_id: string
  crc32: string
}

// `#[serde(default)]` on the Rust side — absent on any record written before this field existed,
// which always means 'download' (the only kind that could have produced them).
export type QueueItemOrigin = "download" | "local_upload"

export interface DownloadQueueItem {
  id: string
  origin?: QueueItemOrigin
  // For a download: the source URL. For a local upload: the uploaded file's own filename — see
  // `origin`.
  url: string
  // Resolved once, client-side, at add-to-queue time and fixed from then on — not re-resolved at
  // start time. For a local upload: the fixed placeholder `'local_upload'`, never a real plugin.
  plugin_namespace: string
  // Known at creation time for a local upload (the request body's byte length) — absent for a
  // download, which instead reports its size live through the linked job's own `total_bytes`
  // (see `JobRecord`).
  file_size?: number | null
  category: string | null
  auto_fetch_metadata: boolean
  overwrite_on_duplicate: boolean
  state: DownloadQueueState
  job_id: string | null
  // Set once a managed download completes successfully — persisted here (not just in the linked
  // job's own ephemeral result) so the completed-item reader link survives a server restart.
  // Absent on items finished before this field existed, or via the unmanaged `file_path` fallback.
  archive_ids?: string[] | null
  title: string | null
  // The metadata plugin's full `execMetadata` response (`{tags?, title?, summary?}`, per
  // `lanrurugi_plugin::protocol::MetadataResult`), set by the "Fetch metadata" preview action —
  // untyped since every plugin's `tags` string uses its own namespace vocabulary (E-Hentai's
  // `artist:`/`uploader:`/`category:`/`timestamp:` mean nothing to a different site's plugin).
  metadata_preview: Record<string, unknown> | null
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

export interface ArchiveFilesResponse {
  job: number
  pages: string[]
}

export interface PageDimensionsResponse {
  dimensions: ({ width: number; height: number } | null)[]
}

export interface SearchResponse {
  data: ArchiveMetadata[]
  recordsTotal: number
  recordsFiltered: number
}

// `GET /login/status` — deliberately not a field on `ServerInfo` (see `misc.rs`'s doc comment):
// `/info` mirrors legacy's third-party `ServerInfo` OpenAPI schema field-for-field, and
// "am I logged in right now" has no place in that contract, only in our own SPA session.
export interface LoginStatus {
  logged_in: boolean
  /** Drives the homepage's "you're using the default password" warning toast — legacy's own
   * `[% IF usingdefpass %]` (`Controller/Index.pm`). */
  using_default_password: boolean
}

// Matches `lanrurugi-api::stamps::StampJson` exactly (constitution Principle II, verified against
// `~/LANraragi/tools/openapi.yaml` + `Model/Stamp.pm`). `position` is a single "x,y" string, not a
// pre-split pair, matching legacy's own storage shape.
export interface StampJson {
  id: string
  position: string
  content: string
  /** A literal emoji character, or a Font Awesome class name prefixed `fa:` (e.g. `fa:fa-heart`)
   * — see the backend's own `Stamp::icon` docs. Empty string means "no custom icon, use the
   * default marker pin," true for every stamp created before this field existed. */
  icon: string
  /** `"x,y,width,height,anchor,color"` (percent x/y/w/h, an 8-way anchor code, `#rrggbb` outline
   * color) — see the backend's own `Stamp::rect` docs. Empty string means this stamp is a plain
   * point with no selection rectangle, true for every stamp created before this field existed. */
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

// `GET/PUT/DELETE /categories/bookmark_link` — the single static category (if any) the reader's
// bookmark icon toggles archive membership in. Empty `category_id` means unconfigured.
export interface BookmarkLinkResponse {
  operation: string
  success: number
  category_id: string
  error?: string
}
