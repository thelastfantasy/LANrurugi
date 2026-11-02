import axios from "axios"

import { ApiError, recoverSession } from "./client"

/** Chunk size for a resumable upload, matching the server's own 64 MiB body cap with headroom.
 *  Large enough that a multi-GB archive is tens of requests rather than thousands, small enough
 *  that a retried chunk is not a meaningful amount of work to redo. */
export const CHUNK_SIZE = 16 * 1024 * 1024

/** How many times one chunk is retried before the upload is reported as failed. A dropped
 *  connection is the normal case this feature exists for, so a single retry would be pointless. */
export const CHUNK_ATTEMPTS = 4

/** Per-attempt backoff in milliseconds (the last value repeats for any further attempt). */
const BACKOFF_MS = [0, 500, 2000, 5000]

/** How many times the server may report an offset that does not move forward before the upload is
 *  abandoned. Without this a proxy or server that always answers "you are at 0" would have the
 *  client re-send the first chunk forever. */
export const MAX_STALLED_ROUNDS = 8

export interface UploadSession {
  upload_id: string
  offset: number
  file_name: string
  file_size?: number | null
  resumed?: boolean
}

export interface ResumableUploadOptions {
  /** Category to file the archive into, applied at `complete`. */
  catid?: string
  /** Total bytes the server holds for this file — pre-existing (resumed) ones included — so the
   *  caller can render one progress bar across both. */
  onProgress?: (sent: number, total: number, resumedFrom: number) => void
  /** Called once the server's offset is known and before any bytes are sent: the "the server
   *  already has N bytes" handshake, surfaced so the UI can say so. */
  onResume?: (offset: number) => void
  /** Aborts between chunks, and mid-chunk if the in-flight request is cancelled. */
  signal?: AbortSignal
}

/** The key an interrupted upload is found again by: the same file, re-picked in any tab, has the
 *  same name/size/last-modified. Deliberately not a content hash — hashing 2 GB in the browser
 *  costs more than re-sending the first chunk would, and `POST .../session` matches on this key
 *  server-side, so nothing has to be remembered in the browser for a refresh to resume. */
export function resumeKeyFor(file: { name: string; size: number; lastModified: number }): string {
  return `${file.name}:${file.size}:${file.lastModified}`
}

/** The byte range of the next chunk, or `null` when the whole file has been sent. */
export function nextChunk(
  offset: number,
  size: number,
  chunkSize: number = CHUNK_SIZE,
): { start: number; end: number } | null {
  if (offset >= size) return null
  return { start: offset, end: Math.min(offset + chunkSize, size) }
}

/** `Upload-Offset` from a response — the server's authoritative position.
 *
 *  Genuinely case-insensitive, not merely "tolerant of the two spellings axios happens to produce":
 *  header casing is not preserved through every proxy, and `Object.keys` on axios's own
 *  `AxiosHeaders` does not necessarily enumerate what a `.get()` would return — so this tries the
 *  documented accessor first and then scans keys, comparing lowercased. */
export function parseUploadOffset(headers: unknown): number | null {
  if (!headers || typeof headers !== "object") return null
  const record = headers as Record<string, unknown> & { get?: (name: string) => unknown }
  const direct =
    typeof record.get === "function" ? record.get("upload-offset") : undefined
  const scanned = Object.entries(record).find(([key]) => key.toLowerCase() === "upload-offset")?.[1]
  const raw = direct ?? scanned
  const value = typeof raw === "string" ? Number.parseInt(raw, 10) : typeof raw === "number" ? raw : NaN
  return Number.isFinite(value) && value >= 0 ? value : null
}

/** Whether a failed chunk request is worth retrying. Any other `4xx` means the request itself is
 *  wrong (expired session, oversized body), which repeating cannot fix. */
export function isRetryableChunkFailure(status: number): boolean {
  return (
    status === 0 || status === 408 || status === 409 || status === 425 || status === 429 || status >= 500
  )
}

function backoffFor(attempt: number): number {
  return BACKOFF_MS[Math.min(attempt, BACKOFF_MS.length - 1)]
}

function abortError(): ApiError {
  return new ApiError(0, "Upload cancelled.")
}

function throwIfAborted(signal?: AbortSignal) {
  if (signal?.aborted) throw abortError()
}

async function sleepUnlessAborted(ms: number, signal?: AbortSignal) {
  if (ms <= 0) return
  await new Promise<void>((resolve, reject) => {
    const timer = setTimeout(() => {
      signal?.removeEventListener("abort", onAbort)
      resolve()
    }, ms)
    function onAbort() {
      clearTimeout(timer)
      reject(abortError())
    }
    signal?.addEventListener("abort", onAbort, { once: true })
  })
}

function errorMessage(data: unknown, fallback: string): string {
  const body = data as { error?: string; detail?: string } | undefined
  if (!body?.error) return fallback
  return body.detail ? `${body.error}: ${body.detail}` : body.error
}

/** Creates a session, or re-attaches to the live one this caller already started for this file —
 *  the server matches on the client key, so a refreshed page, a second tab, or a browser that lost
 *  its storage all resume the same bytes. */
