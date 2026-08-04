import { useTranslation } from "react-i18next"

import { CollapsibleSection } from "@/components/Display"
import { FONT_SIZE_10PT } from "@/theme"

import { CheckboxRow, Row } from "./shared"

export function SecuritySection({
  enablepass,
  setEnablepass,
  newPassword,
  setNewPassword,
  newPassword2,
  setNewPassword2,
  nofunmode,
  setNofunmode,
  apikey,
  setApikey,
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
  apikey: string
  setApikey: (v: string) => void
  enablecors: boolean
  setEnablecors: (v: boolean) => void
}) {
  const { t } = useTranslation()

  return (
    <CollapsibleSection icon="fa-shield-alt" title={t("Security")}>
      <table style={{ margin: "auto", fontSize: FONT_SIZE_10PT }}>
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
              <Row label={t("API Key")}>
                <input className="stdinput" style={{ width: "100%" }} maxLength={255} value={apikey} onChange={(e) => setApikey(e.target.value)} type="text" />
                <br />
                {t("If you wish to use the Client API and have a password, you'll have to set a key here.")}
                <br />
                <span dangerouslySetInnerHTML={{ __html: t("Empty keys will <b>not</b> work!") }} />
                <br />
                <span dangerouslySetInnerHTML={{ __html: t("This key will need to be provided in every protected API call as the <i>Authorization</i> header.") }} />
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
