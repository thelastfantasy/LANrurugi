import axios from "axios"

import { SESSION_QUERY_KEY } from "@/session/queryKey"

import { ApiError, ValidationError } from "./apiError"
import { collectClientReportedInfo } from "./clientReportedInfo"
import { queryClient } from "./queryClient"
import type { JobStatus } from "./types"

export { ApiError, ValidationError } from "./apiError"

/** These handle their own 401s; every other endpoint gets a refresh-then-retry first. */
function isAuthBootstrapPath(path: string): boolean {
  return path === "/login" || path === "/logout" || path === "/token/refresh"
}

/** Marks login status stale instead of redirecting directly; `RequireAuth` handles navigation. */
function invalidateLoginStatus() {
  void queryClient.invalidateQueries({ queryKey: SESSION_QUERY_KEY })
}

/** Refetches everything whose result can differ by caller identity, leaving the identity itself
 * (`login-status`) alone. Needed after a *recovery* refresh, not just a login: while the access
 * token sat expired this tab was classified as a guest by the server, so guest-scoped responses
 * can already be cached under the very keys an admin's responses use (see
 * {@link clientClaimsAuthenticatedSession}). Not scoped to specific keys — the set of queries
 * whose results vary by identity isn't closed (a plugin can add new admin-only ones), and a full
 * invalidation is the only invariant that can't silently rot as more are added. */
function invalidateIdentityDependentQueries() {
  void queryClient.invalidateQueries({ predicate: (q) => q.queryKey[0] !== SESSION_QUERY_KEY[0] })
}

/** Whether this tab currently considers itself an admin session. */
function clientClaimsAuthenticatedSession(): boolean {
  return queryClient.getQueryData<{ logged_in?: boolean }>(SESSION_QUERY_KEY)?.logged_in === true
}

/** `403` earns the same refresh-then-retry as `401` **only** for a tab that already believes it is
 * signed in. With guest mode on, an access token that expires while the tab stays open makes every
 * request look like a legitimate `guest_visitor` to the server, so admin-only endpoints answer
 * `403` instead of `401` (issue #99) — a tab whose refresh path only ever watched for `401` then
 * sits in that half-guest state until a full page reload. A genuine guest's `403` never gets here,
 * so it never pays for a refresh attempt it cannot use. */
function isRefreshWorthyStatus(status: number): boolean {
  return status === 401 || (status === 403 && clientClaimsAuthenticatedSession())
}

/** `"network-error"` must never be treated as `"rejected"` — a connectivity blip isn't a dead session. */
type RefreshOutcome = "ok" | "rejected" | "network-error"

/** Dedupes concurrent 401s into one refresh call *within this tab*. Independent tabs still each
 * have their own `refreshInFlight`, which is what {@link tryRefreshOnce}'s `navigator.locks` wrap
 * additionally guards against — see that function's own docs. */
let refreshInFlight: Promise<RefreshOutcome> | null = null

/** Written to `localStorage` (visible to every same-origin tab) after any tab successfully
 * refreshes — lets a tab that's about to take the cross-tab lock notice another tab already did
 * the work and skip redoing it. */
const LAST_REFRESH_AT_KEY = "lanrurugi_last_refresh_at"

/** Called on logout — the timestamp itself carries no identity, but leaving a stale one behind
 * serves no purpose once the session it described is gone. */
export function clearLastRefreshTimestamp() {
  localStorage.removeItem(LAST_REFRESH_AT_KEY)
}

/** Backend rotates the refresh cookie on every use (single-use + reuse detection) — two tabs
 * presenting the *same* stale cookie is forgiven server-side within a short grace window, but two
 * tabs still don't need to both hit the network. `navigator.locks` serializes actual refresh
 * attempts across every same-origin tab; browsers/WebViews without it (some mobile WebViews — the
 * live symptom reported from a phone, 2026-09-04) must not make the whole login-status query
 * reject, so this falls back to each tab refreshing independently, the same pre-locks behavior. */
