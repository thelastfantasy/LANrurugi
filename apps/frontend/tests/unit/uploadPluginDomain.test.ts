import { describe, expect, it } from "vitest"

import type { PluginInfo } from "@/api/types"
import { findPluginByDomain, matchesDomain } from "@/pages/Upload/shared"

function plugin(domain_match: string[]): PluginInfo {
  return {
    namespace: "metadata/chaika",
    type: "metadata",
    name: "Chaika.moe",
    author: "",
    description: "",
    version: "0.1",
    icon: "",
    oneshot_arg: "",
    url_pattern: "chaika\\.moe",
    domain_match,
    kind: "metadata",
  } as PluginInfo
}

describe("matchesDomain", () => {
  it("covers subdomains of a declared domain", () => {
    expect(matchesDomain(plugin(["chaika.moe"]), "https://panda.chaika.moe/archive/44877/")).toBe(true)
    expect(matchesDomain(plugin(["e-hentai.org"]), "https://g.e-hentai.org/g/1/abc/")).toBe(true)
  })

  it("still matches the declared domain itself", () => {
    expect(matchesDomain(plugin(["chaika.moe"]), "https://chaika.moe/")).toBe(true)
  })

  it("does not match an unrelated host that merely contains the name", () => {
    expect(matchesDomain(plugin(["chaika.moe"]), "https://notchaika.moe/")).toBe(false)
    expect(matchesDomain(plugin(["chaika.moe"]), "https://chaika.moe.example.com/")).toBe(false)
  })

  it("finds the owning plugin for a subdomain URL", () => {
    const plugins = [plugin(["chaika.moe"])]
    expect(findPluginByDomain(plugins, "https://panda.chaika.moe/archive/1/")?.namespace).toBe(
      "metadata/chaika",
    )
  })
})
