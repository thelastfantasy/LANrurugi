import { ApiError, ValidationError } from "./apiError"
import { queryClient } from "./queryClient"
import type { JobStatus } from "./types"

export { ApiError, ValidationError } from "./apiError"

/** These handle their own 401s; every other endpoint gets a refresh-then-retry first. */
function isAuthBootstrapPath(path: string): boolean {
  return path === "/login" || path === "/logout" || path === "/token/refresh"
}

/** Marks login status stale instead of redirecting directly; `RequireAuth` handles navigation. */
function invalidateLoginStatus() {
  void queryClient.invalidateQueries({ queryKey: ["login-status"] })
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
 * attempts across every same-origin tab; browsers without it (none in this project's target set)
 * would just fall back to each tab refreshing independently, same as before this existed. */
async function tryRefreshOnce(): Promise<RefreshOutcome> {
  if (refreshInFlight) return refreshInFlight

  const startedAt = Date.now()
  refreshInFlight = navigator.locks
    .request("lanrurugi-token-refresh", async (): Promise<RefreshOutcome> => {
      // Another tab may have already refreshed while this one was waiting for the lock — if so,
      // its result covers this request too, and presenting the now-rotated-out cookie again would
      // just burn a grace-window slot for nothing.
      const lastRefreshAt = Number(localStorage.getItem(LAST_REFRESH_AT_KEY) ?? 0)
      if (lastRefreshAt > startedAt) return "ok"

      return fetch("/api/token/refresh", { method: "POST" })
        .then((r): RefreshOutcome => {
          if (r.ok) localStorage.setItem(LAST_REFRESH_AT_KEY, String(Date.now()))
          return r.ok ? "ok" : "rejected"
        })
        .catch((): RefreshOutcome => "network-error")
    })
    .finally(() => {
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
    if (response.status === 401) {
      if (shouldAttemptRefresh(path, retried)) {
        const outcome = await tryRefreshWithRetry()
        if (outcome === "ok") return fetchJson<T>(path, true)
        if (shouldInvalidateLoginStatus(outcome)) invalidateLoginStatus()
      } else {
        invalidateLoginStatus()
      }
    }
    throw new ApiError(response.status, await readErrorBody(response, path))
  }

  return (await response.json()) as T
}

export async function fetchText(path: string, retried = false): Promise<string> {
  const response = await fetch(`/api${path}`)

  if (!response.ok) {
    if (response.status === 401) {
      if (shouldAttemptRefresh(path, retried)) {
        const outcome = await tryRefreshWithRetry()
        if (outcome === "ok") return fetchText(path, true)
        if (shouldInvalidateLoginStatus(outcome)) invalidateLoginStatus()
      } else {
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
    if (response.status === 401) {
      if (shouldAttemptRefresh(path, retried)) {
        const outcome = await tryRefreshWithRetry()
        if (outcome === "ok") return sendJson<T>(method, path, body, true)
        if (shouldInvalidateLoginStatus(outcome)) invalidateLoginStatus()
      } else {
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
    if (response.status === 401) {
      if (shouldAttemptRefresh(path, retried)) {
        const outcome = await tryRefreshWithRetry()
        if (outcome === "ok") return sendJsonForBlob(method, path, body, true)
        if (shouldInvalidateLoginStatus(outcome)) invalidateLoginStatus()
      } else {
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
    if (response.status === 401) {
      if (shouldAttemptRefresh(path, retried)) {
        const outcome = await tryRefreshWithRetry()
        if (outcome === "ok") return sendForm<T>(method, path, params, true)
        if (shouldInvalidateLoginStatus(outcome)) invalidateLoginStatus()
      } else {
        invalidateLoginStatus()
      }
    }
    throw new ApiError(response.status, `Request to ${path} failed with ${response.status}`)
  }

  return (await response.json()) as T
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
