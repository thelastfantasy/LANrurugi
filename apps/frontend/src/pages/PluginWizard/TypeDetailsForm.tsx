import { useState } from "react"
import { useTranslation } from "react-i18next"

import { ApiError } from "@/api/apiError"
import { sendJson } from "@/api/client"
import { AiSkeleton } from "@/components/Display/AiSkeleton"
import { toast } from "@/toast"

import type { LoginParameter, PluginType, TypeSession, WizardSession } from "./useWizardSession"
import { cleanLinks } from "./useWizardSession"

/** Per-type input collection layered on top of `WizardSession.sharedLinks`. Login is the one
 * type with real per-type work: an AI-analyzed credential-field form via `/plugin-wizard/analyze-login`. */
export function TypeDetailsForm({
  session,
  typeSession,
  onPatch,
  showLoginDependencyQuestion,
}: {
  session: WizardSession
  typeSession: TypeSession
  onPatch: (patch: Partial<TypeSession>) => void
  /** True only when this run's selection includes both "login" and this type. Always
   * false/ignored for the login type itself. */
  showLoginDependencyQuestion: boolean
}) {
  const { t } = useTranslation()
  const needsLinks = typeSession.type === "metadata" || typeSession.type === "download"
  const [analyzing, setAnalyzing] = useState(false)
  const [loginReferenceUrlOverride, setLoginReferenceUrlOverride] = useState("")
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
