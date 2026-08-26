import { useTranslation } from "react-i18next"

import type { DomainLookupResult, PluginType, TypeSession } from "./useWizardSession"

const PLUGIN_TYPES: PluginType[] = ["login", "metadata", "download"]

/** FR-003/FR-004: renders each of the three types' coverage state. An uncovered type can be
 * multi-selected freely. A covered type's checkbox stays disabled (nothing to toggle), but gets
 * one or both follow-up actions depending on what actually covers it (`coverageSource` —
 * `TypeCoverage`'s own docs): a built-in plugin offers only "生成覆盖版本" (behaves exactly like
 * selecting an uncovered type — the actual override-in-practice comes from the backend's
 * priority-sorted plugin matching, not any flag here). An AI-generated plugin from an earlier
 * wizard session offers *both* buttons side by side — real user feedback, 2026-08-26: even with
 * an existing AI-generated plugin already there to edit, the user may still want to throw it away
 * and generate a fresh override instead, same as a built-in-covered type can. "编辑已有 AI 插件"
 * jumps straight into edit mode; "生成覆盖版本" starts a genuinely fresh `TypeSession` (discarding
 * any edit-mode session already staged for this type) — the two are mutually exclusive *outcomes*
 * for the same type (only one `TypeSession` can occupy a given type slot at a time) but both
 * remain clickable, matching a built-in-covered row's own toggle behavior. If all three are
 * covered by something *and* the user hasn't chosen to override/edit any of them yet, nothing else
 * changes here — the "next step" gating in `index.tsx` already only requires `selectedTypes.
 * length > 0`. */
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
  /** Toggling a type that's already selected in "generate override" mode deselects it — per
   * spec's own Edge Case on switching types mid-session, the parent's `typeDeselected` reducer
   * action discards that type's entire `TypeSession` (all history included), not just hides it.
   * Also used by a built-in-covered type's "生成覆盖版本" button (selects exactly like an
   * uncovered type), and by an ai-generated-covered type's own "生成覆盖版本" button — for that
   * last case, if the type is currently in edit mode (`editingExistingNamespace` set) rather than
   * actually deselected, the caller re-selects into a fresh override session instead of
   * deselecting (see `index.tsx`'s own `onToggleType` handler). */
  onToggleType: (type: PluginType) => void
  /** Fired by an ai-generated-covered type's "编辑已有 AI 插件" button — jumps straight to edit
   * mode (dispatches `editExistingType`), bypassing the ordinary selection/shared-links steps. */
  onEditExisting: (type: PluginType, namespace: string, declaredNamespace: string, sourceCode: string) => void
}) {
  const { t } = useTranslation()

  return (
    <div>
      <p>{t("pluginWizard.selectTypesToCreate")}</p>
      {/* Deliberately NOT `.checklist` (legacy's own class, `lrr.css`) — that class hardcodes
          `height: 480px` for its usual long-scrollable-list use case (e.g. category multi-select),
          which for this fixed 3-item list left a large empty area below the checkboxes instead of
          the list sizing to its actual content. `list-style: none` replaces `.checklist`'s own
          bullet-suppression. No left/right margin — this list now lives inside the wizard's own
          fixed-width step container (`index.tsx`), so its checkboxes' left edge lines up flush
          against every other step's content instead of being inset from it. */}
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
                // A covered type's checkbox is permanently `checked+disabled` regardless of
                // `selected` (it's carrying the "covered by an installed plugin" signal, not a
                // real toggle state) — so this button, not the checkbox, is the only place
                // "already selected to override" can show at all. Toggling this on/off exactly
                // mirrors `onToggleType`'s own select/deselect pair; the button's own pressed
                // look (`.favtag-btn`/`.toggled`, same pattern as `WizardSteps.tsx`'s type tabs)
                // is what makes the click's effect visible (real user feedback, 2026-08-26: "点了
                // 后什么变化都没有" — the click *was* landing, `selectedTypes` really did update
                // and unlocked "下一步", but nothing on screen reflected it).
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
