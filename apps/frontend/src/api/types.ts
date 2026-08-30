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
  /** Whether a sidecar `.patch.zip` currently exists next to this archive (issue #77's own
   * follow-on design) — additive, LANrurugi-only field alongside the legacy-pinned ones above,
   * not present on a synthetic Tankoubon entry (`archive_count !== null`). */
  has_patch?: boolean
}

/** `DELETE /archives` (issue #63) — per-id outcome of a batch delete, so the caller can show
 * exactly which archives failed instead of assuming the whole batch either fully succeeded or
 * fully failed. */
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
  /** 007-guest-restricted-access: whether an unauthenticated guest visitor (when the site-wide
   *  `guestmode` setting is on) can see archives belonging to this category — `0`/`1`, matching
   *  `pinned`'s own integer-boolean convention. */
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
  /** Reader recommendation-cache precision (`"low"` / `"medium"` / `"high"`) — how many
   *  candidates the background precompute job keeps per archive's cached similar-archive list.
   *  Changing this triggers a full background rebuild (see `lanrurugi_api::settings::put_settings`). */
  recommendprecision: string
  /** JWT access-token lifetime, in seconds — the short-lived cookie a login issues; a request
   *  past this age transparently refreshes rather than failing (see `api/client.ts`'s own
   *  refresh-then-retry interceptor). Issue #44. */
  access_token_lifetime_secs: number
  /** Refresh-token lifetime, in seconds — the longer-lived, rotating, revocable cookie that
   *  actually re-issues access tokens. See `lanrurugi_storage::refresh_tokens`'s own docs for the
   *  rotation/reuse-detection semantics. Issue #44. */
  refresh_token_lifetime_secs: number
  pagesize: number
  tempmaxsize: number
  sizethreshold: number
  readerquality: number
  webpquality: number
  enablecors: boolean
  localprogress: boolean
  authprogress: boolean
  /** issue #97: placing a stamp on a not-yet-bookmarked page also bookmarks it. */
  stampautobookmark: boolean
  /** issue #97: removing a page's last remaining stamp also removes that page's bookmark — only
   *  meaningful while `stampautobookmark` is also on. */
  stampautounbookmark: boolean
  /** 007-guest-restricted-access: site-wide guest-mode master switch — see `Category.
   *  visible_to_guest` for the per-category half of this two-layer setting. */
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
  /** Always `true` as of 007-guest-restricted-access — password protection can no longer be
   *  disabled. Kept present (not removed) since third-party clients read this field. */
  has_password: boolean
  /** Now reflects a deploy-time flag, not a Settings-page toggle (`devmode` — which had zero
   *  server-side behavior of its own — is removed entirely). */
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
  // Case-insensitive regex (source only, no delimiters) matched against a full candidate URL —
  // the precise trigger condition for a real download/metadata fetch (the Upload page's own
  // URL-queue grouping). NOT what a domain-ownership lookup should match against — see
  // `domain_match` below for that. `null` when this plugin has no meaningful URL-based routing.
  url_pattern: string | null
  // Bare domains (no scheme, no path, e.g. ["e-hentai.org", "exhentai.org"]) this plugin
  // considers itself the owner of — only for domain-ownership lookups (`findPluginByDomain` in
  // `pages/Upload/shared.tsx`), never for real dispatch (still `url_pattern`'s job). Empty when
  // undeclared, matching the backend's own `Vec<String>` (never `null`).
  domain_match: string[]
  // Persisted display-order position within its own `type` group — `null` when never explicitly
  // set. `GET /plugins/{type}` already returns the list pre-sorted by this, so a caller only needs
  // this field to render a drag handle's current position, not to re-derive the sort.
  priority: number | null
  parameters: Array<{ name: string; desc: string; type?: string }>
  // `specs/006-ai-plugin-wizard` FR-026/FR-027 — self-declared by a wizard-generated plugin's own
  // `pluginInfo()`, never inferred by the host. Absent/false for a hand-written plugin.
  generated_by_wizard?: boolean
}

// One `customargs` element's real type — `PluginInfo.parameters[i].type` decides which: `"bool"`
// is a real `boolean`, `"int"` a real `number`, `"string"` (or absent) a `string`. Carried as its
// real type end to end (settings form → PUT → Redis → plugin's own `exec_*` call) rather than a
// uniform `string[]` a plugin has to parse back out itself.
export type CustomArgValue = string | boolean | number

