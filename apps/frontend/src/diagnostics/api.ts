/** TanStack Query hooks for the diagnostics surface (`/api/diagnostics*`, specs/018). */

import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query"

import { fetchJson, sendJson, sendJsonForBlob } from "@/api/client"

export interface DiagnosticsSettings {
  diagnostics_enabled: boolean
  diagnostics_web: boolean
  diagnostics_server: boolean
  diagnostics_plugins: boolean
  diagnostics_breadcrumbs: boolean
  diagnostics_request_bodies: boolean
  diagnostics_include_private_data: boolean
  diagnostics_retention_days: number
  diagnostics_max_events: number
  diagnostics_max_bytes: number
  diagnostics_rate_per_minute: number
  diagnostics_rate_per_hour: number
  tracker_configured: boolean
  tracker_built_in: boolean
  tracker_host: string | null
  tracker_routes: Record<string, string>
  tracker_official: boolean
  tracker_project: string | null
}

export interface DiagnosticsUsage {
  events: number
  issues: number
  bytes: number
}

export interface DiagnosticsDrops {
  retention: number
  rate_limit: number
  too_large: number
  store_error: number
}

export interface ForwardWindow {
  open: boolean
  deadline: number
  remaining_secs: number
  sent: number
  failed: number
  last_error: string | null
}

export interface DiagnosticsOverview {
  settings: DiagnosticsSettings
  usage: DiagnosticsUsage
  drops: DiagnosticsDrops
  window: ForwardWindow
}

export interface IssueRow {
  fingerprint: string
  title: string
  level: "info" | "warning" | "error"
  count: number
  first_seen: number
  last_seen: number
  releases: string[]
  sources: string[]
  state: "open" | "resolved" | "ignored"
  request_id: string | null
  sample_event_id: string | null
}

export interface Breadcrumb {
  at: number
  category: string
  level: string
  message: string
}

export interface EventRow {
  kind: "event"
  id: string
  at: number
  level: string
  source: string
  message: string
  stack: string | null
  fingerprint: string
  request_id: string | null
  release: string | null
  tags: Record<string, string>
  breadcrumbs: Breadcrumb[]
}

export interface LogRow {
  kind: "log"
  category: string
  at: number
  line: number
  level: string
  text: string
}

export interface IssueDetail {
  issue: IssueRow
  events: EventRow[]
  log_excerpt: Record<string, { line: number; at: number; text: string }[]> | null
}

export interface BundleSelection {
  from: number
  to: number
  levels: string[]
  sources: string[]
  issues: string[]
  include_ids: string[]
  exclude_ids: string[]
  categories: {
    server_logs: boolean
    frontend_events: boolean
    environment: boolean
    config: boolean
  }
  truncate: boolean
  resolve_stacks: boolean
}

export interface BundlePreview {
  files: { path: string; bytes: number; entries: number }[]
  total_bytes: number
  over_cap: boolean
  cap_bytes: number
  truncations: string[]
  events: number
  log_lines: number
}

export interface BundleText {
  zh: { title: string; body: string }
  en: { title: string; body: string }
  bundle_name: string
}

export function useDiagnosticsOverview() {
  return useQuery({
    queryKey: ["diagnostics", "overview"],
    queryFn: () => fetchJson<DiagnosticsOverview>("/diagnostics/settings"),
    refetchInterval: 30_000,
  })
}

export function useSaveDiagnosticsSettings() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (body: Record<string, unknown>) =>
      sendJson<{ settings: DiagnosticsSettings }>("PUT", "/diagnostics/settings", body),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ["diagnostics"] }),
  })
}

/** Applies one state to every ticked issue; the list endpoint has no bulk form, and a handful of
 * requests is simpler than a second contract. */
export function useBulkSetIssueState() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async ({
      fingerprints,
      state,
    }: {
      fingerprints: string[]
      state: IssueRow["state"]
    }) => {
      await Promise.all(
        fingerprints.map((fingerprint) =>
          sendJson<{ issue: IssueRow }>(
            "POST",
            `/diagnostics/issues/${encodeURIComponent(fingerprint)}/state`,
            { state },
          ),
        ),
      )
      return fingerprints.length
    },
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ["diagnostics"] }),
  })
}

export function useIssues(params: URLSearchParams) {
  return useQuery({
    queryKey: ["diagnostics", "issues", params.toString()],
    queryFn: () => fetchJson<{ issues: IssueRow[]; total: number }>(`/diagnostics/issues?${params}`),
  })
}

export function useIssueDetail(fingerprint: string | null) {
  return useQuery({
    enabled: fingerprint !== null,
    queryKey: ["diagnostics", "issue", fingerprint],
    queryFn: () =>
      fetchJson<IssueDetail>(`/diagnostics/issues/${encodeURIComponent(fingerprint ?? "")}`),
  })
}

export function useSetIssueState() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: ({ fingerprint, state }: { fingerprint: string; state: IssueRow["state"] }) =>
      sendJson<{ issue: IssueRow }>(
        "POST",
        `/diagnostics/issues/${encodeURIComponent(fingerprint)}/state`,
        { state },
      ),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ["diagnostics"] }),
  })
}

export function useEvents(params: URLSearchParams) {
  return useQuery({
    queryKey: ["diagnostics", "events", params.toString()],
    queryFn: () =>
      fetchJson<{ entries: EventRow[]; log: LogRow[] }>(`/diagnostics/events?${params}`),
  })
}

export function usePreviewBundle() {
  return useMutation({
    mutationFn: (selection: BundleSelection) =>
      sendJson<BundlePreview>("POST", "/diagnostics/bundles/preview", selection),
  })
}

export function useBundleText() {
  return useMutation({
    mutationFn: (selection: BundleSelection) =>
      sendJson<BundleText>("POST", "/diagnostics/bundles/text", selection),
  })
}

export function useDownloadBundle() {
  return useMutation({
    mutationFn: (selection: BundleSelection) =>
      sendJsonForBlob("POST", "/diagnostics/bundles", selection),
  })
}

export function useOpenWindow() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (duration_secs: number) =>
      sendJson<ForwardWindow>("POST", "/diagnostics/window", { duration_secs }),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ["diagnostics"] }),
  })
}

export function useCloseWindow() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () => sendJson<ForwardWindow>("DELETE", "/diagnostics/window"),
    onSuccess: () => void queryClient.invalidateQueries({ queryKey: ["diagnostics"] }),
  })
}

/** Submits a note to the configured tracker as a Sentry `feedback` item. Name and email are
 * required; `client` is the browser-only environment report, `url` the page it was written on, and
 * `images` are screenshots the server re-encodes to WebP before forwarding.
 *
 * Multipart, not JSON: the screenshots are files. `Content-Type` is deliberately left unset so the
 * browser writes its own boundary. */
export async function submitFeedback(input: {
  message: string
  name: string
  email: string
  url?: string
  client?: Record<string, string>
  images?: File[]
  eventId?: string
}): Promise<{ sent: boolean; remaining_today: number }> {
  const form = new FormData()
  form.append("message", input.message)
  form.append("name", input.name)
  form.append("email", input.email)
  if (input.url) form.append("url", input.url)
  if (input.client) form.append("client", JSON.stringify(input.client))
  if (input.eventId) form.append("event_id", input.eventId)
  for (const image of input.images ?? []) form.append("image", image, image.name)
  const response = await fetch("/api/diagnostics/feedback", {
    method: "POST",
    body: form,
  })
  const body = await response.json().catch(() => ({}))
  if (!response.ok) {
    throw new Error(body.error ?? `HTTP ${response.status}`)
  }
  return body as { sent: boolean; remaining_today: number }
}
