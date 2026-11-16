import { afterEach, describe, expect, it, vi } from "vitest"

// #282: the envelope post must bypass every `fetch` wrapper installed after the SDK module loads,
// or each posted span records a new span to post.
describe("diagnostics ingest loop", () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.resetModules()
  })

  it("posts envelopes with the fetch captured before any wrapper", async () => {
    const native = vi.fn(async () => new Response(null, { status: 204 }))
    vi.stubGlobal("fetch", native)
    const { localTransport } = await import("@/diagnostics/sdk")

    const wrapper = vi.fn(async () => new Response(null, { status: 204 }))
    vi.stubGlobal("fetch", wrapper)

    const result = await localTransport().send({ body: "{}\n" } as never)
    expect(result).toEqual({ statusCode: 204 })
    expect(native).toHaveBeenCalledOnce()
    expect(wrapper).not.toHaveBeenCalled()
  })

  it("recognises the ingest path in any URL form", async () => {
    const { isIngestRequest } = await import("@/diagnostics/sdk")
    expect(isIngestRequest("/api/diagnostics/envelope")).toBe(true)
    expect(isIngestRequest("https://lrrg.example/api/diagnostics/envelope/")).toBe(true)
    expect(isIngestRequest("/api/diagnostics/config")).toBe(false)
    expect(isIngestRequest("/api/archives")).toBe(false)
  })
})

describe("foreign-node errors", () => {
  it("matches only React's and Sentry's own properties refused by Firefox", async () => {
    const { FOREIGN_NODE_ERROR } = await import("@/diagnostics/sdk")
    expect(FOREIGN_NODE_ERROR.test('Permission denied to access property "__reactFiber$63mpwa23tfj"')).toBe(true)
    expect(FOREIGN_NODE_ERROR.test('Permission denied to access property "__reactProps$abc"')).toBe(true)
    expect(FOREIGN_NODE_ERROR.test('Permission denied to access property "_sentryId"')).toBe(true)
    expect(FOREIGN_NODE_ERROR.test('Permission denied to access property "document"')).toBe(false)
    expect(FOREIGN_NODE_ERROR.test("Cannot read properties of undefined")).toBe(false)
  })
})
