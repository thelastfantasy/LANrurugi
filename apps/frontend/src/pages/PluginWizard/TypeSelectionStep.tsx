import { useTranslation } from "react-i18next"

import type { DomainLookupResult, PluginType, TypeSession } from "./useWizardSession"

const PLUGIN_TYPES: PluginType[] = ["login", "metadata", "download"]

/** Renders each of the three types' coverage state. An uncovered type can be multi-selected
 * freely; a covered type's checkbox is disabled but gets follow-up action button(s) per `coverageSource`. */
export function TypeSelectionStep({
  lookupResult,
  selectedTypes,
  typeSessions,
  onToggleType,
  onEditExisting,
}: {
  lookupResult: DomainLookupResult
  selectedTypes: PluginType[]
  typeSessions: Partial<Record<PluginType, TypeSession>>
  /** Toggling a type already selected in "generate override" mode deselects it, discarding its
   * entire `TypeSession`. Also used by "生成覆盖版本" buttons on covered types. */
  onToggleType: (type: PluginType) => void
  /** Fired by "编辑已有 AI 插件" — jumps straight to edit mode, bypassing the ordinary flow. */
  onEditExisting: (type: PluginType, namespace: string, declaredNamespace: string, sourceCode: string) => void
}) {
  const { t } = useTranslation()

  return (
    <div>
      <p>{t("pluginWizard.selectTypesToCreate")}</p>
      <ul style={{ listStyle: "none", fontSize: "9pt", padding: 0, margin: 0 }}>
        {PLUGIN_TYPES.map((type) => {
          const coverage = lookupResult[type]
          const selected = selectedTypes.includes(type)
          const isEditingExisting = typeSessions[type]?.editingExistingNamespace != null
          return (
            <li key={type} style={{ marginBottom: 4 }}>
              <label style={{ opacity: coverage.covered ? 0.6 : 1 }}>
                <input
                  type="checkbox"
                  checked={coverage.covered ? true : selected}
                  disabled={coverage.covered}
                  onChange={() => onToggleType(type)}
                />
                {t(`pluginWizard.type.${type}`)}
                {coverage.covered && (
                  <span style={{ marginLeft: 6 }}>
                    — {t("pluginWizard.alreadyCoveredBy", { namespace: coverage.namespace })}
                  </span>
                )}
              </label>
              {coverage.covered && coverage.coverageSource === "built-in" && (
                <button
                  type="button"
                  className={`favtag-btn${selected ? " toggled" : ""}`}
                  style={{ marginLeft: 8, fontSize: "8pt", minWidth: "auto", width: "auto", padding: "0 8px" }}
                  onClick={() => onToggleType(type)}
                >
                  {t(selected ? "pluginWizard.overrideSelected" : "pluginWizard.generateOverride")}
                </button>
              )}
              {coverage.covered && coverage.coverageSource === "ai-generated" && (
                <>
                  <button
                    type="button"
                    className={`favtag-btn${selected && !isEditingExisting ? " toggled" : ""}`}
                    style={{ marginLeft: 8, fontSize: "8pt", minWidth: "auto", width: "auto", padding: "0 8px" }}
                    onClick={() => onToggleType(type)}
                  >
                    {t(
                      selected && !isEditingExisting
                        ? "pluginWizard.overrideSelected"
                        : "pluginWizard.generateOverride",
                    )}
                  </button>
                  <button
                    type="button"
                    className={`favtag-btn${isEditingExisting ? " toggled" : ""}`}
                    style={{ marginLeft: 8, fontSize: "8pt", minWidth: "auto", width: "auto", padding: "0 8px" }}
                    onClick={() =>
                      onEditExisting(type, coverage.namespace, coverage.declaredNamespace, coverage.sourceCode)
                    }
                  >
                    {t("pluginWizard.editExisting")}
                  </button>
                </>
              )}
            </li>
          )
        })}
      </ul>
    </div>
  )
}
