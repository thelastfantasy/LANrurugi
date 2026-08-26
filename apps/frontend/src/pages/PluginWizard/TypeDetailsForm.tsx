import { useState } from "react"
import { useTranslation } from "react-i18next"

import { ApiError } from "@/api/apiError"
import { sendJson } from "@/api/client"
import { AiSkeleton } from "@/components/Display/AiSkeleton"
import { toast } from "@/toast"

import type { LoginParameter, PluginType, TypeSession, WizardSession } from "./useWizardSession"
import { cleanLinks } from "./useWizardSession"

/** FR-005/FR-006/FR-007: per-type input collection layered on top of `WizardSession.sharedLinks`
 * (the one domain-level link box every type draws from — see that field's own doc comment for why
 * links were merged into a single shared input rather than one box per type). Metadata/download
 * need nothing further here beyond the login-dependency question — their own generation/trial-run
 * reads `sharedLinks` directly (`GenerationStep.tsx`/`TrialRunResult.tsx`). Login is the one type
 * with real per-type work left: an AI-analyzed credential-field form
 * (`/plugin-wizard/analyze-login` inspects the shared links' first entry — or a page/API doc among
 * them the user points at instead — and decides whether the site needs a password pair, a single
 * token/API key, a cookie value, or something else; never assumed up front). No free-text
 * page-feature description anywhere — AI infers page structure itself via `fetch_page` against the
 * real shared links. */