// `GET/PUT /api/plugins/settings?namespace=...` — a plugin's own persisted custom-parameter
// values (e.g. E-Hentai login's cookie fields), positionally matching `PluginInfo.parameters`,
// plus (metadata plugins only) legacy's "Run Automatically" toggle. Distinct from the
// download-specific `PluginOptions` below (concurrency/rate-limit/bundling).
export interface PluginSettings {
  customargs: CustomArgValue[]
  enabled: boolean
}

// `PUT`'s own body is a genuine partial update — an absent field leaves that stored value
// untouched, so toggling "Run Automatically" doesn't require resending the current parameter
// values just to avoid clobbering them (and vice versa).
export interface PluginSettingsUpdate {
  customargs?: CustomArgValue[]
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
  // Issue #67: `true` only when the job's own result exceeded the server's `MAX_JOB_RESULT_BYTES`
  // cap — `result` is `null` in that case (dropped, not partially kept), and the Jobs page's
  // detail view should show an explanatory message instead of an empty/missing result.
  result_truncated: boolean
  error: string | null
}

export interface JobsResponse {
  jobs: JobRecord[]
}

// Upload page's persistent, plugin-grouped download queue (additive, no legacy equivalent) —
// mirrors `lanrurugi_storage::download_queue::DownloadQueueItem` field-for-field. Backed by
// Redis so a queued/in-progress item survives a page refresh or a different browser tab; the
// actual download itself is a `JobRecord` (`job_id` below links the two once started).
export type DownloadQueueState =
  | "queued"
  | "starting"
  | "waiting"
  | "downloading"
  | "done"
  | "error"
  | "cancelled"

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
  | { kind: "already_patched"; existing_id: string; filename: string }

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

/** Mirrors `lanrurugi_imgcompare::PageComparison` — one aligned page pair's own sharpness scores
 * (issue #77's AI quality-comparison judgment). `a`/`b` are deliberately not "new"/"old" here
 * either — `POST /download_queue/{id}/compare` (the only caller today) assigns `a` = the staged
 * download, `b` = the existing library archive, but this type itself stays symmetric. */
export interface PageComparison {
  a_page_index: number
  b_page_index: number
  /** This page's own real filename inside the archive (e.g. `"012.jpg"`) — distinct from
   * `ComparisonResult.a_filename`/`b_filename`, which are the whole *archive's* filename. */
  a_filename: string
  b_filename: string
  /** This page's own compressed byte size inside the archive (not the decoded pixel buffer's
   * size). Distinct from `ComparisonResult.a_file_size`/`b_file_size`, which are the whole
   * *archive's* size. */
  a_file_size: number
  b_file_size: number
  a_sharpness: number
  b_sharpness: number
  a_width: number
  a_height: number
  b_width: number
  b_height: number
  /** Maps a point in A's pixel space to the corresponding point in B's — accounts for the two
   * scans having a different crop margin and/or resolution (a common real scenario). Identity
   * (`scale: 1, offset_x: 0, offset_y: 0`) when no reliable alignment was found. */
  crop_alignment: CropAlignment
}

/** Mirrors `lanrurugi_imgcompare::crop_align::CropAlignment` — maps a point in A to the
 * corresponding point in B, in normalized per-own-dimension (UV-texture-style) coordinates: given
 * `a_u = a_px_x / a_width`, `a_v = a_px_y / a_height`, the corresponding B point is
 * `b_u = a_u * scale + offset_x`, `b_v = a_v * scale + offset_y`, then `b_px_x = b_u * b_width`,
 * `b_px_y = b_v * b_height`. */
export interface CropAlignment {
  scale: number
  offset_x: number
  offset_y: number
}

export type ComparisonSide = "a" | "b"

/** Mirrors `lanrurugi_scanner::archive_format::ArchiveEntryInfo` — one raw archive entry (file
 * *or* directory, including empty directories) from a cheap header-only walk. A strict superset
 * of the page list whenever the archive bundles non-image files (readme/torrent/etc) alongside
 * its actual pages, and of the page count whenever it contains empty directories the page list
 * silently skips. Powers the tree-structure popover next to each side's filename in the
 * comparison modal. */
