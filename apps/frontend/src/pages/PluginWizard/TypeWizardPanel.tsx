import { useTranslation } from "react-i18next"

import { DraftEditor } from "./DraftEditor"
import { DraftHistoryPanel } from "./DraftHistoryPanel"
import { GenerationStep } from "./GenerationStep"
import { LoginSuggestionPanel } from "./LoginSuggestionPanel"
import { RefinePanel } from "./RefinePanel"
import { TrialRunResult } from "./TrialRunResult"
import { TypeDetailsForm } from "./TypeDetailsForm"
import type {
  ConversationTurn,
  DraftRevision,
  PluginType,
  TrialRunResult as TrialRunResultData,
  TypeSession,
  WizardSession,
} from "./useWizardSession"
import { isFormComplete } from "./useWizardSession"

/** One type's full four-substep flow — form → generate → trial-run → save — each substep only
 * rendered once the previous one is satisfied. */
export function TypeWizardPanel({
  session,
  typeSession,
  type,
  isInFlight,
  onBeginOperation,
  onEndOperation,
  dispatch,
}: {
  session: WizardSession
  typeSession: TypeSession
  type: PluginType
  isInFlight: boolean
  onBeginOperation: () => void
  onEndOperation: () => void
  /** The wizard session's own reducer dispatch, passed through directly. */
  dispatch: (
    action:
      | { kind: "typeSessionUpdated"; type: PluginType; patch: Partial<TypeSession> }
      | { kind: "revisionAppended"; type: PluginType; revision: DraftRevision }
      | { kind: "conversationTurnAppended"; type: PluginType; turn: ConversationTurn }
      | { kind: "activeRevisionChanged"; type: PluginType; index: number }
      | { kind: "trialRunAppended"; type: PluginType; revisionId: string; result: TrialRunResultData }
      | { kind: "typeSelected"; type: PluginType },
  ) => void
}) {
  const { t } = useTranslation()
  const activeRevision = typeSession.revisions[typeSession.activeRevisionIndex]
  const formComplete = isFormComplete(session, typeSession)
  const hasDraft = typeSession.revisions.length > 0

  return (
    <div className="ptbox" style={{ marginTop: 12, padding: 12 }}>
      <TypeDetailsForm
        session={session}
        typeSession={typeSession}
        onPatch={(patch) => dispatch({ kind: "typeSessionUpdated", type, patch })}
        showLoginDependencyQuestion={
          type !== "login" && (session.selectedTypes.includes("login") || session.lookupResult.login.covered)
        }
      />

      {formComplete && !typeSession.savedNamespace && (
        <div style={{ marginTop: 12, borderTop: "1px dashed #999", paddingTop: 8 }}>
          <GenerationStep
            session={session}
            typeSession={typeSession}
            type={type}
            isInFlight={isInFlight}
            onBeginOperation={onBeginOperation}
            onEndOperation={onEndOperation}
            onRevisionAppended={(revision) => dispatch({ kind: "revisionAppended", type, revision })}
            onConversationTurnAppended={(turn) => dispatch({ kind: "conversationTurnAppended", type, turn })}
            onCredentialValuesResolved={(resolvedCredentialValues) =>
              dispatch({ kind: "typeSessionUpdated", type, patch: { resolvedCredentialValues } })
            }
          />
        </div>
      )}

      {hasDraft && (
        <div style={{ marginTop: 12, borderTop: "1px dashed #999", paddingTop: 8 }}>
          {activeRevision?.explanation && (
            <div className="ptbox" style={{ padding: 8, marginBottom: 8 }}>
              <h3 className="ih">{t("pluginWizard.explanationHeading")}</h3>
              <p style={{ whiteSpace: "pre-wrap", margin: 0 }}>{activeRevision.explanation}</p>
            </div>
          )}
          {!typeSession.savedNamespace && (
            <DraftEditor
              activeRevision={activeRevision}
              disabled={isInFlight}
              onApply={(code) => {
                const revision: DraftRevision = {
                  id: crypto.randomUUID(),
                  code,
                  origin: "manual-edit",
                  createdAt: Date.now(),
                  trialRuns: [],
                }
                dispatch({ kind: "revisionAppended", type, revision })
                dispatch({
                  kind: "conversationTurnAppended",
                  type,
                  turn: { userMessage: "用户手动修改了代码。", assistantCode: code },
                })
              }}
            />
          )}
          <TrialRunResult
            session={session}
            domain={session.domain}
            typeSession={typeSession}
            type={type}
            activeRevision={activeRevision}
            isInFlight={isInFlight}
            onBeginOperation={onBeginOperation}
            onEndOperation={onEndOperation}
            onTrialRunAppended={(result) =>
              activeRevision && dispatch({ kind: "trialRunAppended", type, revisionId: activeRevision.id, result })
            }
            onSaved={(namespace, declaredNamespace) =>
              dispatch({
                kind: "typeSessionUpdated",
                type,
                patch: { savedNamespace: namespace, savedDeclaredNamespace: declaredNamespace },
              })
            }
            onRevisionAppended={(revision) => dispatch({ kind: "revisionAppended", type, revision })}
            onConversationTurnAppended={(turn) => dispatch({ kind: "conversationTurnAppended", type, turn })}
            onCredentialValuesResolved={(resolvedCredentialValues) =>
              dispatch({ kind: "typeSessionUpdated", type, patch: { resolvedCredentialValues } })
            }
          />
          {!typeSession.savedNamespace && (
            <RefinePanel
              session={session}
              typeSession={typeSession}
              type={type}
              activeRevision={activeRevision}
              isInFlight={isInFlight}
              onBeginOperation={onBeginOperation}
              onEndOperation={onEndOperation}
              onRevisionAppended={(revision) => dispatch({ kind: "revisionAppended", type, revision })}
              onConversationTurnAppended={(turn) => dispatch({ kind: "conversationTurnAppended", type, turn })}
              onCredentialValuesResolved={(resolvedCredentialValues) =>
                dispatch({ kind: "typeSessionUpdated", type, patch: { resolvedCredentialValues } })
              }
            />
          )}
          {typeSession.revisions.length > 1 && (
            <DraftHistoryPanel
              typeSession={typeSession}
              onActiveRevisionChanged={(index) => dispatch({ kind: "activeRevisionChanged", type, index })}
            />
          )}
          {type !== "login" &&
            (() => {
              const lastRun = activeRevision?.trialRuns[activeRevision.trialRuns.length - 1]
              const suggestion = lastRun && lastRun.type !== "login" ? lastRun.loginSuggestion : undefined
              if (!suggestion) return null
              return (
                <LoginSuggestionPanel
                  session={session}
                  suggestion={suggestion}
                  onAssociateExisting={(namespace) =>
                    dispatch({
                      kind: "typeSessionUpdated",
                      type,
                      patch: { loginAssociation: { namespace, source: "existing" } },
                    })
                  }
                  onSelectLoginType={() => dispatch({ kind: "typeSelected", type: "login" })}
                />
              )
            })()}
          {type !== "login" &&
            !typeSession.loginAssociation &&
            (() => {
              const loginDeclaredNamespace = session.typeSessions.login?.savedDeclaredNamespace
              if (!loginDeclaredNamespace) return null
              return (
                <div className="ptbox" style={{ marginTop: 8, padding: 8 }}>
                  <input
                    type="button"
                    className="stdbtn"
                    value={t("pluginWizard.linkGeneratedLoginPlugin", { namespace: loginDeclaredNamespace }) ?? ""}
                    onClick={() =>
                      dispatch({
                        kind: "typeSessionUpdated",
                        type,
                        patch: {
                          dependsOnLogin: true,
                          loginAssociation: { namespace: loginDeclaredNamespace, source: "generated-this-session" },
                        },
                      })
                    }
                  />
                </div>
              )
            })()}
        </div>
      )}
    </div>
  )
}
