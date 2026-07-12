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
  type: 'metadata' | 'login' | 'download'
  name: string
  author: string
  description: string
  version: string
  icon: string | null
  oneshot_arg: string | null
  parameters: Array<{ name: string; desc: string }>
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
  result: unknown | null
  error: string | null
}

export interface JobsResponse {
  jobs: JobRecord[]
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
