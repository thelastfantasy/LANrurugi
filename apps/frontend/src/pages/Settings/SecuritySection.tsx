import { useState } from "react"
import { useTranslation } from "react-i18next"

import { useRenameSession, useRevokeSession, useSessions } from "@/api/hooks"
import { Button, Input, NumberInput } from "@/components/common-ui/Form"
import { CollapsibleSection } from "@/components/Display"
import { confirmDialog } from "@/dialog"
import { FONT_SIZE_SM } from "@/theme"
import { toast } from "@/toast"

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
  refreshTokenIdleLifetimeSecs,
  setRefreshTokenIdleLifetimeSecs,
  maxLoginDevices,
  setMaxLoginDevices,
  trustedOrigins,
  setTrustedOrigins,
  cookieDomain,
  setCookieDomain,
  ssoAutoRedirect,
  setSsoAutoRedirect,
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
  refreshTokenIdleLifetimeSecs: number
  setRefreshTokenIdleLifetimeSecs: (v: number) => void
  maxLoginDevices: number
  setMaxLoginDevices: (v: number) => void
  trustedOrigins: string
  setTrustedOrigins: (v: string) => void
  cookieDomain: string
  setCookieDomain: (v: string) => void
  ssoAutoRedirect: boolean
  setSsoAutoRedirect: (v: boolean) => void
  enablecors: boolean
  setEnablecors: (v: boolean) => void
}) {
  const { t } = useTranslation()

  return (
    <CollapsibleSection id="security" icon="fa-shield-alt" title={t("settings.security")}>
      <div className="settings-table" style={{ margin: "auto", fontSize: FONT_SIZE_SM }}>
          {/* Password login can no longer be disabled — fields are unconditionally visible now. */}
          <Row label={t("settings.newPassword")}>
            <Input
              style={{ width: "100%" }}
              maxLength={255}
              value={newPassword}
              onValueChange={(value) => setNewPassword(value)}
              type="password"
            />
          </Row>
          <Row label={t("settings.newPasswordConfirmation")}>
            <Input
              style={{ width: "100%" }}
              maxLength={255}
              value={newPassword2}
              onValueChange={(value) => setNewPassword2(value)}
              type="password"
            />
            <br />
            {t("settings.onlyEditTheseFieldsIf")}
            <br />
            {t("settings.theOneAlreadyStoredWill")}
          </Row>
          <Row label={t("settings.loginSessionLifetime")}>
            <NumberInput
              style={{ width: 80 }}
              min={1}
              value={Math.round(accessTokenLifetimeSecs / SECS_PER_HOUR)}
              onValueChange={(v) => setAccessTokenLifetimeSecs(Math.max(1, v) * SECS_PER_HOUR)}
            />{" "}
            {t("settings.hours")}
            <br />
            {t("settings.howLongYouStayLogged")}
          </Row>
          <Row label={t("settings.sessionRefreshIdleWindow")}>
            <NumberInput
              style={{ width: 80 }}
              min={1}
              value={Math.round(refreshTokenIdleLifetimeSecs / SECS_PER_DAY)}
              onValueChange={(v) => setRefreshTokenIdleLifetimeSecs(Math.max(1, v) * SECS_PER_DAY)}
            />{" "}
            {t("settings.days")}
            <br />
            {t("settings.sessionRefreshIdleWindowHint")}
          </Row>
          <Row label={t("settings.sessionRefreshAbsoluteWindow")}>
            <NumberInput
              style={{ width: 80 }}
              min={1}
              value={Math.round(refreshTokenLifetimeSecs / SECS_PER_DAY)}
              onValueChange={(v) => setRefreshTokenLifetimeSecs(Math.max(1, v) * SECS_PER_DAY)}
            />{" "}
            {t("settings.days")}
            <br />
            {t("settings.sessionRefreshAbsoluteWindowHint")}
          </Row>
          <Row label={t("settings.maxLoginDevices")}>
            <NumberInput
              style={{ width: 80 }}
              min={0}
              value={maxLoginDevices}
              onValueChange={(v) => setMaxLoginDevices(Math.max(0, v))}
            />
            <br />
            {t("settings.maxLoginDevicesHint")}
          </Row>
          <Row label={t("settings.trustedOrigins")}>
            <Input
              rows={3}
              style={{ width: "100%", minHeight: 60, height: 60 }}
              value={trustedOrigins}
              onValueChange={(value) => setTrustedOrigins(value)}
              placeholder={"https://a.com\nhttps://b.com"}
            />
            <br />
            {t("settings.trustedOriginsHint")}
          </Row>
          <Row label={t("settings.cookieDomain")}>
            <Input
              style={{ width: "100%" }}
              value={cookieDomain}
              onValueChange={(value) => setCookieDomain(value)}
              placeholder=".example.com"
            />
            <br />
            {t("settings.cookieDomainHint")}
          </Row>
          <CheckboxRow
            id="ssoAutoRedirect"
            checked={ssoAutoRedirect}
            onChange={setSsoAutoRedirect}
            label={t("settings.ssoAutoRedirect")}
          >
            {t("settings.ssoAutoRedirectHint")}
          </CheckboxRow>
          <CheckboxRow id="enablecors" checked={enablecors} onChange={setEnablecors} label={t("settings.enableCorsForTheClient")}>
            {t("settings.haveApiRequestsSupportCrossorigin")}
            <br />
            {t("settings.turnThisOnIfYou")}
          </CheckboxRow>
      </div>
      <ActiveSessionsList />
    </CollapsibleSection>
  )
}

