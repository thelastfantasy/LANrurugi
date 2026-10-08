import { useTranslation } from "react-i18next"

import type { SubscriptionSource } from "@/api/types"

/** Which plugins a source will actually use, and a warning when the login one is missing.
 *
 * Worth stating up front because a missing sign-in does not fail — the source answers a signed-out
 * request with a *smaller* listing, so a subscription built on one quietly tracks less than the user
 * believes, with nothing anywhere reporting it. */
export function SourcePlugins({
  source,
  credentials,
  onCredentialsChange,
}: {
  source: SubscriptionSource
  credentials?: { cookies?: string; headers?: string }
  onCredentialsChange: (next: { cookies?: string; headers?: string }) => void
}) {
  const { t } = useTranslation()
  const login = source.login_plugin
  const missingLogin = login != null && login.resolved === null

  return (
    <div className="source-plugins">
      {source.download_plugin && (
        <span>
          {t("subscriptions.usesDownloadPlugin", { name: source.download_plugin })}
        </span>
      )}
      {login?.resolved && (
        <span>{t("subscriptions.usesLoginPlugin", { name: login.resolved })}</span>
      )}
      {missingLogin && (
        <span className="source-plugins-warning">
          {t("subscriptions.noLoginPlugin", { declared: login.declared })}
        </span>
      )}

      {/* Offered only where no login plugin answers: with one installed this would be a second,
          staler way to do the same thing. */}
      {missingLogin && (
        <details className="source-credentials">
          <summary>{t("subscriptions.manualCredentials")}</summary>
          <label>
            {t("subscriptions.cookies")}
            <input
              className="stdinput"
              value={credentials?.cookies ?? ""}
              placeholder="name=value; other=value"
              onChange={(e) => onCredentialsChange({ ...credentials, cookies: e.target.value })}
            />
          </label>
          <label>
            {t("subscriptions.headers")}
            <textarea
              className="stdinput"
              rows={3}
              value={credentials?.headers ?? ""}
              placeholder={"Authorization: Bearer …\nX-Api-Key: …"}
              onChange={(e) => onCredentialsChange({ ...credentials, headers: e.target.value })}
            />
          </label>
          <p className="sub-form-hint">{t("subscriptions.credentialsHint")}</p>
        </details>
      )}
    </div>
  )
}
