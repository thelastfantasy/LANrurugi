import type { ImportConflictMode } from "@/api/types"

/** The wizard's step order — the step indicator, the clickable-back rule, and which step counts
 *  as "last" all read this one list.
 *
 *  `basics` was `password` until the language and timezone pickers moved onto the same page: the
 *  page is no longer about one item, and a tab labelled "Password" holding a timezone dropdown is
 *  worse than the churn of renaming it. */
export const FIRST_RUN_WIZARD_STEPS = ["welcome", "basics", "llmKey", "import", "done"] as const

export type FirstRunWizardStep = (typeof FIRST_RUN_WIZARD_STEPS)[number]

/** `null` when the pair is submittable, otherwise which of the two problems it has.
 *
 *  Pulled out of the form so the rules stay testable without rendering it: an untouched pair
 *  (`"empty"`) is a deliberate no-op the caller turns into "just advance", while a mismatch is a
 *  real error — treating those two as one case is how an accidental single-field typo would end
 *  up silently *clearing* the password instead of being refused, which is exactly what
 *  `settings::change_password`'s own docs warn about (it always sets whatever it is given, with
 *  no "blank means keep current" convention to fall back on). */
export function passwordStepProblem(
  password: string,
  confirmation: string,
): "empty" | "mismatch" | null {
  if (!password) return "empty"
  if (password !== confirmation) return "mismatch"
  return null
}

/** The conflict mode the wizard's import step starts on.
 *
 *  `overwrite`, not the Backup page's `merge`: by the time the wizard runs every archive already
 *  has a title and summary derived from its file name, and `merge` keeps those, silently discarding
 *  the ones edited in LANraragi. Every import saves a rollback snapshot. */
export const FIRST_RUN_IMPORT_CONFLICT_MODE = "overwrite" satisfies ImportConflictMode
