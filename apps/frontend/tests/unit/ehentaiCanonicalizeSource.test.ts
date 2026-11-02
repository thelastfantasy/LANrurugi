import { describe, expect, it } from "vitest"

// Imported straight from the real plugin file rather than copied — a copy would let the plugin and
// this contract test drift apart silently, which is the exact failure mode the test exists to
// prevent. `canonicalizeSource` is a pure function with no `legacyCompat`/ambient-global use, so it
// imports cleanly outside a Deno plugin subprocess (the rest of the module's exports are never
// called here).
import { canonicalizeSource } from "../../../../plugins/download/ehentai"

/** Every URL shape that refers to one specific gallery. The SDK contract requires all of these to
 * normalize to one byte-identical string: the host compares stored `source:` tags against freshly
 * reported ones by plain string equality, so any disagreement here means a silent failure to match
 * with nothing reported anywhere — harder to diagnose than an exception (issue #107). */
const GID = "1000003"
const TOKEN = "cccccccccc"
const CANONICAL = `e-hentai.org/g/${GID}/${TOKEN}`

describe("ehentai canonicalizeSource", () => {
  it.each([
    // Freshly resolved by execDownload.
    [`https://e-hentai.org/g/${GID}/${TOKEN}/`, "https + trailing slash"],
    [`https://exhentai.org/g/${GID}/${TOKEN}/`, "exhentai domain alias"],
    [`https://g.e-hentai.org/g/${GID}/${TOKEN}/`, "g. subdomain alias"],
    [`https://forums.e-hentai.org/g/${GID}/${TOKEN}/`, "forums. subdomain alias"],
    // Shapes a `source:` tag could already hold from an older plugin version, or from the host's
    // own generic trim_url().
    [`e-hentai.org/g/${GID}/${TOKEN}`, "already trimmed by the host"],
    [`http://www.e-hentai.org/g/${GID}/${TOKEN}/`, "http + www"],
    [`https://e-hentai.org/g/${GID}/${TOKEN}/?p=2`, "with a query string"],
    [`https://e-hentai.org/g/${GID}/${TOKEN}/#comments`, "with a fragment"],
    [`  https://e-hentai.org/g/${GID}/${TOKEN}/  `, "surrounded by whitespace"],
    [`HTTPS://E-Hentai.ORG/g/${GID}/${TOKEN}/`, "mixed case scheme and host"],
    // A version_history entry, built by the plugin itself.
    [`e-hentai.org/g/${GID}/${TOKEN}`, "a version_history entry"],
  ])("folds %s (%s) onto the canonical form", (input) => {
    expect(canonicalizeSource(input)).toBe(CANONICAL)
  })

  it("is idempotent", () => {
    const once = canonicalizeSource(`https://exhentai.org/g/${GID}/${TOKEN}/?p=3`)
    expect(canonicalizeSource(once)).toBe(once)
    expect(once).toBe(CANONICAL)
  })

  it("keeps different galleries distinct", () => {
    expect(canonicalizeSource(`https://e-hentai.org/g/${GID}/${TOKEN}/`)).not.toBe(
      canonicalizeSource("https://e-hentai.org/g/1000002/dddddddddd/"),
    )
  })

  it("does not collapse two galleries that share a gid but not a token", () => {
    // E-Hentai identity is the gid+token pair, not the gid alone — normalizing toward the gid only
    // would silently merge two distinct galleries.
    expect(canonicalizeSource(`e-hentai.org/g/${GID}/aaaaaaaaaa`)).not.toBe(CANONICAL)
  })

  it("falls back to generic trimming for a non-gallery URL rather than mangling it", () => {
    expect(canonicalizeSource("https://www.e-hentai.org/uploader/someone/")).toBe(
      "e-hentai.org/uploader/someone",
    )
  })

  it("tolerates an empty or junk input without throwing", () => {
    expect(canonicalizeSource("")).toBe("")
    expect(canonicalizeSource("not a url at all")).toBe("not a url at all")
  })
})
