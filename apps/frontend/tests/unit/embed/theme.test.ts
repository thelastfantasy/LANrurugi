import { describe, expect, it } from "vitest"

import { applyCardTheme, parseCardTheme } from "@/embed/theme"

// jsdom has no `CSS.supports`; a hex or named colour stands in for "valid".
const isColor = (v: string) => /^#[0-9a-f]{3,8}$/i.test(v) || v === "rebeccapurple"

describe("card theme", () => {
  it("takes a preset, else the library's theme, else the visitor's own", () => {
    expect(parseCardTheme("ex", "g.css", isColor)).toEqual({ base: "ex", colors: {} })
    expect(parseCardTheme(null, "g.css", isColor)).toEqual({ base: "g", colors: {} })
    expect(parseCardTheme("bogus", "g.css", isColor)).toEqual({ base: "g", colors: {} })
    expect(parseCardTheme(null, undefined, isColor)).toEqual({ base: "system", colors: {} })
  })

  it("lays colours from an object over a preset", () => {
    const theme = parseCardTheme('{"base":"dark","bg":"#101820","accent":"rebeccapurple"}', "g.css", isColor)
    expect(theme).toEqual({ base: "dark", colors: { bg: "#101820", accent: "rebeccapurple" } })
    expect(parseCardTheme('{"fg":"#fff"}', "g.css", isColor)).toEqual({ base: "g", colors: { fg: "#fff" } })
  })

  it("drops anything that is not a colour, so nothing else reaches the stylesheet", () => {
    const theme = parseCardTheme(
      '{"bg":"red;} body{display:none","fg":42,"accent":"#abc","position":"#fff"}',
      "g.css",
      isColor,
    )
    expect(theme.colors).toEqual({ accent: "#abc" })
    expect(parseCardTheme("{not json", "g.css", isColor)).toEqual({ base: "g", colors: {} })
  })

  it("puts the colours on the card root as variables", () => {
    const root = document.createElement("div")
    root.id = "card-root"
    applyCardTheme(root, { base: "ex", colors: { bg: "#101820" } })
    expect(root.dataset.cardTheme).toBe("ex")
    expect(document.head.querySelector("style")?.textContent).toBe("#card-root .lrrg-card { --card-bg: #101820; }")
  })
})