function formatTimestamp(seconds: number): string {
  return new Date(seconds * 1000).toLocaleString()
}

/** Active login devices for the current admin, with inline rename and revoke. Lives inside the
 *  Security accordion because it is account-access state, not a generic display preference. */
function ActiveSessionsList() {
  const { t } = useTranslation()
  const sessions = useSessions()
  const renameSession = useRenameSession()
  const revokeSession = useRevokeSession()
  const [editingId, setEditingId] = useState<string | null>(null)
  const [draftName, setDraftName] = useState("")

  async function handleRename(familyId: string) {
    const name = draftName.trim()
    if (!name) return
    try {
      await renameSession.mutateAsync({ familyId, name })
      setEditingId(null)
      toast({ text: t("settings.deviceNameUpdated"), icon: "success" })
    } catch {
      toast({ heading: t("settings.errorUpdatingDevice"), icon: "error" })
    }
  }

  async function handleRevoke(familyId: string, current: boolean) {
    const confirmed = await confirmDialog(
      t(current ? "settings.confirmRevokeCurrentDevice" : "settings.confirmRevokeDevice") ?? "",
      true,
    )
    if (!confirmed) return
    try {
      await revokeSession.mutateAsync(familyId)
      toast({ text: t("settings.deviceRevoked"), icon: "success" })
    } catch {
      toast({ heading: t("settings.errorUpdatingDevice"), icon: "error" })
    }
  }

  return (
    <div className="settings-table" style={{ margin: "auto", fontSize: FONT_SIZE_SM }}>
      <Row label={t("settings.activeDevices")}>
        {sessions.isLoading && <span>{t("common.loadingLibrary")}</span>}
        {sessions.isError && <span>{t("settings.errorLoadingDevices")}</span>}
        {sessions.data && sessions.data.length === 0 && <span>{t("settings.noActiveDevices")}</span>}
        {sessions.data && sessions.data.length > 0 && (
          <div style={{ display: "flex", flexDirection: "column", gap: 8 }}>
            {sessions.data.map((session) => (
              <div
                key={session.family_id}
                style={{
                  border: "1px solid rgba(127,127,127,0.35)",
                  borderRadius: 4,
                  padding: "8px 10px",
                  display: "flex",
                  flexDirection: "column",
                  gap: 6,
                }}
              >
                <div style={{ display: "flex", gap: 8, alignItems: "center", flexWrap: "wrap" }}>
                  {editingId === session.family_id ? (
                    <>
                      <Input
                        style={{ flex: "1 1 220px" }}
                        value={draftName}
                        maxLength={80}
                        onValueChange={(value) => setDraftName(value)}
                        onKeyDown={(event) => {
                          if (event.key === "Enter") {
                            event.preventDefault()
                            void handleRename(session.family_id)
                          } else if (event.key === "Escape") {
                            event.preventDefault()
                            setEditingId(null)
                          }
                        }}
                        disabled={renameSession.isPending}
                        autoFocus
                      />
                      <Button
                        onClick={() => void handleRename(session.family_id)}
                        disabled={!draftName.trim() || renameSession.isPending}
                      >
                        {t("settings.saveDeviceName")}
                      </Button>
                      <Button
                        variant="ghost-btn"
                        className="device-row-action"
                        onClick={() => setEditingId(null)}
                      >
                        {t("settings.cancelDeviceName")}
                      </Button>
                    </>
                  ) : (
                    <>
                      <strong>{session.name}</strong>
                      {session.current && <em>{t("settings.deviceCurrent")}</em>}
                      <Button
                        variant="ghost-btn"
                        className="device-row-action"
                        onClick={() => {
                          setEditingId(session.family_id)
                          setDraftName(session.name)
                        }}
                      >
                        {t("settings.editDeviceName")}
                      </Button>
                    </>
                  )}
                </div>
                <div style={{ opacity: 0.8 }}>
                  IP: {session.ip || t("settings.unknownIp")} · {t("settings.deviceLastSeen")}{" "}
                  {formatTimestamp(session.last_seen_at)} · {t("settings.deviceCreated")}{" "}
                  {formatTimestamp(session.created_at)}
                </div>
                <div style={{ opacity: 0.65 }}>
                  {t("settings.sessionRefreshIdleWindow")}: {formatTimestamp(session.idle_expires_at)} ·{" "}
                  {t("settings.sessionRefreshAbsoluteWindow")}: {formatTimestamp(session.expires_at)}
                </div>
                <div>
                  <Button
                    variant="stdbtn"
                    className="stdbtn-danger"
                    onClick={() => void handleRevoke(session.family_id, session.current)}
                  >
                    {session.current ? t("settings.revokeCurrentDevice") : t("settings.revokeDevice")}
                  </Button>
                </div>
              </div>
            ))}
          </div>
        )}
        <br />
        {t("settings.activeDevicesHint")}
      </Row>
    </div>
  )
}