export function TypeDetailsForm({
  session,
  typeSession,
  onPatch,
  showLoginDependencyQuestion,
}: {
  session: WizardSession
  typeSession: TypeSession
  onPatch: (patch: Partial<TypeSession>) => void
  /** FR-007: only true when this run's selection includes both "login" and this (metadata/
   * download) type — the question must be asked explicitly per type, never assumed from
   * "login was selected too". Always `false`/ignored for the login type itself. */
  showLoginDependencyQuestion: boolean
}) {
  const { t } = useTranslation()
  const needsLinks = typeSession.type === "metadata" || typeSession.type === "download"
  const [analyzing, setAnalyzing] = useState(false)
  const [loginReferenceUrlOverride, setLoginReferenceUrlOverride] = useState("")
  // Defaults to the shared links' first entry (per real user feedback: a single API doc URL
  // often documents auth alongside metadata/download, so login analysis shouldn't need its own
  // separately-pasted URL in the common case) — still freely overridable per-type below.
  const loginReferenceUrl = loginReferenceUrlOverride || cleanLinks(session.sharedLinks)[0] || ""

  async function analyzeLogin() {
    if (!loginReferenceUrl.trim()) return
    setAnalyzing(true)
    try {
      const response = await sendJson<{ parameters: LoginParameter[] }>(
        "POST",
        "/plugin-wizard/analyze-login",
        { reference_url: loginReferenceUrl.trim() },
      )
      onPatch({ loginParameters: response.parameters, loginFieldValues: {} })
    } catch (err) {
      const heading =
        err instanceof ApiError && err.status === 422
          ? t("pluginWizard.loginAnalysisNotParameters")
          : t("pluginWizard.loginAnalysisFailed")
      toast({ heading: heading ?? undefined, text: String(err), icon: "error" })
    } finally {
      setAnalyzing(false)
    }
  }

  return (
    <div style={{ marginTop: 12 }}>
      <h2>{t(`pluginWizard.type.${typeSession.type as PluginType}`)}</h2>

      {showLoginDependencyQuestion && needsLinks && (
        // Not a native <fieldset>/<legend> — its browser-drawn default border rendered as a stray,
        // visually inconsistent vertical rule against this theme's background (real user feedback,
        // 2026-08-26). Same `.favtag-btn`/`.toggled` pill pattern `WizardSteps.tsx`'s own type-tab
        // row uses (itself borrowed from `dialog.tsx`'s `NewCategoryForm`), just semantically a
        // `radiogroup` of two mutually exclusive options instead of tabs.
        <div style={{ marginBottom: 8 }}>
          <p style={{ margin: "0 0 4px", fontWeight: "bold" }}>{t("pluginWizard.dependsOnLoginQuestion")}</p>
          <div role="radiogroup" style={{ display: "flex", gap: 4, maxWidth: 200 }}>
            <button
              type="button"
              role="radio"
              aria-checked={typeSession.dependsOnLogin === true}
              className={`favtag-btn${typeSession.dependsOnLogin === true ? " toggled" : ""}`}
              style={{ flex: 1 }}
              onClick={() => {
                // Answering "yes" here used to only set `dependsOnLogin` (FR-013's generation-
                // ordering gate) and leave `loginAssociation` untouched — the *only* two things
                // that ever set it were an after-the-fact AI trial-run suggestion or a manual
                // "link the login plugin I just generated" click, so a domain whose login plugin
                // was already installed (found at the initial lookup step) never actually got
                // associated at all: the generated code never declared `login_from`, silently
                // defeating the whole point of answering "yes" (real report, 2026-08-25). Prefers
                // whatever the domain lookup already found installed; falls back to a login
                // plugin *this session* generated and saved; leaves the existing association
                // (if any) alone if neither source has anything yet — a later AI suggestion or
                // manual link can still fill it in.
                const existingLogin = session.lookupResult.login
                const sessionLogin = session.typeSessions.login
                const loginAssociation = existingLogin.covered
                  ? { namespace: existingLogin.declaredNamespace, source: "existing" as const }
                  : sessionLogin?.savedDeclaredNamespace
                    ? { namespace: sessionLogin.savedDeclaredNamespace, source: "generated-this-session" as const }
                    : typeSession.loginAssociation
                onPatch({ dependsOnLogin: true, loginAssociation })
              }}
            >
              {t("common.yes")}
            </button>
            <button
              type="button"
              role="radio"
              aria-checked={typeSession.dependsOnLogin === false}
              className={`favtag-btn${typeSession.dependsOnLogin === false ? " toggled" : ""}`}
              style={{ flex: 1 }}
              onClick={() => onPatch({ dependsOnLogin: false })}
            >
              {t("common.no")}
            </button>
          </div>
        </div>
      )}

      {!needsLinks && (
        <div>
          <label>
            {t("pluginWizard.loginReferenceUrlHint")}
            <div style={{ display: "flex", gap: 6 }}>
              <input
                className="stdinput"
                type="text"
                value={loginReferenceUrl}
                placeholder={t("pluginWizard.loginReferenceUrlPlaceholder") ?? undefined}
                onChange={(e) => setLoginReferenceUrlOverride(e.target.value)}
                style={{ flex: "1 1 auto" }}
              />
              <input
                type="button"
                className="stdbtn"
                value={
                  t(typeSession.loginParameters ? "pluginWizard.reanalyzeLogin" : "pluginWizard.analyzeLogin") ?? ""
                }
                disabled={analyzing || !loginReferenceUrl.trim()}
                onClick={() => void analyzeLogin()}
              />
            </div>
          </label>
          {analyzing && <AiSkeleton />}

          {typeSession.loginParameters && (
            <div style={{ marginTop: 8 }}>
              {typeSession.loginParameters.map((param) => (
                <label key={param.name} style={{ display: "block", marginTop: 4 }}>
                  {param.description}
                  {param.required && " *"}
                  <input
                    className="stdinput"
                    type={/secret|password|token|key|cookie/i.test(param.name) ? "password" : "text"}
                    value={typeSession.loginFieldValues[param.name] ?? ""}
                    placeholder={param.description}
                    onChange={(e) =>
                      onPatch({ loginFieldValues: { ...typeSession.loginFieldValues, [param.name]: e.target.value } })
                    }
                    style={{ width: "100%", display: "block" }}
                  />
                </label>
              ))}
            </div>
          )}
        </div>
      )}
    </div>
  )
}
