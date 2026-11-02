/** Shapes of the sharing API (specs/017-archive-sharing contracts/http-api.md). */

export interface SharePermissions {
  import: boolean
  read: boolean
  download: boolean
}

export type ShareStatus = "active" | "exhausted" | "expired" | "revoked"

export type ShareItemRef = { kind: "archive"; id: string } | { kind: "tankoubon"; id: string }

export interface ShareSummary {
  id: string
  title: string
  status: ShareStatus
  permissions: SharePermissions
  downloads_used: number
  download_limit: number | null
  item_count: number
  created_at: number
  expires_at: number | null
  embed: boolean
  allow_helpers: boolean
  short_code: boolean
  restricted: boolean
  /** The share's code, so the list can show and copy it without a request per row. */
  share_code?: string
}

export interface ResolvedShareItem {
  kind: "archive" | "tankoubon"
  id: string
  title: string | null
  available: boolean
  member_count: number | null
}

export interface ClaimHistoryEntry {
  claim_id: string
  kind: "browser_download" | "import"
  first_address: string
  started_at: number
}

export interface ShareCodeInfo {
  code: string
  text_form: string
  link: string | null
  embed: { iframe: string; element: string } | null
  /** No rendezvous is in use, so importers must reach this instance directly. */
  needs_direct_route?: boolean
}

export interface ShareDetail extends ShareSummary {
  items: ResolvedShareItem[]
  ip_allow: string[]
  ip_first_n: number | null
  claims: ClaimHistoryEntry[]
  helpers: { endpoint_id: string; last_seen: number }[]
  /** Present only in the response that created the share. */
  code?: ShareCodeInfo
}

/** Editable fields; `null` clears an optional one. */
export interface ShareFields {
  title?: string | null
  permissions?: SharePermissions
  download_limit?: number | null
  ip_allow?: string[]
  ip_first_n?: number | null
  expires_at?: number | null
  embed?: boolean
  allow_helpers?: boolean
}

export interface NewShare extends ShareFields {
  items: ShareItemRef[]
  short_code?: boolean
}

export interface ArchiveEntry {
  archive_id: string
  title: string
  tags: string
  summary: string
  pagecount: number
  filename: string
  size: number
  /** The sharer's static category names; only in a claim manifest. */
  categories?: string[]
  /** Chapters: 1-based first page and name (a library preset identifier or free text). */
  toc?: { page: number; name: string }[]
}

/** Rust's externally tagged `ManifestItem`. */
export type ManifestItem =
  | { Archive: ArchiveEntry }
  | { Tankoubon: { id: string; name: string; tags: string; summary: string; members: ArchiveEntry[] } }
  | { Unavailable: { title: string | null } }

export interface ShareManifest {
  share_id: string
  title: string
  permissions: SharePermissions
  items: ManifestItem[]
}

export interface RecipientView {
  manifest: ShareManifest
  permissions: SharePermissions
  status: ShareStatus
}

/** `POST /api/shares/imports/resolve`: what the importer is shown before claiming (FR-018). */
export interface ImportPreview {
  preview_id: string
  manifest: ShareManifest
  permissions: SharePermissions
  archive_count: number
  total_size: number
  already_present: string[]
  /** Whether the sharer lets other instances help deliver; absent means no (older instances). */
  allow_helpers?: boolean
  /** Archive ids whose filename a local archive already owns (content is new, name is not). */
  filename_conflicts?: string[]
}

/** What an import does with an archive whose filename another local archive owns. */
export type FilenamePolicy = "rename" | "skip" | "overwrite"

/// This instance's sharing engine: the protocol epoch and the transport generation underneath it.
/// Two instances must match on every field before an import between them can work.
export interface ShareEngine {
  protocol: number
  transport: string
}

/** `GET /api/shares/settings`: every `share_*` field, typed, plus read-only status. */
export type ShareSettings = Record<string, unknown> & { share_engine?: ShareEngine }

export interface ShareEndpointStatus {
  running: boolean
  endpoint_id?: string
  relays?: string[]
  direct_addresses?: string[]
  proxy?: string | null
}

export interface ShareServiceStatus {
  url: string
  kind: string
  reachable: boolean | null
  last_ok_at: number | null
}

export type HelpState = "inactive" | "active" | "paused" | "sharer_offline" | "ended"

export type HelpReason = "exhausted" | "revoked" | "expired" | "helping_disabled"

/** One row of `GET /api/shares/imports?helping=1` (the helper side of US7). */
export interface HelpingImport {
  id: string
  title: string
  sharer_endpoint_id: string
  help_deliver: boolean
  help_state: HelpState
  help_reason: HelpReason | null
  help_state_at: number | null
  help_last_contact_at: number | null
  /** How many of the share's archives the sharer has removed (no longer served). */
  help_removed_archives: number
  help_bytes_served: number
}