async function tryRefreshOnce(): Promise<RefreshOutcome> {
  if (refreshInFlight) return refreshInFlight

  const startedAt = Date.now()
  const attemptRefresh = async (): Promise<RefreshOutcome> => {
    // Another tab may have already refreshed while this one was waiting for the lock — if so,
    // its result covers this request too, and presenting the now-rotated-out cookie again would
    // just burn a grace-window slot for nothing.
    const lastRefreshAt = Number(localStorage.getItem(LAST_REFRESH_AT_KEY) ?? 0)
    if (lastRefreshAt > startedAt) return "ok"

    const body = new URLSearchParams(await collectClientReportedInfo())
    return fetch("/api/token/refresh", { method: "POST", body })
      .then((r): RefreshOutcome => {
        if (r.ok) localStorage.setItem(LAST_REFRESH_AT_KEY, String(Date.now()))
        return r.ok ? "ok" : "rejected"
      })
      .catch((): RefreshOutcome => "network-error")
  }

  const withLock = typeof navigator !== "undefined" && typeof navigator.locks?.request === "function"
  const run =
    withLock
      ? Promise.resolve()
          .then(() => navigator.locks.request("lanrurugi-token-refresh", attemptRefresh))
          .catch(() => attemptRefresh())
      : attemptRefresh()
  refreshInFlight = run.finally(() => {
    refreshInFlight = null
  })
  return refreshInFlight
}

const REFRESH_NETWORK_RETRY_DELAYS_MS = [500, 1500, 3000]

/** How long to wait before double-checking a `"rejected"` refresh against `/login/status` — long
 * enough for another tab's own in-flight refresh (and its `Set-Cookie`) to land. */
const REJECTED_REFRESH_RECHECK_DELAY_MS = 300

/** A `"rejected"` refresh in *this* tab doesn't necessarily mean the session is dead — another tab
 * may have refreshed (and rotated the cookie) in the moment between this tab reading its now-stale
 * cookie and this request landing. Confirming against `/login/status`, which reads whatever cookie
 * the browser has *right now*, catches that instead of logging out a still-valid session. */
async function recheckLoginStatusAfterRejectedRefresh(): Promise<RefreshOutcome> {
  await sleep(REJECTED_REFRESH_RECHECK_DELAY_MS)
  try {
    const response = await fetch("/api/login/status")
    if (!response.ok) return "rejected"
    const status = (await response.json()) as { logged_in?: boolean }
    return status.logged_in ? "ok" : "rejected"
  } catch {
    return "rejected" // can't confirm either way; don't leave the caller hanging indefinitely
  }
}

/** Retries only on `"network-error"`; a `"rejected"` response gets one recheck against
 * `/login/status` (see {@link recheckLoginStatusAfterRejectedRefresh}) before being treated as
 * definitive. */
async function tryRefreshWithRetry(): Promise<RefreshOutcome> {
  for (const delayMs of REFRESH_NETWORK_RETRY_DELAYS_MS) {
    const outcome = await tryRefreshOnce()
    if (outcome === "rejected") return recheckLoginStatusAfterRejectedRefresh()
    if (outcome === "ok") return outcome
    await sleep(delayMs)
  }
  const outcome = await tryRefreshOnce()
  return outcome === "rejected" ? recheckLoginStatusAfterRejectedRefresh() : outcome
}

/** Attempted at most once per request, to avoid hanging if the refreshed token is also rejected. */
function shouldAttemptRefresh(path: string, retried: boolean): boolean {
  return !retried && !isAuthBootstrapPath(path)
}

/** Only a confirmed-dead session invalidates login status, not a mere connectivity blip. */
function shouldInvalidateLoginStatus(outcome: RefreshOutcome): boolean {
  return outcome === "rejected"
}

/** Shared tail of every request helper's `401`/`403` branch: one refresh attempt, then either a
 * retry (the caller re-issues its own request) or — when the refresh proved the session is really
 * gone — a login-status invalidation so `RequireAuth` can route the caller to `/login`. */
async function recoverSession(): Promise<boolean> {
  const outcome = await tryRefreshWithRetry()
  if (outcome === "ok") {
    invalidateIdentityDependentQueries()
    return true
  }
  if (shouldInvalidateLoginStatus(outcome)) invalidateLoginStatus()
  return false
}

/** Reads `{error, detail?, raw_output?}`, appending detail/raw_output when present. */
async function readErrorBody(response: Response, path: string): Promise<string> {
  const body = (await response.json().catch(() => null)) as
    | { error?: string; detail?: string; raw_output?: string }
    | null
  if (!body?.error) return `Request to ${path} failed with ${response.status}`
  const extra = body.detail ?? body.raw_output
  return extra ? `${body.error}: ${extra}` : body.error
}

export async function fetchJson<T>(path: string, retried = false): Promise<T> {
  const response = await fetch(`/api${path}`)

  if (!response.ok) {
    if (isRefreshWorthyStatus(response.status)) {
      if (shouldAttemptRefresh(path, retried)) {
        if (await recoverSession()) return fetchJson<T>(path, true)
      } else if (response.status === 401) {
        invalidateLoginStatus()
      }
    }
    throw new ApiError(response.status, await readErrorBody(response, path))
  }

  return (await response.json()) as T
}

