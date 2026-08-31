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

/** Dedupes concurrent 401s into one refresh call — independent calls would race to rotate the
 * same one-time-use token and revoke the session. */
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

/** Retries only on `"network-error"`; a `"rejected"` response is definitive and never retried. */
async function tryRefreshWithRetry(): Promise<RefreshOutcome> {
  for (const delayMs of REFRESH_NETWORK_RETRY_DELAYS_MS) {
    const outcome = await tryRefreshOnce()
    if (outcome !== "network-error") return outcome
    await sleep(delayMs)
  }
  return tryRefreshOnce()
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
