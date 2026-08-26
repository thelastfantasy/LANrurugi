import { useTranslation } from "react-i18next"

import type { PluginType, WizardStep } from "./useWizardSession"

const STEPS: WizardStep[] = ["typeSelection", "sharedLinks", "typeDetail"]

/** Step indicator + back button + (while on the `typeDetail` step with more than one selected
 * type) a pill-tab row for switching which type's panel is shown — replaces the old "render every
 * step/type stacked on one screen" layout (real user feedback, 2026-08-26: "首先从上到下的瀑布式
 * 布局就要改掉"). No existing step-indicator component/CSS to reuse anywhere in this codebase
 * (verified) — plain inline-styled `<span>`s, matching every other `PluginWizard/*` file's own
 * convention. The type-switch row *does* reuse a real precedent: `.favtag-btn`/`.toggled`, the
 * same pill-button pattern `dialog.tsx`'s `NewCategoryForm` already uses for its static/dynamic
 * category toggle. */
export function WizardSteps({
  currentStep,
  selectedTypes,
  activeType,
  onBack,
  onActiveTypeChanged,
  onGoToLookup,
  onGoToStep,
  nextStep,
}: {
  currentStep: WizardStep
  selectedTypes: PluginType[]
  activeType: PluginType | null
  onBack: () => void
  onActiveTypeChanged: (type: PluginType) => void
  /** Clicking "1 查找域名" itself — that step corresponds to `session === null` (before this
   * component even mounts), so it can't be reached via `goToStep`; the parent resets the whole
   * session, same as "上一步" already does from `typeSelection`. */
  onGoToLookup: () => void
  /** Clicking one of "2/3/4" — real user feedback, 2026-08-26: "这里1到4能点击切换更好". Only
   * ever moves backward; a click on a step ahead of `currentStep` is ignored (see `goToStep`'s own
   * reducer docs on why forward jumps aren't allowed here). */
  onGoToStep: (step: WizardStep) => void
  /** Mirrors "上一步" on the opposite side of the same row — real user feedback, 2026-08-26:
   * "下一步按钮放到右上角比较好，和上一步按钮对应". Only passed for steps that actually have a
   * "下一步" (`typeSelection`/`sharedLinks` in `index.tsx`) — `typeDetail` has no next step of its
   * own (generation/save happen inline in `TypeWizardPanel`), so this stays `undefined` there and
   * the button simply isn't rendered. */
  nextStep?: { disabled: boolean; onClick: () => void }
}) {
  const { t } = useTranslation()
  const currentIndex = STEPS.indexOf(currentStep)

  return (
    <div style={{ marginBottom: 12 }}>
      <div style={{ display: "flex", alignItems: "center", gap: 12, flexWrap: "wrap" }}>
        <input
          type="button"
          className="stdbtn"
          style={{ minWidth: "auto", width: "auto" }}
          value={t("pluginWizard.previousStep") ?? ""}
          onClick={onBack}
        />
        <nav style={{ display: "flex", gap: 8, fontSize: "9pt", alignItems: "center" }}>
          {/* Clickable steps render as `.favtag-btn` pills (same pattern as the type-switch row
              below, and `dialog.tsx`'s own `NewCategoryForm` toggle) — real user feedback,
              2026-08-26: first an opacity-only distinction, then underlined links, were both too
              subtle to notice at a glance; a background-highlight pill is what the user actually
              asked for next. `.toggled` marks the current step. A non-clickable step stays plain
              gray text, no pill at all. */}
          {/* "1 查找域名" itself is never `currentStep` (that's `session === null` in `index.tsx`,
              before this component even mounts) — always rendered as already-done, and always
              clickable (going there always resets, same as "上一步" from `typeSelection`). */}
          <button
            type="button"
            className="favtag-btn"
            style={{ fontSize: "8pt", padding: "0 8px" }}
            onClick={onGoToLookup}
          >
            {t("pluginWizard.step.lookup")}
          </button>
          {STEPS.map((step, index) => {
            const clickable = index <= currentIndex
            const label = t(`pluginWizard.step.${step}`)
            const isCurrent = step === currentStep
            return clickable ? (
              <button
                key={step}
                type="button"
                aria-current={isCurrent ? "step" : undefined}
                className={`favtag-btn${isCurrent ? " toggled" : ""}`}
                style={{ fontSize: "8pt", padding: "0 8px" }}
                onClick={() => onGoToStep(step)}
              >
                {label}
              </button>
            ) : (
              <span key={step} style={{ opacity: 0.6 }}>
                {label}
              </span>
            )
          })}
        </nav>
        {nextStep && (
          <input
            type="button"
            className="stdbtn"
            style={{ marginLeft: "auto", minWidth: "auto", width: "auto" }}
            disabled={nextStep.disabled}
            value={t("pluginWizard.nextStep") ?? ""}
            onClick={nextStep.onClick}
          />
        )}
      </div>
      {currentStep === "typeDetail" && selectedTypes.length > 1 && (
        <div role="tablist" style={{ display: "flex", gap: 4, marginTop: 8, marginLeft: 68, maxWidth: 320 }}>
          {selectedTypes.map((type) => (
            <button
              key={type}
              type="button"
              role="tab"
              aria-selected={type === activeType}
              className={`favtag-btn${type === activeType ? " toggled" : ""}`}
              style={{ flex: 1, fontSize: "8pt" }}
              onClick={() => onActiveTypeChanged(type)}
            >
              {t(`pluginWizard.type.${type}`)}
            </button>
          ))}
        </div>
      )}
    </div>
  )
}
