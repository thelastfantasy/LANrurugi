import { useState } from "react"
import { useTranslation } from "react-i18next"
import { Link } from "react-router-dom"

import { useSettings } from "@/api/hooks"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { useApplyTheme } from "@/theme"

import { DomainLookupStep } from "./DomainLookupStep"
import { SharedLinksForm } from "./SharedLinksForm"
import { TypeSelectionStep } from "./TypeSelectionStep"
import { TypeWizardPanel } from "./TypeWizardPanel"
import { isSharedLinksStepComplete, useWizardSession } from "./useWizardSession"
import { WizardSteps } from "./WizardSteps"

/** AI plugin creation wizard (`specs/006-ai-plugin-wizard`) — a step container that renders
 * whichever step the current session state calls for. Session-only (FR-023, enforced server-side
 * via `route_policy.csv`, not re-checked here — a logged-out user simply can't successfully call
 * any of the four endpoints this page drives).
 *
 * Genuinely one step at a time (`session.currentStep`), not the previous "render everything
 * stacked" layout — real user feedback, 2026-08-26: "首先从上到下的瀑布式布局就要改掉". Only ever
 * mounts a single `TypeWizardPanel` (for `session.activeType`), switchable via `WizardSteps`'s own
 * pill-tab row when more than one type is selected — each type's own `TypeSession` still lives in
 * the reducer regardless of which one is currently shown, so switching away and back loses
 * nothing. */
export function PluginWizard() {
  const { t } = useTranslation()
  useApplyTheme()
  useDocumentTitle(t("pluginWizard.pageTitle") ?? undefined)

  const { session, dispatch, isInFlight, beginOperation, endOperation } = useWizardSession()
  const settings = useSettings()
  // Remembers the domain across a `reset` back to the lookup step (`session` itself becomes
  // `null` on reset, so this can't be read off `session.domain` after the fact) — real user
  // feedback, 2026-08-26: clicking "上一步" from 选择类型 back to 查找域名 emptied the domain
  // field instead of keeping what was already looked up.
  const [lastDomain, setLastDomain] = useState("")

  return (
    // `textAlign: "left"` overrides legacy's own `body { text-align: center }` base rule once
    // here, at the container level, rather than needing every descendant (steps, forms, buttons)
    // to set it individually — real user feedback, 2026-08-26: the whole wizard read as
    // center-aligned when it should read as a normal left-to-right form.
    <div
      className="ido"
      style={{
        maxWidth: 720,
        marginLeft: "auto",
        marginRight: "auto",
        paddingLeft: 12,
        paddingRight: 12,
        boxSizing: "border-box",
        textAlign: "left",
      }}
    >
      <h1 className="ih">{t("pluginWizard.pageTitle")}</h1>

      {/* FR-024 covers the failure itself (a 503 once the user actually hits "generate"); this
          is a proactive heads-up shown as soon as the page loads, before the user invests time
          filling in the lookup/description/links, since domain lookup/trial-run/save all work
          fine without a key — only generation itself needs one. */}
      {settings.data && !settings.data.llm_api_key_set && (
        <p className="ptbox" style={{ padding: 8 }}>
          {t("pluginWizard.llmKeyNotConfiguredHint")}{" "}
          <Link to={routes.settings()}>{t("pluginWizard.llmKeyNotConfiguredLinkText")}</Link>
        </p>
      )}

      {!session && (
        <>
          <p>{t("pluginWizard.domainLookupPlaceholder")}</p>
          <DomainLookupStep
            initialDomain={lastDomain}
            onLookupSucceeded={(domain, result) => dispatch({ kind: "lookupSucceeded", domain, result })}
          />
        </>
      )}

      {session && (
        <>
          <WizardSteps
            currentStep={session.currentStep}
            selectedTypes={session.selectedTypes}
            activeType={session.activeType}
            onBack={() => {
              if (session.currentStep === "typeSelection") {
                setLastDomain(session.domain)
                dispatch({ kind: "reset" })
              } else {
                dispatch({ kind: "stepBack" })
              }
            }}
            onGoToLookup={() => {
              setLastDomain(session.domain)
              dispatch({ kind: "reset" })
            }}
            onGoToStep={(step) => dispatch({ kind: "goToStep", step })}
            onActiveTypeChanged={(type) => dispatch({ kind: "activeTypeChanged", type })}
            nextStep={
              session.currentStep === "typeSelection"
                ? {
                    disabled: session.selectedTypes.length === 0,
                    onClick: () => dispatch({ kind: "advanceToSharedLinks" }),
                  }
                : session.currentStep === "sharedLinks"
                  ? {
                      disabled: !isSharedLinksStepComplete(session),
                      onClick: () => dispatch({ kind: "advanceToTypeDetail", type: session.selectedTypes[0] }),
                    }
                  : undefined
            }
          />

          {session.currentStep === "typeSelection" && (
            <TypeSelectionStep
              lookupResult={session.lookupResult}
              selectedTypes={session.selectedTypes}
              typeSessions={session.typeSessions}
              onToggleType={(type) => {
                const isEditingExisting = session.typeSessions[type]?.editingExistingNamespace != null
                // A type currently in edit mode isn't really "deselected" from 生成覆盖's own
                // point of view — clicking 生成覆盖版本 there means "start a fresh override
                // instead of editing", i.e. select into a brand new TypeSession, same as an
                // uncovered type's first selection. Only an already-in-override-mode type
                // (selected AND not editing) actually deselects on a second click.
                dispatch(
                  session.selectedTypes.includes(type) && !isEditingExisting
                    ? { kind: "typeDeselected", type }
                    : { kind: "typeSelected", type },
                )
              }}
              onEditExisting={(type, namespace, declaredNamespace, sourceCode) =>
                dispatch({ kind: "editExistingType", type, namespace, declaredNamespace, sourceCode })
              }
            />
          )}

          {session.currentStep === "sharedLinks" && (
            <SharedLinksForm
              links={session.sharedLinks}
              onChange={(links) => dispatch({ kind: "sharedLinksChanged", links })}
            />
          )}

          {session.currentStep === "typeDetail" &&
            session.activeType &&
            (() => {
              const type = session.activeType
              const typeSession = session.typeSessions[type]
              if (!typeSession) return null
              return (
                <TypeWizardPanel
                  key={type}
                  session={session}
                  typeSession={typeSession}
                  type={type}
                  isInFlight={isInFlight(type)}
                  onBeginOperation={() => beginOperation(type)}
                  onEndOperation={() => endOperation(type)}
                  dispatch={dispatch}
                />
              )
            })()}
        </>
      )}
    </div>
  )
}
