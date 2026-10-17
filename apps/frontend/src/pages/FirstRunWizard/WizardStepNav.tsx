import { useTranslation } from "react-i18next"

import { Button } from "@/components/common-ui/Form"

import { FIRST_RUN_WIZARD_STEPS, type FirstRunWizardStep } from "./logic"

// The two pieces of chrome every first-run wizard step shares: the step indicator at the top of
// the page and the action row at the bottom of each step. Kept in one file because they are only
// ever used together and both exist purely to keep the five steps' own layouts consistent.

/** Step indicator, same visual language as `PluginWizard/WizardSteps.tsx` (`favtag-btn`, with
 *  `.toggled` on the current one) so the two wizards in this app look like one family.
 *
 *  Every step is clickable, always: this wizard is a set of independent optional items rather than
 *  a linear form, so jumping straight to the one you came for is a legitimate way to use it. An
 *  earlier version rendered not-yet-reached steps as plain dimmed text, which also made the row
 *  look broken — five identical pills read as one control, one pill followed by four bare labels
 *  reads as an accident.
 *
 *  `completed` is keyed by the *item* being handled (a password of your own is set, a key is
 *  configured, an import has run), not by "this step was looked at" — with every item skippable, a
 *  tick for merely having passed through would put four green checks and a "done" next to a library
 *  still on the default password. */
export function WizardStepNav({
  current,
  completed,
  onGoTo,
}: {
  current: FirstRunWizardStep
  completed: Partial<Record<FirstRunWizardStep, boolean>>
  onGoTo: (step: FirstRunWizardStep) => void
}) {
  const { t } = useTranslation()

  return (
    <nav
      style={{
        display: "flex",
        gap: 8,
        alignItems: "center",
        justifyContent: "center",
        flexWrap: "wrap",
        marginBottom: 16,
      }}
    >
      {FIRST_RUN_WIZARD_STEPS.map((step) => {
        const label = t(`firstRunWizard.step.${step}`)
        const isCurrent = step === current
        const isDone = completed[step] === true
        return (
          <Button
            key={step}
            variant="favtag-btn"
            className={isCurrent ? "toggled" : undefined}
            aria-current={isCurrent ? "step" : undefined}
            // A `title` rather than an `aria-label`: the tick is decorative for the accessible name
            // (which has to stay the plain step label), but a sighted user should be able to hover
            // and read what the tick means.
            title={isDone ? t("firstRunWizard.stepCompleted") ?? undefined : undefined}
            style={{ fontSize: "8pt", padding: "0 8px" }}
            onClick={() => onGoTo(step)}
          >
            {isDone && (
              <i className="fa fa-check wizard-step-done" aria-hidden="true"></i>
            )}{" "}
            {label}
          </Button>
        )
      })}
    </nav>
  )
}

/** One step's own action row — `Back` only where there is somewhere to go back to, and the
 *  per-step `Skip` only where skipping the *item* means something distinct from the wizard-wide
 *  skip in the page footer.
 *
 *  `Skip`'s own `marginRight: auto` absorbs the free space, so the default `"end"` alignment reads
 *  `[Skip] ......... [Back] [Primary]` — a conventional footer, with the primary button in the same
 *  place on every step that has one. `"center"` is for the first step, which has neither Back nor
 *  Skip: a lone call to action with no navigation either side of it reads better centred under the
 *  intro text than pinned to a corner. The label stays visible while `busy`, with a spinner beside
 *  it, so a slow save never turns into an unlabelled spinner. */
export function StepActions({
  onBack,
  onSkip,
  skipLabel,
  primaryLabel,
  onPrimary,
  primaryDisabled,
  busy,
  align = "end",
}: {
  onBack?: () => void
  onSkip?: () => void
  skipLabel?: string
  primaryLabel: string
  onPrimary: () => void
  primaryDisabled?: boolean
  busy?: boolean
  align?: "end" | "center"
}) {
  const { t } = useTranslation()

  return (
    <div
      style={{
        display: "flex",
        gap: 8,
        alignItems: "center",
        justifyContent: align === "center" ? "center" : "flex-end",
        flexWrap: "wrap",
        marginTop: 16,
      }}
    >
      {onSkip && (
        <Button variant="favtag-btn" style={{ marginRight: "auto" }} onClick={onSkip}>
          {skipLabel}
        </Button>
      )}
      {onBack && (
        <Button variant="stdbtn" style={{ minWidth: "auto", width: "auto" }} onClick={onBack}>
          {t("pluginWizard.previousStep")}
        </Button>
      )}
      <Button variant="stdbtn" disabled={primaryDisabled || busy} onClick={onPrimary}>
        {busy && <i className="fa fa-compact-disc fa-spin" aria-hidden="true"></i>} {primaryLabel}
      </Button>
    </div>
  )
}