export interface ArchiveEntryInfo {
  name: string
  is_regular_file: boolean
  is_page: boolean
}

/** Mirrors `lanrurugi_imgcompare::UnmatchedPage` — one page with no counterpart on the other side,
 * plus a ready-made default for where a patch inserting it should anchor (the *other* side's own
 * page index — ready to pass straight through as `ExportPatchInsertion.after_filename`'s
 * equivalent once resolved to a real filename via `useComparePages`). `default_insert_after: null`
 * means "insert at the very start." No AI/LLM involved in computing this — see that Rust type's
 * own docs for why the alignment DP's output already encodes it. */
export interface UnmatchedPage {
  page_index: number
  default_insert_after: number | null
}

/** Mirrors `lanrurugi_imgcompare::ComparisonResult`. `likely_different_language` is a separate
 * signal from `recommendation` being absent — see that Rust type's own docs for why "probably two
 * legitimate language editions" and "genuine quality conflict, too close to call" need different
 * UI treatment rather than collapsing into one "no recommendation" case. */
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
  /** Every entry (files and directories, including empty ones) inside each archive — see
   * `ArchiveEntryInfo`'s own docs. */
  a_entries: ArchiveEntryInfo[]
  b_entries: ArchiveEntryInfo[]
  /** A's own pages with no counterpart found in B (issue #77's own follow-on design) — the
   * material for the "keep some of A's extra pages as a patch" flow, read via the existing
   * `.../compare/page?side=a&index=N` endpoint (same indices `samples`' own `a_page_index` uses). */
  a_unmatched_pages: UnmatchedPage[]
  /** The `b`-side mirror of `a_unmatched_pages` — for the symmetric "keep some of B's extra pages
   * even when picking A overall" flow. */
  b_unmatched_pages: UnmatchedPage[]
}

/** Mirrors `lanrurugi_imgcompare::ComparePhase` — which pass produced a `CompareEvent::Sample`.
 * `"coarse"` is the fast pipeline every sample gets first (opens the result view immediately);
 * `"precise"` is a pixel-accurate replacement `crop_alignment` streamed in afterward for whichever
 * samples the coarse pass already flagged as needing a synthetic border — see
 * `useCompareQueueItemStream`'s own docs for how the two get merged into one live sample list. */
export type ComparePhase = "coarse" | "precise"

/** Mirrors `lanrurugi_imgcompare::CompareEvent` — one `GET /download_queue/{id}/compare/stream`
 * SSE message, tagged by `type` so the frontend never has to infer phase from message order (issue
 * #77's own confirmed design: "注意sse的数据里面要进行区分，用flag标明是粗结果还是精结果"). `sample`
 * carries `sample_index` (this sample's stable position in the eventual result's own `samples`
 * array — NOT `a_page_index`/`b_page_index`) so a `"precise"` event can be applied as an in-place
 * replacement of the matching `"coarse"` one. `done` is emitted exactly once, after every `sample`
 * event across both phases, carrying every `ComparisonResult` field except `samples` itself (the
 * frontend has already assembled that incrementally by the time `done` arrives) — see that
 * variant's own docs for why this doubles as the explicit stream-end marker
 * ("并且sse要有结束标记") instead of relying on the connection closing. */
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

/** One insertion group for `POST /download_queue/{id}/compare/export-patch`,
 * `/overwrite`, or `/compare/keep-b` — mirrors `lanrurugi-api`'s own `ExportPatchInsertion`.
 * `after_filename`/`before_filename` are real entry names from the *target*'s own page list (the
 * user picks the anchor visually, resolved via `useComparePages`), `page_indices` are the
 * *source* side's own unmatched page indices (from `ComparisonResult.a_unmatched_pages`/
 * `b_unmatched_pages`) to insert there, in order. Which side is source vs. target depends on the
 * endpoint: exporting/keeping B reads from A onto B; the overwrite flow (keeping A) reads from B
 * onto the new A. */
export interface ExportPatchInsertion {
  after_filename?: string | null
  before_filename?: string | null
  page_indices: number[]
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
  metadata_preview_at: number | null
  error: QueueError | null
  pending_filename_conflict?: PendingFilenameConflict | null
  created_at: number
}