/** `GET /login/status` itself is on every role's allow-list (including `guest_visitor` and
 * `anonymous`) so `RequireAuth`/`RequireGuest`/`AllowGuest` can resolve *before* any protected
 * route is ever hit — which means an expired admin's access token cookie produces a plain 200
 * `{logged_in: false}` here, never a 401. The 401-only refresh path above (`fetchJson` etc.) is
 * consequently never reached for this specific request, and `RequireAuth` would otherwise bounce
 * a still-has-a-valid-refresh-token admin straight to `/login` — confirmed live via issue #99's
 * own repro (guest mode on, access token expired, refresh token still valid: `/login/status`
 * returned `logged_in:false` directly, no 401/403 ever crossed this function).
 *
 * Guest mode is *not* a valid proxy for "has a refresh cookie": an expired access token plus a
 * still-valid refresh cookie looks identical to a fully logged-out caller here, whether guest
 * mode is on or off. The refresh cookie is path-scoped to `/api/token/refresh`, so this request
 * never carries it and cannot ask the server to distinguish the two. Always make one refresh
 * attempt on `logged_in:false`; a genuinely logged-out visitor pays one extra 401 and a short
 * recheck, which is much better than logging out an admin whose refresh token is still good. */
export async function fetchLoginStatusWithRefresh<T extends { logged_in: boolean; guest_mode_enabled: boolean }>(): Promise<T> {
  const status = await fetchJson<T>("/login/status")
  if (status.logged_in) return status
  // A refresh failure must never make the login-status query itself reject. A rejected query turns
  // `RequireAuth` into an optimistic "keep showing admin UI" forever on browsers where the refresh
  // path can throw (e.g. a mobile WebView without `navigator.locks`). Returning the already-known
  // `logged_in: false` lets the normal route guards treat the caller as the guest they currently
  // appear to be, while a supported browser still silently refreshes a valid session below.
  let outcome: RefreshOutcome
  try {
    outcome = await tryRefreshWithRetry()
  } catch {
    return status
  }
  if (outcome !== "ok") return status
  const refreshed = await fetchJson<T>("/login/status")
  // The identity this tab was rendering as just changed from guest to admin. Every other query
  // already in the cache (search results, categories, jobs, ...) may have been fetched *while*
  // this tab still looked like a guest — e.g. `search` scoped to guest-visible archives only —
  // and none of them depend on `login-status` in their own `queryKey`, so nothing else would
  // otherwise notice this transition and refetch. Left alone, the UI tears: an admin-only nav
  // (driven by this query) over guest-scoped content (driven by those stale ones) — confirmed
  // live via issue #99's own repro. Fire-and-forget: awaiting the full invalidation here makes
  // `login-status` itself wait for every identity-dependent query (search, settings, bookmarks,
  // ...) to refetch before the admin UI is allowed to render, which is far too slow on mobile. The
  // strict `RequireAuth`/`Layout` changes already stop the admin chrome from appearing before
  // login-status settles; the invalidation then brings the remaining queries over to the admin
  // view asynchronously.
  if (refreshed.logged_in) invalidateIdentityDependentQueries()
  return refreshed
}

export async function fetchText(path: string, retried = false): Promise<string> {
  const response = await fetch(`/api${path}`)

  if (!response.ok) {
    if (isRefreshWorthyStatus(response.status)) {
      if (shouldAttemptRefresh(path, retried)) {
        if (await recoverSession()) return fetchText(path, true)
      } else if (response.status === 401) {
        invalidateLoginStatus()
      }
    }
    throw new ApiError(response.status, await readErrorBody(response, path))
  }

  return response.text()
}

/** JSON body mutation (PUT/POST/PATCH/DELETE with a JSON-encoded body). */
export async function sendJson<T>(
  method: "PUT" | "POST" | "PATCH" | "DELETE",
  path: string,
  body?: unknown,
  retried = false,
): Promise<T> {
  const response = await fetch(`/api${path}`, {
    method,
    headers: body !== undefined ? { "Content-Type": "application/json" } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })

  if (!response.ok) {
    if (isRefreshWorthyStatus(response.status)) {
      if (shouldAttemptRefresh(path, retried)) {
        if (await recoverSession()) return sendJson<T>(method, path, body, true)
      } else if (response.status === 401) {
        invalidateLoginStatus()
      }
    }
    if (response.status === 422) {
      const errorBody = (await response.json().catch(() => null)) as { error?: string; field?: string } | null
      if (errorBody?.error && errorBody.field) throw new ValidationError(errorBody.error, errorBody.field)
    }
    throw new ApiError(response.status, await readErrorBody(response, path))
  }

  const text = await response.text()
  return (text ? JSON.parse(text) : undefined) as T
}

