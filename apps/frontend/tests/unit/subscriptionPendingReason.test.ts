// The waiting row's own wording — the split added after a live report (2026-10-09) that every
// candidate of a subscription said "published too recently" when the real blocker was a field the
// listing never carried.
import "@/i18n"

import { describe, expect, it } from "vitest"

import i18n from "@/i18n"
import { tooSoonText } from "@/pages/Settings/subscriptionVerdicts"

const t = i18n.t.bind(i18n)

describe("tooSoonText", () => {
  it("names the clock for an age rule, with the time still to wait", () => {
    const text = tooSoonText(t, { kind: "age", field: "posted_at", remaining_secs: 3600 })
    expect(text).toContain("1:00:00")
  })

  it("names the missing field instead of blaming the publish time", () => {
    const text = tooSoonText(t, { kind: "unknown_field", field: "uploader" })
    // The field is labelled the way the preview's own column labels it, not as a raw identifier.
    expect(text).toContain("Uploader")
    expect(text).not.toMatch(/too recently/i)
  })

  it("keeps the original wording for records written before the reason existed", () => {
    expect(tooSoonText(t, undefined)).toBe(t("subscriptions.verdictTooSoon"))
  })

  it("falls back to the raw field name when there is no label for it", () => {
    const text = tooSoonText(t, { kind: "unknown_field", field: "view_count" })
    expect(text).toContain("view_count")
  })
})