/** `created_at` added by backend's `list_all` sort — pure display-only ordering field. */
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

// `GET /login/status` — deliberately not a field on `ServerInfo` (see `misc.rs`'s doc comment):
// `/info` mirrors legacy's third-party `ServerInfo` OpenAPI schema field-for-field, and
// "am I logged in right now" has no place in that contract, only in our own SPA session.
export interface LoginStatus {
  /** As of 007-guest-restricted-access, means exactly "a valid administrator session exists" —
   *  password login can no longer be disabled, so this no longer conflates "authenticated" with
   *  "the whole instance happens to require no credentials". */
  logged_in: boolean
  /** Drives the homepage's "you're using the default password" warning toast — legacy's own
   * `[% IF usingdefpass %]` (`Controller/Index.pm`). */
  using_default_password: boolean
  /** Whether the site-wide guest-mode switch is on (007-guest-restricted-access) — on its own
   *  doesn't guarantee an unauthenticated visitor sees anything (at least one category also needs
   *  to be `visible_to_guest`), but tells `RouteGuards.tsx`'s `AllowGuest` guard and `Layout.tsx`'s
   *  nav links whether to attempt scoped guest rendering at all instead of redirecting to /login. */
  guest_mode_enabled: boolean
}

// `GET/POST /api/tokens`, `DELETE /api/tokens/{id}` (issue #54) — first-party API token
// management, replacing legacy's single fixed `apikey` mechanism. Mirrors
// `lanrurugi_api::api_tokens::token_json` exactly — never carries the token's own hash or raw
// value except immediately after creation (`ApiTokenCreateResponse` below).
/** Matches `lanrurugi_storage::api_tokens::TokenRole`'s `#[serde(rename_all = "snake_case")]`
 *  wire shape exactly. `admin`: full access except token management itself and the other
 *  session-only-gated actions (see `lanrurugi_api::procedure::require_session`'s own docs).
 *  `guest`: read-only — every non-`GET` request is rejected regardless of endpoint. */
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

/** `POST /api/tokens`'s response — same fields as `ApiToken` plus the raw token value, present
 *  only this once (the server never stores or returns it again after this response). */
export interface ApiTokenCreateResponse extends ApiToken {
  token: string
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

// `GET /bookmarks` — page-level reading bookmarks aggregated by archive. `pages` is ascending;
// the first entry is what a hover preview treats as "the" cover-aligned thumbnail.
export interface BookmarkedArchiveResponse {
  archive: ArchiveMetadata
  pages: number[]
}

export type BookmarkSort = "bookmarked_at" | "title" | "date_added"

/** `GET /bookmarks?sort=...&cursor=...&limit=...`'s own paginated envelope — `next_cursor: null`
 * means this was the last page. */
export interface BookmarksPageResponse {
  entries: BookmarkedArchiveResponse[]
  next_cursor: string | null
}

/** `GET /archives/{id}/bookmarks` — one archive's own bookmarked pages, each with its resolved
 * in-archive filename (`null` if the page no longer corresponds to a real entry, e.g. the archive
 * was re-scanned with fewer pages since the bookmark was added). */
export interface BookmarkedPageResponse {
  page: number
  filename: string | null
  /** Unix seconds this specific page was bookmarked — for `BookmarkHoverGrid`'s own "sort by when
   * bookmarked" option. Distinct from `BookmarkedArchiveResponse`'s archive-level sort key. */
  bookmarked_at: number
  /** issue #97: how many stamps currently sit on this page — `0` when none. */
  stamp_count: number
}

// `/activity*` (issue #87) — structured, persisted operator activity records, distinct from the
// unstructured `tracing`-based request log. Matches `lanrurugi_storage::activity::ActivityEntry`.
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
  /** Whether this target still exists (checked live against the database at list-fetch time) —
   * only ever set for `archive`/`tankoubon` kinds (see `activity.rs::target_exists`'s own docs),
   * `undefined` for every other kind (not applicable, not "unknown"). `false` means the resource
   * was deleted by some *other*, unrelated action after this entry was written (a rating/metadata
   * update on an archive later deleted) — the row still renders its own historical content, just
   * with its title struck through and no longer a live link. */
  exists?: boolean
}

