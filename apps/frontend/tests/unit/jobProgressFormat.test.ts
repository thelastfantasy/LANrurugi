import { describe, expect, it } from "vitest"

import { formatBytes, formatDuration } from "@/components/Display"

describe("formatDuration", () => {
  it("stays minutes:seconds under an hour, zero-padded", () => {
    expect(formatDuration(0)).toBe("0:00")
    expect(formatDuration(65_000)).toBe("1:05")
    expect(formatDuration(59 * 60_000 + 59_000)).toBe("59:59")
  })

  it("adds an hours field rather than a third ambiguous number", () => {
    expect(formatDuration(3_600_000)).toBe("1:00:00")
    expect(formatDuration(3_600_000 + 4 * 60_000 + 7_000)).toBe("1:04:07")
  })

  it("never renders a negative time", () => {
    expect(formatDuration(-5_000)).toBe("0:00")
  })
})

describe("formatBytes", () => {
  it("scales 1024-based, one decimal from MB up", () => {
    expect(formatBytes(512)).toBe("512 B")
    expect(formatBytes(2048)).toBe("2 KB")
    expect(formatBytes(1.5 * 1024 * 1024)).toBe("1.5 MB")
  })
})