export async function openUploadSession(
  file: File,
  options: ResumableUploadOptions = {},
): Promise<UploadSession> {
  const body = {
    file_name: file.name || "upload.zip",
    file_size: file.size,
    client_key: resumeKeyFor(file),
  }
  const response = await axios.request<UploadSession>({
    url: "/api/archives/upload/session",
    method: "POST",
    data: body,
    headers: { "Content-Type": "application/json" },
    validateStatus: () => true,
  })
  if (response.status === 401 || response.status === 403) {
    if (await recoverSession()) return openUploadSession(file, options)
  }
  if (response.status < 200 || response.status >= 300) {
    throw new ApiError(response.status, errorMessage(response.data, "Could not start the upload."))
  }
  options.onResume?.(response.data.offset)
  return response.data
}

/** Sends one chunk, retrying transient failures. Resolves with the server's new offset; a `409`
 *  resolves with the server's *actual* offset so the caller rewinds rather than failing. */
async function sendChunk(
  uploadId: string,
  file: File,
  range: { start: number; end: number },
  resumedFrom: number,
  options: ResumableUploadOptions,
): Promise<number> {
  const blob = file.slice(range.start, range.end)
  let lastError: unknown = null

  for (let attempt = 0; attempt < CHUNK_ATTEMPTS; attempt += 1) {
    throwIfAborted(options.signal)
    await sleepUnlessAborted(backoffFor(attempt), options.signal)
    try {
      const response = await axios.request({
        url: `/api/archives/upload/session/${encodeURIComponent(uploadId)}`,
        method: "PUT",
        data: blob,
        headers: { "Content-Type": "application/octet-stream", "Upload-Offset": String(range.start) },
        signal: options.signal,
        onUploadProgress: (event) => {
          options.onProgress?.(range.start + event.loaded, file.size, resumedFrom)
        },
        validateStatus: () => true,
      })

      if (response.status === 401 || response.status === 403) {
        if (await recoverSession()) continue
      }
      if (response.status === 204) {
        // The server's own count is the truth; the requested range's end is only a fallback for a
        // proxy that stripped the header, and the stall guard catches it if that is wrong.
        return parseUploadOffset(response.headers) ?? range.end
      }
      const serverOffset = parseUploadOffset(response.headers)
      if (response.status === 409 && serverOffset !== null) return serverOffset
      const failure = new ApiError(
        response.status,
        errorMessage(response.data, `Chunk upload failed (${response.status}).`),
      )
      if (!isRetryableChunkFailure(response.status)) throw failure
      lastError = failure
    } catch (err) {
      if (options.signal?.aborted) throw abortError()
      if (err instanceof ApiError && !isRetryableChunkFailure(err.status)) throw err
      lastError = err
    }
  }
  throw lastError instanceof Error
    ? lastError
    : new ApiError(0, "Chunk upload failed after several attempts.")
}

/** Uploads `file` in chunks, resuming from whatever the server already holds, then completes the
 *  session and returns the ingest response.
 *
 *  A rejected promise means the whole attempt failed — but the session and its bytes stay on the
 *  server (unless the ingest itself refused them at `complete`), so calling this again resumes. */
export async function uploadResumable<T>(
  file: File,
  options: ResumableUploadOptions = {},
): Promise<T> {
  const session = await openUploadSession(file, options)
  const resumedFrom = session.offset
  let offset = resumedFrom
  options.onProgress?.(offset, file.size, resumedFrom)

  let stalled = 0
  for (;;) {
    const range = nextChunk(offset, file.size)
    if (!range) break
    const next = await sendChunk(session.upload_id, file, range, resumedFrom, options)
    options.onProgress?.(next, file.size, resumedFrom)
    if (next > file.size) {
      throw new ApiError(0, `Server reported more bytes (${next}) than the file has (${file.size}).`)
    }
    // Progress, not merely "a response arrived": an offset that never advances would otherwise
    // resend the same chunk indefinitely.
    if (next <= offset) {
      stalled += 1
      if (stalled >= MAX_STALLED_ROUNDS) {
        throw new ApiError(0, `Upload stalled at ${offset} of ${file.size} bytes.`)
      }
    } else {
      stalled = 0
    }
    offset = next
  }
  if (offset !== file.size) {
    throw new ApiError(0, `Upload ended at ${offset} of ${file.size} bytes.`)
  }

  return completeUpload<T>(session.upload_id, options)
}

/** `POST .../complete` — the one step that cannot be retried blindly, since a success removes the
 *  session. A session-recovery retry is still safe: the refresh happens before the first attempt is
 *  accepted, so a `401` here means nothing was ingested yet. */
async function completeUpload<T>(uploadId: string, options: ResumableUploadOptions): Promise<T> {
  const response = await axios.request<T>({
    url: `/api/archives/upload/session/${encodeURIComponent(uploadId)}/complete`,
    method: "POST",
    data: options.catid ? { catid: options.catid } : {},
    headers: { "Content-Type": "application/json" },
    signal: options.signal,
    validateStatus: () => true,
  })
  if (response.status === 401 || response.status === 403) {
    if (await recoverSession()) return completeUpload<T>(uploadId, options)
  }
  if (response.status < 200 || response.status >= 300) {
    throw new ApiError(response.status, errorMessage(response.data, "Could not finish the upload."))
  }
  return response.data
}
