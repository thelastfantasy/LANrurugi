import { describe, expect, it } from "vitest"

import {
  isFirstRunWizardTriggerPath,
  shouldOpenFirstRunWizard,
} from "@/FirstRunWizardGate"

/** Defaults describe "a signed-in admin landing on the library of an instance that has never run
 *  the wizard" — the one case that opens it. Each test flips exactly one field. */
function input(overrides: Partial<Parameters<typeof shouldOpenFirstRunWizard>[0]> = {}) {
  return {
    pathname: "/",
    sessionResolved: true,
    isAuthenticated: true,
    firstRunWizardPending: true,
    alreadyOpenedThisPageLoad: false,
    ...overrides,
  }
}

describe("isFirstRunWizardTriggerPath", () => {
  it("triggers only on the library landing page", () => {
    expect(isFirstRunWizardTriggerPath("/")).toBe(true)
    for (const path of ["/welcome", "/config", "/upload", "/login", "/reader/abc"]) {
      expect(isFirstRunWizardTriggerPath(path)).toBe(false)
    }
  })
})

describe("shouldOpenFirstRunWizard", () => {
  it("opens for an authenticated admin on the library while the wizard is pending", () => {
    expect(shouldOpenFirstRunWizard(input())).toBe(true)
  })

  it("does not open once the instance has handled the wizard", () => {
    expect(shouldOpenFirstRunWizard(input({ firstRunWizardPending: false }))).toBe(false)
  })

  it("never opens for a caller who is not an authenticated admin", () => {
    expect(shouldOpenFirstRunWizard(input({ isAuthenticated: false }))).toBe(false)
  })

  it("waits rather than guessing while the session status is unresolved", () => {
    // `isSuccess` false covers both "still loading" and "errored" — neither is an answer, and
    // acting on either would open the wizard for a visitor who may turn out to be a guest.
    expect(shouldOpenFirstRunWizard(input({ sessionResolved: false }))).toBe(false)
  })

  it("leaves every other route alone, so a deep link or a nav click is not hijacked", () => {
    expect(shouldOpenFirstRunWizard(input({ pathname: "/config" }))).toBe(false)
    expect(shouldOpenFirstRunWizard(input({ pathname: "/welcome" }))).toBe(false)
  })

  it("opens at most once per page load", () => {
    // Without this the browser's Back button out of `/welcome` would be a dead end.
    expect(shouldOpenFirstRunWizard(input({ alreadyOpenedThisPageLoad: true }))).toBe(false)
  })
})
