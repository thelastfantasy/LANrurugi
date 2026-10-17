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

/** Where the wizard's legacy-import step sends its upload.
 *
 *  Deliberately hardcoded to `merge` + no tag minimization rather than exposing the choices the
 *  Backup page offers: on the fresh instance this wizard is written for there is nothing to
 *  conflict with yet, so the advanced modes would be a decision with no observable consequence.
 *  The `/backup` link next to the file picker is the escape hatch for a library that is *not*
 *  fresh. Exported so a test can pin the contract rather than leaving it as a string literal
 *  buried in a component. */
export const FIRST_RUN_LEGACY_IMPORT_URL =
  "/api/database/import-legacy?on_existing=merge&minimize_tags=false"