export interface ActivityCausedBy {
  reason: string
  source_entry_id: string | null
  description: string
}

// Matches `lanrurugi_storage::activity::Outcome`'s own `#[serde(tag = "status", rename_all =
// "snake_case")]` internally-tagged shape — a real, discriminated union on `status`, not two
// separate optional fields, so a `Failure` entry's `reason` is only ever reachable once the caller
// has already narrowed on `status === "failure"`.
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
  /** OR'd together — matches the backend's own `ActivityFilter::actor_keys` semantics (a single
   *  entry has exactly one actor, so this can only ever be "match any of", never "match all of"). */
  actors?: string[]
  /** OR'd together — same reasoning as `actors` above. */
  actionTypes?: string[]
  /** `"success"` | `"failure"`, OR'd together — same reasoning as `actors`/`actionTypes` above. */
  outcomes?: string[]
}

export interface ActivityRetention {
  retention_secs: number | null
}

export interface HoverPageOrderResponse {
  order: string | null
}

/** How to reconcile an archive already present on this instance with the same archive's record
 *  in the LANraragi backup JSON being imported — see `Backup.tsx`'s own docs on why `"merge"` is
 *  the default (least information loss of the three). */
export type ImportConflictMode = "overwrite" | "merge" | "skip"

/** Response shape for `POST /database/import-legacy` (`lanrurugi_backup::import_legacy::
 *  ImportLegacySummary`, plus the shared `rebuild` result `run_rebuild_sequence` also produces
 *  for `POST /database/rebuild-index`) — a real file upload, not a remote Redis connection; see
 *  `import_legacy.rs`'s own module docs for why an earlier "connect to a remote Redis" design was
 *  rejected (the official LANraragi Docker image never exposes its bundled Redis externally). */
export interface ImportLegacyResult {
  archives_updated: number
  archives_skipped_already_exists: number
  archives_skipped_no_match: number
  /** A legacy record's basename matched more than one archive here — excluded rather than
   *  guessed, per the "accuracy over recall" requirement this feature was built around. */
  archives_ambiguous_match: number
  /** More than one *distinct* legacy record independently resolved (by basename) to the same
   *  archive here — purely informational, doesn't change what gets written. */
  archives_multiple_legacy_records_same_target: number
  titles_mojibake_repaired: number
  categories_restored: number
  categories_skipped_already_exists: number
  tankoubons_restored: number
  tankoubons_skipped_already_exists: number
  stamps_restored: number
  stamps_skipped_already_exists: number
  /** ids of brand-new category/tankoubon/stamp records this import created — these have no prior
   *  version on this instance, so the accompanying snapshot (differential-apply, never deletes)
   *  can't roll them back; a full "undo this import" via the snapshot won't remove them. */
  new_category_ids: string[]
  new_tankoubon_ids: string[]
  new_stamp_ids: string[]
  /** Absent when nothing was actually written (every archive/category/tankoubon/stamp was
   *  skipped, unmatched, or ambiguous) — the backend skips the rebuild entirely in that case
   *  rather than running one over an unchanged library. */
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
  /** Present only when the import failed partway through (see `import_from_legacy`'s own docs on
   *  why this isn't transactional) — `true` if the summary/counts above reflect a partial import,
   *  not a completed one. */
  partial?: boolean
  /** Running count of LANraragi imports this instance has ever completed (including this one) —
   *  same value `GET /database/import-legacy/count` returns, included here too so a caller
   *  polling this job's result doesn't need a second request just to know whether to warn about
   *  the *next* import. */
  import_count: number
}

/** One row of `GET /database/import-snapshots` — a Time-Machine-style rollback point
 *  `queue_import_legacy` captured automatically (the pre-write state of every record that
 *  import actually overwrote), listed newest first. No `document` payload here — see
 *  `useDownloadImportSnapshot`'s own docs for why the full backup JSON is fetched separately,
 *  only when the user actually clicks download. */
export interface ImportSnapshotMetadata {
  id: string
  created_at: number
  archive_count: number
  category_count: number
  tankoubon_count: number
  stamp_count: number
}
