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
 * rendered once the previous one is satisfied, per user feedback that having every type's entire
 * form/generate/trial-run/save block permanently expanded and stacked together (previously all of
 * this lived inline in `index.tsx`'s own render loop) looked cluttered and asked for too much
 * input up front. Wrapped in a bordered card (`.ptbox`) per type so multiple selected types read
 * as visually distinct sections rather than one long undifferentiated column. */
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
  /** The wizard session's own reducer dispatch — passed through directly rather than a pile of
   * individual callback props, since this component fires nearly every action kind. */
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
      {/* Substep 1: form — always visible, the entry point for this type. */}
      <TypeDetailsForm
        session={session}
        typeSession={typeSession}
        onPatch={(patch) => dispatch({ kind: "typeSessionUpdated", type, patch })}
        // Previously only true when "login" was *also* selected for generation this session — but
        // a domain whose login plugin is already installed (found covered at the initial lookup
        // step) never puts "login" in `selectedTypes` at all (there's nothing left to generate for
        // it), so this question never showed up for the far more common case: an existing login
        // plugin the user still needs to answer "does this type depend on it" for. Also asking
        // whenever the domain lookup found login covered fixes that (a real reported gap,
        // 2026-08-25 — `dependsOnLogin` had no way to ever become `true` for an existing login
        // plugin, so `loginAssociation` never got set either, and the generated code never
        // declared `login_from` at all).
        showLoginDependencyQuestion={
          type !== "login" && (session.selectedTypes.includes("login") || session.lookupResult.login.covered)
        }
      />

      {/* Substep 2: generate — hidden until the form's own minimum requirements are met (FR-005),
          rather than shown-but-disabled, so the page doesn't ask the user to look at a generate
          button before there's anything valid to generate from. */}
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

      {/* Substep 3/4: trial-run, edit, history, and (once TrialRunResult's own canSaveFor gate
          passes) confirm-save — hidden until at least one draft revision exists. TrialRunResult
          itself already collapses to a simple "Installed as ..." line once savedNamespace is set
          (US6), so this one block covers both trial-run/save substeps without a separate wrapper
          for each. */}
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
                // A manual edit still becomes a real turn in the replayed conversation (see
                // `TypeSession.conversationHistory`'s own docs) — otherwise a later AI refine
                // round would only ever see the last *AI-authored* code, silently discarding
                // whatever the user just hand-edited on top of it.
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
          {/* T043 (US7): a login TypeSession this run generated only has a real, resolvable
              namespace once it's actually saved (`with_login_cookies`-style resolution needs a
              real installed plugin file, not merely a trial-run-succeeded draft still sitting in
              `custom/_wizard/`) — so the link action becomes available once `savedNamespace` is
              set, offering to associate it with a type still missing an association and kick off
              a fresh generate call carrying that association (FR-008's login_from declaration,
              FR-025's "automatically update the previously failed draft"). */}
          {type !== "login" &&
            !typeSession.loginAssociation &&
            (() => {
              // loginAssociation.namespace must be the plugin's own self-declared
              // pluginInfo().namespace (savedDeclaredNamespace), not the file-path
              // savedNamespace — see save.rs's SaveResponse doc comment on why the two differ and
              // which one login_from resolution actually matches against.
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
