import type { JobStatus } from './types'

export class ApiError extends Error {
  status: number

  constructor(status: number, message: string) {
    super(message)
    this.status = status
  }
}

/** `/login` and `/logout` handle their own 401s (a wrong password, not a stale session) — only
 * every other endpoint's 401 means "you need to (re-)authenticate", so only those redirect. */
function handleUnauthorized(path: string) {
  if (path !== '/login' && path !== '/logout' && !window.location.pathname.startsWith('/login')) {
    window.location.assign('/login')
  }
}

export async function fetchJson<T>(path: string): Promise<T> {
  const response = await fetch(`/api${path}`)

  if (!response.ok) {
    if (response.status === 401) handleUnauthorized(path)
    throw new ApiError(response.status, `Request to ${path} failed with ${response.status}`)
  }

  return (await response.json()) as T
}

export async function fetchText(path: string): Promise<string> {
  const response = await fetch(`/api${path}`)

  if (!response.ok) {
    if (response.status === 401) handleUnauthorized(path)
    throw new ApiError(response.status, `Request to ${path} failed with ${response.status}`)
  }

  return response.text()
}

/** JSON body mutation (PUT/POST/DELETE with a JSON-encoded body). */
export async function sendJson<T>(
  method: 'PUT' | 'POST' | 'DELETE',
  path: string,
  body?: unknown,
): Promise<T> {
  const response = await fetch(`/api${path}`, {
    method,
    headers: body !== undefined ? { 'Content-Type': 'application/json' } : undefined,
    body: body !== undefined ? JSON.stringify(body) : undefined,
  })

  if (!response.ok) {
    if (response.status === 401) handleUnauthorized(path)
    throw new ApiError(response.status, `Request to ${path} failed with ${response.status}`)
  }

  const text = await response.text()
  return (text ? JSON.parse(text) : undefined) as T
}

/** Form-encoded mutation — most legacy-derived endpoints (categories, metadata, plugins) take
 * `application/x-www-form-urlencoded` bodies, matching their original Mojolicious `req->param`
 * handling, not JSON. */
export async function sendForm<T>(
  method: 'PUT' | 'POST' | 'DELETE',
  path: string,
  params: Record<string, string | undefined>,
): Promise<T> {
  const body = new URLSearchParams()
  for (const [key, value] of Object.entries(params)) {
    if (value !== undefined) body.set(key, value)
  }
  const response = await fetch(`/api${path}`, { method, body })

  if (!response.ok) {
    if (response.status === 401) handleUnauthorized(path)
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

export async function waitForJob(jobId: string): Promise<JobStatus> {
  let status = await pollJob(jobId)
  while (status.state === 'inactive' || status.state === 'active') {
    await sleep(400)
    status = await pollJob(jobId)
  }
  return status
}
