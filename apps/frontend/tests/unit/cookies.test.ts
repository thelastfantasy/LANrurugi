import { describe, expect, it } from "vitest"

import { cookieNameForParameter, hasCookieParameters, parseCookieHeader } from "@/lib/cookies"

describe("parseCookieHeader", () => {
  it("splits a document.cookie string into pairs, trimming whitespace", () => {
    const cookies = parseCookieHeader(" ipb_member_id=63553; ipb_pass_hash=abc123 ; sk=xyz ")
    expect(cookies.get("ipb_member_id")).toBe("63553")
    expect(cookies.get("ipb_pass_hash")).toBe("abc123")
    expect(cookies.get("sk")).toBe("xyz")
  })

  it("keeps everything after the first '=', so base64-like values survive", () => {
    expect(parseCookieHeader("igneous=abc==; hath_perks=p1.p2.a.s.q").get("igneous")).toBe("abc==")
  })

  it("strips surrounding double quotes and skips malformed or empty segments", () => {
    const cookies = parseCookieHeader(' star="quoted"; broken; =nope; ; ')
    expect(cookies.get("star")).toBe("quoted")
    expect(cookies.size).toBe(1)
  })
})

describe("cookieNameForParameter", () => {
  it("reads the cookie name the real login plugins put at the start of the description", () => {
    expect(cookieNameForParameter({ name: "param2", desc: "ipb_pass_hash cookie" })).toBe(
      "ipb_pass_hash",
    )
    expect(
      cookieNameForParameter({ name: "param4", desc: "igneous cookie(optional, if present …)" }),
    ).toBe("igneous")
    expect(
      cookieNameForParameter({ name: "param5", desc: "sk cookie (optional fallback — …)" }),
    ).toBe("sk")
  })

  it("falls back to the parameter name when the description mentions a cookie without naming it", () => {
    expect(cookieNameForParameter({ name: "sessionid", desc: "The session cookie" })).toBe("sessionid")
  })

  it("returns null for a non-cookie parameter, so the tool only appears where it helps", () => {
    expect(cookieNameForParameter({ name: "api_key", desc: "Your API key" })).toBeNull()
    expect(
      hasCookieParameters([
        { name: "api_key", desc: "Your API key" },
        { name: "sk", desc: "sk cookie" },
      ]),
    ).toBe(true)
  })
})
