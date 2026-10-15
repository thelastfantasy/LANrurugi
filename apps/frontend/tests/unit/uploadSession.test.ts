import { describe, expect, it } from "vitest"

import {
  CHUNK_SIZE,
  isRetryableChunkFailure,
  MAX_STALLED_ROUNDS,
  nextChunk,
  parseUploadOffset,
  resumeKeyFor,
} from "@/api/uploadSession"

describe("resumeKeyFor", () => {
  it("identifies a file by what a re-pick preserves, and differs when the file does", () => {
    const file = { name: "book.zip", size: 1234, lastModified: 1_700_000_000_000 }
    expect(resumeKeyFor(file)).toBe(resumeKeyFor({ ...file }))
    // A same-named but different file (re-exported, edited) must not resume the other one's bytes.
    expect(resumeKeyFor(file)).not.toBe(resumeKeyFor({ ...file, size: 1235 }))
    expect(resumeKeyFor(file)).not.toBe(resumeKeyFor({ ...file, lastModified: 1_700_000_000_001 }))
    expect(resumeKeyFor(file)).not.toBe(resumeKeyFor({ ...file, name: "other.zip" }))
  })
})

describe("nextChunk", () => {
  it("walks a file in chunk-sized pieces and stops exactly at the end", () => {
    const size = CHUNK_SIZE * 2 + 10
    const ranges = []
    for (let offset = 0; ; ) {
      const range = nextChunk(offset, size)
      if (!range) break
      ranges.push(range)
      offset = range.end
    }
    expect(ranges).toEqual([
      { start: 0, end: CHUNK_SIZE },
      { start: CHUNK_SIZE, end: CHUNK_SIZE * 2 },
      { start: CHUNK_SIZE * 2, end: size },
    ])
  })

  it("returns nothing once the file is fully sent, and never past its end", () => {
    expect(nextChunk(500, 500)).toBeNull()
    expect(nextChunk(600, 500)).toBeNull()
    // A resume that already holds everything sends no further chunk.
    expect(nextChunk(500, 500, 16)).toBeNull()
    // A short file is one short chunk, not a full one.
    expect(nextChunk(0, 10)).toEqual({ start: 0, end: 10 })
    // A zero-byte file has nothing to send (the server refuses an empty complete).
    expect(nextChunk(0, 0)).toBeNull()
  })
})

describe("parseUploadOffset", () => {
  it("reads the server's offset whatever the header casing a proxy left behind", () => {
    expect(parseUploadOffset({ "upload-offset": "4096" })).toBe(4096)
    expect(parseUploadOffset({ "Upload-Offset": "4096" })).toBe(4096)
    expect(parseUploadOffset({ "UPLOAD-OFFSET": 4096 })).toBe(4096)
    expect(parseUploadOffset({ "upload-offset": "0" })).toBe(0)
  })

  it("reports nothing rather than guessing when the header is absent or nonsense", () => {
    expect(parseUploadOffset({})).toBeNull()
    expect(parseUploadOffset(undefined)).toBeNull()
    expect(parseUploadOffset(null)).toBeNull()
    expect(parseUploadOffset("4096")).toBeNull()
    expect(parseUploadOffset({ "upload-offset": "abc" })).toBeNull()
    // A negative offset would mean the server lost bytes it cannot have lost; the caller's fallback
    // (the requested range's end) is safer than rewinding to a nonsense position.
    expect(parseUploadOffset({ "upload-offset": "-5" })).toBeNull()
  })
})

describe("isRetryableChunkFailure", () => {
  it("retries transport failures, rate limits, and the offset conflict", () => {
    for (const status of [0, 408, 409, 425, 429, 500, 502, 503]) {
      expect(isRetryableChunkFailure(status)).toBe(true)
    }
  })

  it("does not retry a request that is simply wrong", () => {
    // 404 (session gone), 413 (chunk too large), 401 after a failed refresh: repeating any of these
    // produces the identical failure, so retrying them only delays the error the user needs to see.
    for (const status of [400, 401, 403, 404, 413, 422]) {
      expect(isRetryableChunkFailure(status)).toBe(false)
    }
  })

  it("bounds the stall guard so a non-advancing server cannot loop forever", () => {
    expect(MAX_STALLED_ROUNDS).toBeGreaterThan(1)
    expect(MAX_STALLED_ROUNDS).toBeLessThan(100)
  })
})
