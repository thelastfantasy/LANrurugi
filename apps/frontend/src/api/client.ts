import { ApiError, ValidationError } from "./apiError"
import { queryClient } from "./queryClient"
import type { JobStatus } from "./types"

export { ApiError, ValidationError } from "./apiError"

/** `/login`, `/logout`, and `/token/refresh` itself handle their own 401s (a wrong password or an
 * already-dead refresh token, not "the access token merely expired") — every other endpoint's 401
 * first gets one shot at a transparent refresh-then-retry (see `tryRefreshWithRetry`/
 * `shouldAttemptRefresh` below) before falling back to invalidating login status. */
function isAuthBootstrapPath(path: string): boolean {
  return path === "/login" || path === "/logout" || path === "/token/refresh"
}

/** A confirmed-dead session no longer redirects the caller itself (issue #92's own bug: a hard
 * `window.location.assign("/login")` fires *after* this function returns, but doesn't stop the
 * caller's own code from continuing to run and throwing — React gets one more render in first,
 * during which whatever page called this paints its own "request failed" fallback for an instant
 * before the browser actually navigates away). Instead this just marks `/login/status` stale;
 * `RequireAuth` (`RouteGuards.tsx`), mounted on every authenticated route, is the *only* thing
 * that ever navigates to `/login`, and it does so via `<Navigate>` — a real React Router
 * transition, not a full page reload racing against in-flight renders. */
function invalidateLoginStatus() {
  void queryClient.invalidateQueries({ queryKey: ["login-status"] })
}

/** `"ok"` — refresh succeeded, retry the original request. `"rejected"` — the server was actually
 * reached and said no (dead/reused/expired refresh token): the session really is over. `"network-
 * error"` — the `fetch` itself never got a response (offline, DNS hiccup, dev-container mid-
 * rebuild); this says nothing about whether the refresh token is still good, so it must never be
 * treated the same as `"rejected"` (see `tryRefreshWithRetry` below — issue: a few seconds of
 * container-rebuild unreachability was enough to force a real login-status invalidation and kick
 * the user back to /login even though their 7-day session was still perfectly valid). */
type RefreshOutcome = "ok" | "rejected" | "network-error"

/** Dedupes concurrent 401s into a single real `POST /token/refresh` call — several requests can
 * easily fail together (e.g. the Library page's own archives + categories + stats queries all
 * firing near-simultaneously on an expired access token), and each independently calling refresh
 * would race to rotate the same one-time-use refresh token, guaranteeing every call after the
 * first hits `RotateOutcome::ReuseDetected` and gets its whole session revoked. Every caller
 * within the same in-flight window shares this one promise instead. */
let refreshInFlight: Promise<RefreshOutcome> | null = null

async function tryRefreshOnce(): Promise<RefreshOutcome> {
  if (!refreshInFlight) {
    refreshInFlight = fetch("/api/token/refresh", { method: "POST" })
      .then((r): RefreshOutcome => (r.ok ? "ok" : "rejected"))
      .catch((): RefreshOutcome => "network-error")
      .finally(() => {
        refreshInFlight = null
      })
  }
  return refreshInFlight
}

const REFRESH_NETWORK_RETRY_DELAYS_MS = [500, 1500, 3000]

/** Retries only on `"network-error"` — a transient blip (e.g. the dev container's own few-second
 * rebuild window) shouldn't cost the user their session. A `"rejected"` response is never retried:
 * the server was reached and gave a definitive answer, so retrying can't change it. If every
 * attempt is a network error, this still gives up and reports `"network-error"` rather than
 * silently hanging the caller forever — the caller (`shouldInvalidateLoginStatus`) then treats that
 * as "couldn't confirm either way," not "session confirmed dead". */
async function tryRefreshWithRetry(): Promise<RefreshOutcome> {
  for (const delayMs of REFRESH_NETWORK_RETRY_DELAYS_MS) {
    const outcome = await tryRefreshOnce()
    if (outcome !== "network-error") return outcome
    await sleep(delayMs)
  }
  return tryRefreshOnce()
}

/** Only ever attempted once per original request (`retried` guards this) — a refresh that
 * succeeds but is immediately followed by another 401 means the *new* access token itself is
 * already being rejected for some other reason (e.g. `enablepass` just got flipped, or the
 * refreshed session is invalid some other way), and retrying forever would just hang the caller. */
function shouldAttemptRefresh(path: string, retried: boolean): boolean {
  return !retried && !isAuthBootstrapPath(path)
}

/** A confirmed-dead session (`"rejected"`) invalidates login status, same as before. A run of
 * network errors does not — it means "couldn't reach the server to ask," not "the server said no",
 * so forcing the user back to /login on nothing but a connectivity blip would be exactly the bug
 * this is fixing. The original request still fails (the caller's `throw` below is unchanged) —
 * this only controls whether that failure *also* kicks the user out of their session. */
function shouldInvalidateLoginStatus(outcome: RefreshOutcome): boolean {
  return outcome === "rejected"
}

/** Tries to read a JSON error body (`{error: "..."}`) from the response;
 * falls back to a status-only message if the body isn't valid JSON or lacks `error`. */
async function readErrorBody(response: Response, path: string): Promise<string> {
  const body = await response.json().catch(() => null) as { error?: string } | null
  return body?.error ?? `Request to ${path} failed with ${response.status}`
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

/** JSON-body mutation whose response is a binary file (e.g. `POST
 * /download_queue/{id}/compare/export-patch`'s `.patch.zip` bytes), not JSON — same error-handling
 * shape as {@link sendJson}, but reads the response as a `Blob` and also returns the server-supplied
 * filename (parsed from `Content-Disposition`), so the caller can drive a real "Save As" download
 * with the name the server actually intended rather than a generic placeholder. */
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

/** Form-encoded mutation — most legacy-derived endpoints (categories, metadata, plugins) take
 * `application/x-www-form-urlencoded` bodies, matching their original Mojolicious `req->param`
 * handling, not JSON. */
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
