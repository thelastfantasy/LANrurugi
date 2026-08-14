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
    <CollapsibleSection icon="fa-shield-alt" title={t("Security")}>
      <table style={{ margin: "auto", fontSize: FONT_SIZE_SM }}>
        <tbody>
          <CheckboxRow id="enablepass" checked={enablepass} onChange={setEnablepass} label={t("Enable Password")}>
            {t("If enabled, everything that isn't reading will require a password.")}
          </CheckboxRow>
          {enablepass && (
            <>
              <Row label={t("New Password")}>
                <input
                  className="stdinput"
                  style={{ width: "100%" }}
                  maxLength={255}
                  value={newPassword}
                  onChange={(e) => setNewPassword(e.target.value)}
                  type="password"
                />
              </Row>
              <Row label={t("New Password Confirmation")}>
                <input
                  className="stdinput"
                  style={{ width: "100%" }}
                  maxLength={255}
                  value={newPassword2}
                  onChange={(e) => setNewPassword2(e.target.value)}
                  type="password"
                />
                <br />
                {t("Only edit these fields if you want to change your password.")}
                <br />
                {t("The one already stored will be used otherwise.")}
              </Row>
              <CheckboxRow id="nofunmode" checked={nofunmode} onChange={setNofunmode} label={t("No-Fun Mode")}>
                {t("Enabling No-Fun Mode will lock reading archives behind the password as well.")}
                <br />
                {t("Fully effective after restarting LANraragi.")}
              </CheckboxRow>
              <Row label={t("Login Session Lifetime")}>
                <input
                  className="stdinput"
                  style={{ width: 80 }}
                  type="number"
                  min={1}
                  value={Math.round(accessTokenLifetimeSecs / SECS_PER_HOUR)}
                  onChange={(e) => setAccessTokenLifetimeSecs(Math.max(1, Number(e.target.value)) * SECS_PER_HOUR)}
                />{" "}
                {t("hours")}
                <br />
                {t("How long you stay logged in before your browser needs to silently refresh its session (no re-login needed as long as the refresh window below hasn't also expired).")}
              </Row>
              <Row label={t("Session Refresh Window")}>
                <input
                  className="stdinput"
                  style={{ width: 80 }}
                  type="number"
                  min={1}
                  value={Math.round(refreshTokenLifetimeSecs / SECS_PER_DAY)}
                  onChange={(e) => setRefreshTokenLifetimeSecs(Math.max(1, Number(e.target.value)) * SECS_PER_DAY)}
                />{" "}
                {t("days")}
                <br />
                {t("How long after logging in you can stay away before actually needing to re-enter your password. Each silent refresh above extends this window from the moment you logged in, not from the refresh itself.")}
              </Row>
            </>
          )}
          <CheckboxRow id="enablecors" checked={enablecors} onChange={setEnablecors} label={t("Enable CORS for the Client API")}>
            {t("Have API requests support Cross-Origin Resource Sharing, which allows web browsers to access it off other domains.")}
            <br />
            {t("Turn this on if you want to access this service through a web-based wrapper (e.g. a userscript) used/hosted on another domain.")}
          </CheckboxRow>
        </tbody>
      </table>
    </CollapsibleSection>
  )
}
