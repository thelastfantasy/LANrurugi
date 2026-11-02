import axios from "axios"
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest"

import { CHUNK_SIZE, uploadResumable } from "@/api/uploadSession"

/** The driver's own logic, with axios replaced by a scripted transport: retries, the `409` resync,
 *  resuming from a non-zero offset, and the stall guard are all decisions this module makes, and
 *  none of them are observable from the pure helpers alone. */
vi.mock("axios", () => ({ default: { request: vi.fn() } }))

const request = vi.mocked(axios.request)

function file(size: number, name = "book.zip"): File {
  return new File([new Uint8Array(size)], name, { lastModified: 1_700_000_000_000 })
}

function sessionResponse(offset: number, fileSize: number) {
  return {
    status: 200,
    data: { upload_id: "session-1", offset, file_name: "book.zip", file_size: fileSize, resumed: offset > 0 },
    headers: {},
  }
}

/** A `PUT` chunk response: `204` with the authoritative offset, or an explicit failure. */
function chunkResponse(status: number, offset?: number) {
  return {
    status,
    data: status === 204 ? undefined : { error: "nope" },
    headers: offset === undefined ? {} : { "upload-offset": String(offset) },
  }
}

beforeEach(() => {
  request.mockReset()
})

afterEach(() => {
  vi.useRealTimers()
})

describe("uploadResumable", () => {
  it("sends every chunk at the right offset and completes", async () => {
    const size = CHUNK_SIZE + 5
    const offsets: string[] = []
    request.mockImplementation(async (config) => {
      const url = String(config.url)
      if (url === "/api/archives/upload/session") return sessionResponse(0, size)
      if (url.endsWith("/complete")) return { status: 200, data: { id: "archive-1" }, headers: {} }
      offsets.push(String((config.headers as Record<string, string>)["Upload-Offset"]))
      const start = Number((config.headers as Record<string, string>)["Upload-Offset"])
      return chunkResponse(204, start + (config.data as Blob).size)
    })

    await expect(uploadResumable(file(size), {})).resolves.toEqual({ id: "archive-1" })
    expect(offsets).toEqual(["0", String(CHUNK_SIZE)])
  })

  it("resumes from the offset the server already holds, without re-sending those bytes", async () => {
    const size = CHUNK_SIZE * 2
    const offsets: string[] = []
    request.mockImplementation(async (config) => {
      const url = String(config.url)
      // The server already holds the first chunk (e.g. an interrupted earlier attempt).
      if (url === "/api/archives/upload/session") return sessionResponse(CHUNK_SIZE, size)
      if (url.endsWith("/complete")) return { status: 200, data: { id: "archive-2" }, headers: {} }
      const start = Number((config.headers as Record<string, string>)["Upload-Offset"])
      offsets.push(String(start))
      return chunkResponse(204, start + (config.data as Blob).size)
    })

    await expect(uploadResumable(file(size), {})).resolves.toEqual({ id: "archive-2" })
    expect(offsets).toEqual([String(CHUNK_SIZE)])
  })

  it("retries a transient failure instead of failing the whole upload", async () => {
    const size = 10
    let attempts = 0
    request.mockImplementation(async (config) => {
      const url = String(config.url)
      if (url === "/api/archives/upload/session") return sessionResponse(0, size)
      if (url.endsWith("/complete")) return { status: 200, data: { id: "archive-3" }, headers: {} }
      attempts += 1
      if (attempts === 1) throw new Error("socket hang up")
      return chunkResponse(204, size)
    })

    await expect(uploadResumable(file(size), {})).resolves.toEqual({ id: "archive-3" })
    expect(attempts).toBe(2)
  })

  it("rewinds to the server's own offset after a 409 rather than failing", async () => {
    const size = 20
    const seen: string[] = []
    let first = true
    request.mockImplementation(async (config) => {
      const url = String(config.url)
      if (url === "/api/archives/upload/session") return sessionResponse(0, size)
      if (url.endsWith("/complete")) return { status: 200, data: { id: "archive-4" }, headers: {} }
      const start = Number((config.headers as Record<string, string>)["Upload-Offset"])
      seen.push(String(start))
      if (first) {
        first = false
        // The server already had 5 bytes this client thought it still had to send.
        return chunkResponse(409, 5)
      }
      return chunkResponse(204, start + (config.data as Blob).size)
    })

    await expect(uploadResumable(file(size), {})).resolves.toEqual({ id: "archive-4" })
    expect(seen).toEqual(["0", "5"])
  })

  it("does not retry a failure that repeating cannot fix", async () => {
    let chunkAttempts = 0
    request.mockImplementation(async (config) => {
      const url = String(config.url)
      if (url === "/api/archives/upload/session") return sessionResponse(0, 10)
      chunkAttempts += 1
      // The session was swept: every retry would 404 identically.
      return { status: 404, data: { error: "Unknown upload session." }, headers: {} }
    })

    await expect(uploadResumable(file(10), {})).rejects.toThrow(/Unknown upload session/)
    expect(chunkAttempts).toBe(1)
  })

  it("gives up when the server's offset never advances, instead of resending forever", async () => {
    let chunkAttempts = 0
    request.mockImplementation(async (config) => {
      const url = String(config.url)
      if (url === "/api/archives/upload/session") return sessionResponse(0, 100)
      chunkAttempts += 1
      // Always answers "you are still at 0" — a broken proxy, not an offset conflict.
      return chunkResponse(204, 0)
    })

    await expect(uploadResumable(file(100), {})).rejects.toThrow(/stalled/)
    // Each round consumes the driver's retry budget, so this is bounded rather than infinite.
    expect(chunkAttempts).toBeLessThan(100)
  })

  it("refuses a server that claims more bytes than the file has", async () => {
    request.mockImplementation(async (config) => {
      const url = String(config.url)
      if (url === "/api/archives/upload/session") return sessionResponse(0, 10)
      return chunkResponse(204, 99)
    })

    await expect(uploadResumable(file(10), {})).rejects.toThrow(/more bytes/)
  })
})
