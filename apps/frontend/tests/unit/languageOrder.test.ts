// The quick language switch in the settings sidebar writes the very same `language` setting the
// Global section's ordered list edits, so what it stores has to be a list, not a single code — the
// list also picks which spelling of a work's title is shown.
import { describe, expect, it } from "vitest"

import { languageOrderAfterPick } from "@/i18n"

describe("languageOrderAfterPick", () => {
  it("moves the picked language to the front and keeps the rest", () => {
    expect(languageOrderAfterPick("zh,ja,en", "ja")).toBe("ja,zh,en")
  })

  it("does not duplicate a language already in the list", () => {
    expect(languageOrderAfterPick("zh,ja", "zh")).toBe("zh,ja")
  })

  it("starts a one-entry list when nothing was preferred before", () => {
    expect(languageOrderAfterPick("auto", "zh")).toBe("zh")
    expect(languageOrderAfterPick(undefined, "en")).toBe("en")
  })

  it("clears the preference when the browser is chosen", () => {
    expect(languageOrderAfterPick("zh,ja", "auto")).toBe("auto")
  })

  it("keeps entries this build ships no interface translation for", () => {
    // They still order titles, so an interface pick must not drop them.
    expect(languageOrderAfterPick("zh,ja,en", "en")).toBe("en,zh,ja")
  })
})
