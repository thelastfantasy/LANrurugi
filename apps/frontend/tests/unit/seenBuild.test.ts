import { beforeEach, describe, expect, it } from "vitest"

import {
  buildChanged,
  type BuildIdentity,
  readSeenBuild,
  sinceIdentity,
  writeSeenBuild,
} from "@/lib/seenBuild"

const A: BuildIdentity = { version: "0.1.0", sha: "aaaaaaa" }
const B: BuildIdentity = { version: "0.2.0", sha: "bbbbbbb" }

describe("seenBuild", () => {
  beforeEach(() => {
    localStorage.clear()
  })

  it("round-trips a build", () => {
    writeSeenBuild(B)
    expect(readSeenBuild()).toEqual(B)
  })

  it("reports nothing remembered before anything was written", () => {
    expect(readSeenBuild()).toBeNull()
  })

  it("treats a corrupt or partial value as nothing remembered", () => {
    // A hand-edited or half-written marker must not take the app down with it.
    localStorage.setItem("lanrurugi_seen_build", "{not json")
    expect(readSeenBuild()).toBeNull()

    localStorage.setItem("lanrurugi_seen_build", JSON.stringify({ version: "0.2.0" }))
    expect(readSeenBuild()).toBeNull()

    localStorage.setItem("lanrurugi_seen_build", JSON.stringify({ version: 5, sha: "x" }))
    expect(readSeenBuild()).toBeNull()
  })

  it("prefers the commit as the ?since= identity, falling back to the version", () => {
    expect(sinceIdentity(A)).toBe("aaaaaaa")
    // A build with no commit (no `.git`, no injected SHA) still has a version to ask about.
    expect(sinceIdentity({ version: "0.2.0", sha: "" })).toBe("0.2.0")
    expect(sinceIdentity({ version: "", sha: "" })).toBeNull()
  })

  describe("buildChanged", () => {
    it("is false on a first visit — there is no 'before' to report on", () => {
      expect(buildChanged(null, B)).toBe(false)
    })

    it("is false when the browser is looking at the same build", () => {
      expect(buildChanged(A, { ...A })).toBe(false)
    })

    it("is true when either half of the identity moved", () => {
      expect(buildChanged(A, B)).toBe(true)
      // Same version, new commit: a trunk rebuild, which is exactly the case a version-only
      // marker (legacy's own `lrrVersion`) would have missed.
      expect(buildChanged(A, { version: A.version, sha: "ccccccc" })).toBe(true)
      expect(buildChanged(A, { version: "0.1.1", sha: A.sha })).toBe(true)
    })
  })
})
