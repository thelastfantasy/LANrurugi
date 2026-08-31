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

/** AI plugin creation wizard — a step container that renders whichever step the current session
 * state calls for, one step at a time (`session.currentStep`). */
export function PluginWizard() {
  const { t } = useTranslation()
  useApplyTheme()
  useDocumentTitle(t("pluginWizard.pageTitle") ?? undefined)

  const { session, dispatch, isInFlight, beginOperation, endOperation } = useWizardSession()
  const settings = useSettings()
  const [lastDomain, setLastDomain] = useState("")

  return (
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

      {/* Proactive heads-up shown before the user invests time filling in the form, since only
          generation itself (not lookup/trial-run/save) needs an LLM key. */}
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
