import { describe, expect, it } from "vitest"

import { formatShareCode, normalizeShareCode, parseShareRef, toTextForm } from "@/share/code"

describe("share codes (mirrors lanrurugi-share-proto code.rs)", () => {
  it("forgives case, look-alikes and separators", () => {
    expect(normalizeShareCode("k7qm9-xrt2h")).toBe("K7QM9XRT2H")
    expect(normalizeShareCode("O1IL2 3456 7")).toBe("0111234567")
    expect(formatShareCode("K7QM9XRT2H")).toBe("K7QM9-XRT2H")
    expect(formatShareCode("K7QM9XRT")).toBe("K7QM-9XRT")
  })

  it("rejects bad codes", () => {
    expect(normalizeShareCode("K7QM9-XRT2")).toBeNull()
    expect(normalizeShareCode("K7QM9-XRT2U")).toBeNull()
    expect(normalizeShareCode("K7QM9-XRT2!")).toBeNull()
  })

  it("reads every input form", () => {
    for (const input of [
      "K7QM9-XRT2H",
      "  k7qm9 xrt2h ",
      "lrrg:K7QM9-XRT2H",
      "LRRG:k7qm9xrt2h",
      "lrrg://K7QM9-XRT2H/",
      "lrrg:K7QM9-XRT2H?future=1",
      "https://example.org/s/K7QM9-XRT2H",
      "https://example.org/s/K7QM9-XRT2H/?x=1",
      "http://192.168.1.5:3000/embed/s/k7qm9xrt2h",
      "https://example.org/#K7QM9-XRT2H",
    ]) {
      expect(parseShareRef(input)?.code, input).toBe("K7QM9XRT2H")
    }
  })

  it("does not take unrelated links for share references", () => {
    for (const input of [
      "https://example.org/",
      "https://example.org/reader/K7QM9XRT2H",
      "https://example.org/s/",
      "not/a/code",
    ]) {
      expect(parseShareRef(input), input).toBeNull()
    }
  })

  it("round-trips via hints through the text form", () => {
    const parsed = parseShareRef("lrrg:K7QM9-XRT2H?via=abc123@203.0.113.7:443")
    expect(parsed?.via).toEqual({ endpointId: "abc123", addr: "203.0.113.7:443" })
    expect(toTextForm(parsed!)).toBe("lrrg:K7QM9-XRT2H?via=abc123@203.0.113.7:443")
    expect(parseShareRef("lrrg:K7QM9XRT2H?via=abc@[2001:db8::1]:443")?.via?.addr).toBe("[2001:db8::1]:443")
    const relayed = "lrrg:K7QM9-XRT2H?via=abc@203.0.113.7:443&relay=https://relay.example.org/"
    expect(parseShareRef(relayed)?.via?.relay).toBe("https://relay.example.org/")
    expect(toTextForm(parseShareRef(relayed)!)).toBe(relayed)
    expect(parseShareRef("lrrg:K7QM9XRT2H?via=abc@1.2.3.4:5&relay=http://x/")?.via?.relay).toBeUndefined()
    const several = "lrrg:K7QM9-XRT2H?via=abc@[2001:db8::1]:443,203.0.113.7:443&relay=https://a.example/,https://b.example/"
    expect(toTextForm(parseShareRef(several)!)).toBe(several)
    const noded = "lrrg:K7QM9-XRT2H?via=abc@&relay=https://r.example/&node=def@node.example.org:443"
    expect(parseShareRef(noded)?.nodes).toBe("def@node.example.org:443")
    expect(toTextForm(parseShareRef(noded)!)).toBe(noded)
    expect(parseShareRef("lrrg:K7QM9XRT2H?via=abc@&relay=http://x/")?.via).toBeNull()
    const sealed = "lrrg:K7QM9-XRT2H?r=AbC-_09xyz"
    expect(parseShareRef(sealed)).toEqual({ code: "K7QM9XRT2H", via: null, hint: "AbC-_09xyz" })
    expect(toTextForm(parseShareRef(sealed)!)).toBe(sealed)
    expect(parseShareRef("lrrg:K7QM9XRT2H?r=not%20b64")?.hint).toBeUndefined()
    const malformed = parseShareRef("lrrg:K7QM9XRT2H?via=nonsense")
    expect(malformed?.via).toBeNull()
    expect(toTextForm(malformed!)).toBe("lrrg:K7QM9-XRT2H")
  })
})
