import { describe, expect, it } from "vitest"

import en from "@/i18n/locales/en.json"
import ja from "@/i18n/locales/ja.json"
import zh from "@/i18n/locales/zh.json"
import {
  FIRST_RUN_IMPORT_CONFLICT_MODE,
  FIRST_RUN_WIZARD_STEPS,
  passwordStepProblem,
} from "@/pages/FirstRunWizard/logic"
import { runningLibraryScan } from "@/pages/Settings/LegacyImportPanel"

describe("passwordStepProblem", () => {
  it("treats an untouched pair as a no-op rather than an error", () => {
    expect(passwordStepProblem("", "")).toBe("empty")
  })

  it("reports a half-typed password as a mismatch rather than silently discarding it", () => {
    // Only the first field filled in: treating this as "empty" (i.e. advancing) would throw away
    // what the user actually typed without a word — the mismatch message is the useful answer.
    expect(passwordStepProblem("hunter2", "")).toBe("mismatch")
  })

  it("reports a real typo as a mismatch", () => {
    expect(passwordStepProblem("hunter2", "hunter3")).toBe("mismatch")
  })

  it("accepts a matching pair, including a deliberately long generated one", () => {
    const generated = "x".repeat(200)
    expect(passwordStepProblem(generated, generated)).toBeNull()
  })
})

describe("FIRST_RUN_IMPORT_CONFLICT_MODE", () => {
  it("starts on overwrite, so titles edited in LANraragi replace the file-name ones", () => {
    expect(FIRST_RUN_IMPORT_CONFLICT_MODE).toBe("overwrite")
  })
})

describe("runningLibraryScan", () => {
  it("finds a queued or active library scan and ignores everything else", () => {
    const scan = { name: "startup_scan", state: "active" }
    expect(runningLibraryScan([{ name: "backup", state: "active" }, scan])).toBe(scan)
    expect(runningLibraryScan([{ name: "rebuild_index", state: "queued" }])).toBeDefined()
    expect(runningLibraryScan([{ name: "startup_scan", state: "finished" }])).toBeUndefined()
    expect(runningLibraryScan([{ name: "startup_scan", state: "failed" }])).toBeUndefined()
    expect(runningLibraryScan(undefined)).toBeUndefined()
  })
})

describe("FIRST_RUN_WIZARD_STEPS", () => {
  it("starts at the welcome step and ends at done", () => {
    expect(FIRST_RUN_WIZARD_STEPS[0]).toBe("welcome")
    expect(FIRST_RUN_WIZARD_STEPS[FIRST_RUN_WIZARD_STEPS.length - 1]).toBe("done")
  })

  it("covers each optional item and has no duplicates", () => {
    expect(new Set(FIRST_RUN_WIZARD_STEPS).size).toBe(FIRST_RUN_WIZARD_STEPS.length)
    for (const step of ["basics", "llmKey", "import"] as const) {
      expect(FIRST_RUN_WIZARD_STEPS).toContain(step)
    }
  })
})

/** The three locales kept at full key parity (`en`/`ja`/`zh` all carry every key; the other eleven
 *  are partial and fall back to English). A key added here but not to all three renders as the raw
 *  dotted key in that language, which is the failure this guards. */
describe("firstRunWizard translation keys", () => {
  const wizardKeys = (locale: Record<string, string>) =>
    Object.keys(locale)
      .filter((key) => key.startsWith("firstRunWizard."))
      .sort()

  it("is present in all three fully-translated locales with the same keys", () => {
    expect(wizardKeys(en).length).toBeGreaterThan(0)
    expect(wizardKeys(ja)).toEqual(wizardKeys(en))
    expect(wizardKeys(zh)).toEqual(wizardKeys(en))
  })

  it("has no empty strings", () => {
    for (const locale of [en, ja, zh]) {
      for (const key of wizardKeys(locale)) {
        expect(locale[key].trim(), key).not.toBe("")
      }
    }
  })

  it("defines every key the wizard and the Settings re-run button ask for", () => {
    for (const key of [
      "firstRunWizard.title",
      "firstRunWizard.rerunButton",
      "firstRunWizard.skipStep",
      "firstRunWizard.skipWizard",
      "firstRunWizard.submitAndBackToLibrary",
      "firstRunWizard.submitAndBackToSettings",
      "firstRunWizard.submitAllHint",
      "firstRunWizard.submittedAll",
      "firstRunWizard.nothingToSubmit",
      "firstRunWizard.submitFailedOn",
      ...FIRST_RUN_WIZARD_STEPS.map((step) => `firstRunWizard.step.${step}`),
    ]) {
      expect(en[key], key).toBeTruthy()
    }
  })
})
