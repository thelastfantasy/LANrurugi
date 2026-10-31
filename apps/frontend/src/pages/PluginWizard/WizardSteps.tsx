import { useTranslation } from "react-i18next"

import type { PluginType, WizardStep } from "./useWizardSession"

const STEPS: WizardStep[] = ["typeSelection", "sharedLinks", "typeDetail"]

/** Step indicator + back button + (while on the `typeDetail` step with more than one selected
 * type) a pill-tab row for switching which type's panel is shown. */
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
  /** Corresponds to `session === null`, before this component mounts — resets the whole session,
   * same as "上一步" does from `typeSelection`. */
  onGoToLookup: () => void
  /** Only moves backward; a click on a step ahead of `currentStep` is ignored. */
  onGoToStep: (step: WizardStep) => void
  /** Only passed for steps with an actual next step (`typeSelection`/`sharedLinks`) — `typeDetail`
   * has none of its own, so this stays `undefined` and the button isn't rendered there. */
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