/** Like {@link sendJson}, but for a binary response — returns a `Blob` plus the filename parsed
 * from `Content-Disposition`. */
export async function sendJsonForBlob(
  method: "PUT" | "POST" | "PATCH" | "DELETE",
  path: string,
  body?: unknown,
  retried = false,
): Promise<{ blob: Blob; filename: string | null }> {
  const response = await fetch(`/api${path}`, {
    method,
    headers: body !== undefined ? { "Content-Type": "application/json" } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })

  if (!response.ok) {
    if (isRefreshWorthyStatus(response.status)) {
      if (shouldAttemptRefresh(path, retried)) {
        if (await recoverSession()) return sendJsonForBlob(method, path, body, true)
      } else if (response.status === 401) {
        invalidateLoginStatus()
      }
    }
    throw new ApiError(response.status, await readErrorBody(response, path))
  }

  const disposition = response.headers.get("Content-Disposition")
  const match = disposition?.match(/filename="([^"]+)"/)
  return { blob: await response.blob(), filename: match?.[1] ?? null }
}

/** Form-encoded mutation — legacy-derived endpoints expect this instead of JSON. */
export async function sendForm<T>(
  method: "PUT" | "POST" | "DELETE",
  path: string,
  params: Record<string, string | undefined>,
  retried = false,
): Promise<T> {
  const body = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined) body.set(key, value)
  }
  const response = await fetch(`/api${path}`, { method, body })

  if (!response.ok) {
    if (isRefreshWorthyStatus(response.status)) {
      if (shouldAttemptRefresh(path, retried)) {
        if (await recoverSession()) return sendForm<T>(method, path, params, true)
      } else if (response.status === 401) {
        invalidateLoginStatus()
      }
    }
    throw new ApiError(response.status, `Request to ${path} failed with ${response.status}`)
  }

  return (await response.json()) as T
}

/** Multipart upload with byte-level progress — `fetch` has no upload-progress event at all, so
 * this is the one client function backed by axios (`XMLHttpRequest` under the hood) instead.
 * Same 401-refresh-then-retry-once shape as {@link sendForm}. */
export async function sendFormDataWithProgress<T>(
  method: "PUT" | "POST",
  path: string,
  formData: FormData,
  onProgress?: (loaded: number, total: number) => void,
  retried = false,
): Promise<T> {
  try {
    const response = await axios.request<T>({
      url: `/api${path}`,
      method,
      data: formData,
      onUploadProgress: (event) => {
        if (onProgress && event.total !== undefined) onProgress(event.loaded, event.total)
      },
      validateStatus: () => true,
    })

    if (response.status < 200 || response.status >= 300) {
      if (isRefreshWorthyStatus(response.status)) {
        if (shouldAttemptRefresh(path, retried)) {
          if (await recoverSession()) return sendFormDataWithProgress<T>(method, path, formData, onProgress, true)
        } else if (response.status === 401) {
          invalidateLoginStatus()
        }
      }
      const body = response.data as { error?: string; detail?: string; raw_output?: string } | undefined
      const extra = body?.detail ?? body?.raw_output
      const message = body?.error ? (extra ? `${body.error}: ${extra}` : body.error) : `Request to ${path} failed with ${response.status}`
      throw new ApiError(response.status, message)
    }

    return response.data
  } catch (err) {
    if (err instanceof ApiError) throw err
    throw new ApiError(0, `Request to ${path} failed: ${err instanceof Error ? err.message : String(err)}`)
  }
}

export async function pollJob(jobId: string): Promise<JobStatus> {
  return fetchJson(`/minion/${jobId}`)
}

export function sleep(ms: number) {
  return new Promise((resolve) => setTimeout(resolve, ms))
}

const JOB_POLL_INTERVAL_MS = 400

export async function waitForJob(jobId: string): Promise<JobStatus> {
  let status = await pollJob(jobId)
  while (status.state === "inactive" || status.state === "active") {
    await sleep(JOB_POLL_INTERVAL_MS)
    status = await pollJob(jobId)
  }
  return status
}
