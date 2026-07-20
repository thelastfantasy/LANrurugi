// Shapes match lanrurugi-api's JSON responses, which in turn match the legacy
// `tools/openapi.yaml` schemas (ArchiveMetadataJson / CategoryMetadataJson / TankoubonMetadataJson)
// — constitution Principle II.

export interface TocEntry {
  name: string
  page: number
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
}

export interface TankoubonListResponse {
  result: TankoubonMetadata[]
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
  type: 'metadata' | 'login' | 'download' | 'script'
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
  parameters: Array<{ name: string; desc: string }>
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
export type PluginOptionsSource = 'plugin_default' | 'user_override'

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
  state: 'inactive' | 'active' | 'finished' | 'failed'
  notes: unknown
  error: string | null
}

// Native job-console shape (specs/002-job-console `GET /api/jobs`), distinct from the legacy-
// mimicking `JobStatus` above: real field names straight off `lanrurugi_core::jobs::JobStatus`
// (`serde(rename_all = "snake_case")`), no legacy translation layer (contracts/jobs-api.md).
export type JobRecordState = 'queued' | 'active' | 'finished' | 'failed'

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
export type DownloadQueueState = 'queued' | 'starting' | 'downloading' | 'done' | 'error'

export interface DownloadQueueItem {
  id: string
  url: string
  // Resolved once, client-side, at add-to-queue time and fixed from then on — not re-resolved at
  // start time.
  plugin_namespace: string
  category: string | null
  auto_fetch_metadata: boolean
  overwrite_on_duplicate: boolean
  state: DownloadQueueState
  job_id: string | null
  title: string | null
  // The metadata plugin's full `execMetadata` response (`{tags?, title?, summary?}`, per
  // `lanrurugi_plugin::protocol::MetadataResult`), set by the "Fetch metadata" preview action —
  // untyped since every plugin's `tags` string uses its own namespace vocabulary (E-Hentai's
  // `artist:`/`uploader:`/`category:`/`timestamp:` mean nothing to a different site's plugin).
  metadata_preview: Record<string, unknown> | null
  error: string | null
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
}

// Matches `lanrurugi-api::stamps::StampJson` exactly (constitution Principle II, verified against
// `~/LANraragi/tools/openapi.yaml` + `Model/Stamp.pm`). `position` is a single "x,y" string, not a
// pre-split pair, matching legacy's own storage shape.
export interface StampJson {
  id: string
  position: string
  content: string
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
