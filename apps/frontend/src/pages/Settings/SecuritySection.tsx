import { useTranslation } from "react-i18next"

import { CollapsibleSection } from "@/components/Display"
import { FONT_SIZE_SM } from "@/theme"

import { CheckboxRow, Row } from "./shared"

/** Seconds-per-hour/day converters for the two token-lifetime fields below — the backend/wire
 *  format is always seconds (`Settings.access_token_lifetime_secs`/`refresh_token_lifetime_secs`),
 *  but a raw seconds count is a hostile unit for a human to type ("14400" vs. "4 hours"). */
const SECS_PER_HOUR = 3600
const SECS_PER_DAY = 86400

export function SecuritySection({
  enablepass,
  setEnablepass,
  newPassword,
  setNewPassword,
  newPassword2,
  setNewPassword2,
  nofunmode,
  setNofunmode,
  accessTokenLifetimeSecs,
  setAccessTokenLifetimeSecs,
  refreshTokenLifetimeSecs,
  setRefreshTokenLifetimeSecs,
  enablecors,
  setEnablecors,
}: {
  enablepass: boolean
  setEnablepass: (v: boolean) => void
  newPassword: string
  setNewPassword: (v: string) => void
  newPassword2: string
  setNewPassword2: (v: string) => void
  nofunmode: boolean
  setNofunmode: (v: boolean) => void
  accessTokenLifetimeSecs: number
  setAccessTokenLifetimeSecs: (v: number) => void
  refreshTokenLifetimeSecs: number
  setRefreshTokenLifetimeSecs: (v: number) => void
  enablecors: boolean
  setEnablecors: (v: boolean) => void
}) {
  const { t } = useTranslation()

  return (
    <CollapsibleSection icon="fa-shield-alt" title={t("settings.security")}>
      <table style={{ margin: "auto", fontSize: FONT_SIZE_SM }}>
        <tbody>
          <CheckboxRow id="enablepass" checked={enablepass} onChange={setEnablepass} label={t("settings.enablePassword")}>
            {t("settings.ifEnabledEverythingThatIsn")}
          </CheckboxRow>
          <CheckboxRow id="nofunmode" checked={nofunmode} onChange={setNofunmode} label={t("settings.nofunMode")}>
            {t("settings.requiresLoginForEveryRequest")}
          </CheckboxRow>
          {enablepass && (
            <>
              <Row label={t("settings.newPassword")}>
                <input
                  className="stdinput"
                  style={{ width: "100%" }}
                  maxLength={255}
                  value={newPassword}
                  onChange={(e) => setNewPassword(e.target.value)}
                  type="password"
                />
              </Row>
              <Row label={t("settings.newPasswordConfirmation")}>
                <input
                  className="stdinput"
                  style={{ width: "100%" }}
                  maxLength={255}
                  value={newPassword2}
                  onChange={(e) => setNewPassword2(e.target.value)}
                  type="password"
                />
                <br />
                {t("settings.onlyEditTheseFieldsIf")}
                <br />
                {t("settings.theOneAlreadyStoredWill")}
              </Row>
              <Row label={t("settings.loginSessionLifetime")}>
                <input
                  className="stdinput"
                  style={{ width: 80 }}
                  type="number"
                  min={1}
                  value={Math.round(accessTokenLifetimeSecs / SECS_PER_HOUR)}
                  onChange={(e) => setAccessTokenLifetimeSecs(Math.max(1, Number(e.target.value)) * SECS_PER_HOUR)}
                />{" "}
                {t("settings.hours")}
                <br />
                {t("settings.howLongYouStayLogged")}
              </Row>
              <Row label={t("settings.sessionRefreshWindow")}>
                <input
                  className="stdinput"
                  style={{ width: 80 }}
                  type="number"
                  min={1}
                  value={Math.round(refreshTokenLifetimeSecs / SECS_PER_DAY)}
                  onChange={(e) => setRefreshTokenLifetimeSecs(Math.max(1, Number(e.target.value)) * SECS_PER_DAY)}
                />{" "}
                {t("settings.days")}
                <br />
                {t("settings.howLongAfterLoggingIn")}
              </Row>
            </>
          )}
          <CheckboxRow id="enablecors" checked={enablecors} onChange={setEnablecors} label={t("settings.enableCorsForTheClient")}>
            {t("settings.haveApiRequestsSupportCrossorigin")}
            <br />
            {t("settings.turnThisOnIfYou")}
          </CheckboxRow>
        </tbody>
      </table>
    </CollapsibleSection>
  )
}
