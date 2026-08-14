import type { JobStatus } from "./types"

export class ApiError extends Error {
  status: number

  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}

/** A `422` field-level validation failure (spec FR-014, e.g. `PUT /plugins/options` rejecting a
 * non-positive `max_concurrent`/`max_bytes_per_sec`) — carries which field failed and why, so a
 * settings form can show an inline error next to the exact input instead of a generic message. */
export class ValidationError extends ApiError {
  field: string

  constructor(message: string, field: string) {
    super(422, message)
    this.field = field
  }
}

/** `/login`, `/logout`, and `/token/refresh` itself handle their own 401s (a wrong password or an
 * already-dead refresh token, not "the access token merely expired") — every other endpoint's 401
 * first gets one shot at a transparent refresh-then-retry (see `tryRefresh`/`shouldAttemptRefresh`
 * below) before falling back to a hard redirect. */
function isAuthBootstrapPath(path: string): boolean {
  return path === "/login" || path === "/logout" || path === "/token/refresh"
}

function redirectToLogin() {
  if (!window.location.pathname.startsWith("/login")) {
    window.location.assign("/login")
  }
}

/** Dedupes concurrent 401s into a single real `POST /token/refresh` call — several requests can
 * easily fail together (e.g. the Library page's own archives + categories + stats queries all
 * firing near-simultaneously on an expired access token), and each independently calling refresh
 * would race to rotate the same one-time-use refresh token, guaranteeing every call after the
 * first hits `RotateOutcome::ReuseDetected` and gets its whole session revoked. Every caller
 * within the same in-flight window shares this one promise instead. */
let refreshInFlight: Promise<boolean> | null = null

async function tryRefresh(): Promise<boolean> {
  if (!refreshInFlight) {
    refreshInFlight = fetch("/api/token/refresh", { method: "POST" })
      .then((r) => r.ok)
      .catch(() => false)
      .finally(() => {
        refreshInFlight = null
      })
  }
  return refreshInFlight
}

/** Only ever attempted once per original request (`retried` guards this) — a refresh that
 * succeeds but is immediately followed by another 401 means the *new* access token itself is
 * already being rejected for some other reason (e.g. `enablepass` just got flipped, or the
 * refreshed session is invalid some other way), and retrying forever would just hang the caller. */
function shouldAttemptRefresh(path: string, retried: boolean): boolean {
  return !retried && !isAuthBootstrapPath(path)
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
      if (shouldAttemptRefresh(path, retried) && (await tryRefresh())) {
        return fetchJson<T>(path, true)
      }
      redirectToLogin()
    }
    throw new ApiError(response.status, await readErrorBody(response, path))
  }

  return (await response.json()) as T
}

export async function fetchText(path: string, retried = false): Promise<string> {
  const response = await fetch(`/api${path}`)

  if (!response.ok) {
    if (response.status === 401) {
      if (shouldAttemptRefresh(path, retried) && (await tryRefresh())) {
        return fetchText(path, true)
      }
      redirectToLogin()
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
      if (shouldAttemptRefresh(path, retried) && (await tryRefresh())) {
        return sendJson<T>(method, path, body, true)
      }
      redirectToLogin()
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
      if (shouldAttemptRefresh(path, retried) && (await tryRefresh())) {
        return sendJsonForBlob(method, path, body, true)
      }
      redirectToLogin()
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
      if (shouldAttemptRefresh(path, retried) && (await tryRefresh())) {
        return sendForm<T>(method, path, params, true)
      }
      redirectToLogin()
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
