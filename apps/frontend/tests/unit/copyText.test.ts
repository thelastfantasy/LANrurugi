import { afterEach, describe, expect, it, vi } from "vitest"

import { copyText } from "@/lib/copyText"

/** jsdom runs on `http://localhost`, which it counts as a secure context; a self-hosted instance
 *  reached over a LAN IP is not, so each test pins the flag it is about. */
function setSecureContext(secure: boolean) {
  Object.defineProperty(window, "isSecureContext", { configurable: true, value: secure })
}

function setClipboard(writeText: (text: string) => Promise<void>) {
  Object.defineProperty(navigator, "clipboard", { configurable: true, value: { writeText } })
}

function setExecCommand(impl: (command: string) => boolean) {
  Object.defineProperty(document, "execCommand", { configurable: true, value: vi.fn(impl) })
}

afterEach(() => {
  Reflect.deleteProperty(navigator, "clipboard")
  Reflect.deleteProperty(document, "execCommand")
  setSecureContext(true)
})

describe("copyText", () => {
  it("uses the async Clipboard API in a secure context", async () => {
    setSecureContext(true)
    const writeText = vi.fn(() => Promise.resolve())
    setClipboard(writeText)

    await expect(copyText("hello")).resolves.toBe(true)
    expect(writeText).toHaveBeenCalledWith("hello")
  })

  it("falls back to a selection copy over plain HTTP, where the API does not exist", async () => {
    setSecureContext(false)
    // What Chrome reports on an insecure origin: the whole API is absent.
    Reflect.deleteProperty(navigator, "clipboard")
    setExecCommand(() => true)

    await expect(copyText("hello")).resolves.toBe(true)
    expect(document.execCommand).toHaveBeenCalledWith("copy")
    // The temporary textarea must not be left behind in the page.
    expect(document.querySelector("textarea")).toBeNull()
  })

  it("falls back when a present API rejects, rather than reporting failure", async () => {
    setSecureContext(true)
    setClipboard(() => Promise.reject(new Error("denied")))
    setExecCommand(() => true)

    await expect(copyText("hello")).resolves.toBe(true)
  })

  it("reports failure when both paths fail", async () => {
    setSecureContext(true)
    setClipboard(() => Promise.reject(new Error("denied")))
    setExecCommand(() => false)

    await expect(copyText("hello")).resolves.toBe(false)
  })

  it("reports failure when execCommand throws", async () => {
    setSecureContext(false)
    setExecCommand(() => {
      throw new Error("unsupported")
    })

    await expect(copyText("hello")).resolves.toBe(false)
  })
})
