import { useTranslation } from "react-i18next"

import { CollapsibleSection } from "@/components/Display"
import { FONT_SIZE_SM } from "@/theme"

import { CheckboxRow, Row } from "./shared"

/** Seconds-per-hour/day converters for the token-lifetime fields — wire format is always seconds. */
const SECS_PER_HOUR = 3600
const SECS_PER_DAY = 86400

export function SecuritySection({
  newPassword,
  setNewPassword,
  newPassword2,
  setNewPassword2,
  accessTokenLifetimeSecs,
  setAccessTokenLifetimeSecs,
  refreshTokenLifetimeSecs,
  setRefreshTokenLifetimeSecs,
  enablecors,
  setEnablecors,
}: {
  newPassword: string
  setNewPassword: (v: string) => void
  newPassword2: string
  setNewPassword2: (v: string) => void
  accessTokenLifetimeSecs: number
  setAccessTokenLifetimeSecs: (v: number) => void
  refreshTokenLifetimeSecs: number
  setRefreshTokenLifetimeSecs: (v: number) => void
  enablecors: boolean
  setEnablecors: (v: boolean) => void
}) {
  const { t } = useTranslation()

  return (
    <CollapsibleSection id="security" icon="fa-shield-alt" title={t("settings.security")}>
      <table style={{ margin: "auto", fontSize: FONT_SIZE_SM }}>
        <tbody>
          {/* Password login can no longer be disabled — fields are unconditionally visible now. */}
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
