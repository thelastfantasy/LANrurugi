import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import { ApiError } from "@/api/client"
import {
  useRecycleBin,
  useResetSafetyPassword,
  useSafetyPasswordStatus,
  useSetSafetyPassword,
} from "@/api/hooks"
import { Button, Input, NumberInput } from "@/components/common-ui/Form"
import { OTPField } from "@/components/common-ui/Form/OTPField"
import { CollapsibleSection } from "@/components/Display"
import { useIsNarrowViewport } from "@/hooks"
import { formatBytes } from "@/lib/formatBytes"
import { toast } from "@/toast"

import { Row } from "./shared"

/** Settings → Recycle bin: the two bounds on the bin plus the safety-password credential. The
 * numeric fields live in `SettingsPage`'s own save payload; everything else here is immediate. */
export function RecycleBinSection({
  retentionDays,
  setRetentionDays,
  maxSizeGb,
  setMaxSizeGb,
}: {
  retentionDays: number
  setRetentionDays: (value: number) => void
  maxSizeGb: number
  setMaxSizeGb: (value: number) => void
}) {
  const { t } = useTranslation()
  const narrow = useIsNarrowViewport()
  // Phones: every control in this section is centred under its label. Wider screens keep the
  // settings convention (labels left, controls starting at the value column's left edge).
  const stackAlign = narrow ? "center" : "flex-start"
  const textAlign = narrow ? "center" : "left"
  const bin = useRecycleBin()
  const status = useSafetyPasswordStatus()
  const setPassword = useSetSafetyPassword()
  const resetPassword = useResetSafetyPassword()

  const [pin, setPin] = useState("")
  const [newPin, setNewPin] = useState("")
  const [confirmPin, setConfirmPin] = useState("")
  const [loginPassword, setLoginPassword] = useState("")
  const [error, setError] = useState<string | null>(null)
  const [focusSignal, setFocusSignal] = useState(0)
  const [savedFlash, setSavedFlash] = useState(false)

  useEffect(() => {
    if (!savedFlash) return
    const id = window.setTimeout(() => setSavedFlash(false), 2000)
    return () => window.clearTimeout(id)
  }, [savedFlash])

  const configured = status.data?.configured ?? false

  function slotLabel(index: number, total: number) {
    return t("safetyPassword.slot", { index: index + 1, total })
  }

  async function applyPassword() {
    setError(null)
    if (newPin.length !== 4) {
      setError(t("safetyPassword.fourDigits"))
      return
    }
    if (newPin !== confirmPin) {
      setError(t("safetyPassword.mismatch"))
      return
    }
    try {
      await setPassword.mutateAsync({
        new_password: newPin,
        current_password: configured ? pin : undefined,
      })
      setPin("")
      setNewPin("")
      setConfirmPin("")
      toast({ heading: t("safetyPassword.saved"), icon: "success" })
    } catch (err) {
      setError(passwordError(err, t))
      setFocusSignal((n) => n + 1)
    }
  }

  async function applyReset() {
    setError(null)
    try {
      await resetPassword.mutateAsync(loginPassword)
      setLoginPassword("")
      setPin("")
      toast({ heading: t("safetyPassword.resetDone"), icon: "success" })
    } catch (err) {
      setError(passwordError(err, t))
    }
  }

  const usage = bin.data?.usage
  const leftovers = bin.data?.leftovers.count ?? 0

  return (
    <CollapsibleSection id="recycle" icon="fa-recycle" title={t("settings.recycleBin")}>
      <div className="settings-table">
        <Row label={t("settings.recycleRetentionDays")}>
          <div style={{ display: "flex", justifyContent: stackAlign }}>
            <NumberInput
              min={0}
              value={retentionDays}
              onValueChange={(value) => {
                setRetentionDays(Math.max(0, Math.floor(value)))
                setSavedFlash(true)
              }}
            />
          </div>
          <p style={{ textAlign }}>{t("settings.recycleRetentionHelp")}</p>
        </Row>
        <Row label={t("settings.recycleMaxSize")}>
          <div style={{ display: "flex", justifyContent: stackAlign }}>
            <NumberInput
              min={0}
              value={maxSizeGb}
              onValueChange={(value) => {
                setMaxSizeGb(Math.max(0, Math.floor(value)))
                setSavedFlash(true)
              }}
            />
          </div>
          <p style={{ textAlign }}>{t("settings.recycleMaxSizeHelp")}</p>
        </Row>
        <Row label={t("settings.recycleUsage")} noHelp>
          <p style={{ textAlign }}>
            {usage
              ? t("recycleBin.usageInline", {
                  count: usage.count,
                  bytes: formatBytes(usage.bytes),
                })
              : t("common.loadingLibrary")}
            {savedFlash && ` · ${t("settings.saveToApply")}`}
          </p>
        </Row>
        <Row label={t("safetyPassword.title")}>
          <p style={{ textAlign }}>
            {configured ? t("safetyPassword.configuredHelp") : t("safetyPassword.notConfiguredHelp")}
          </p>
          {configured && (
            <div className="otp-field-stack" style={{ alignItems: stackAlign, marginBottom: 10 }}>
              <OTPField
                value={pin}
                onValueChange={setPin}
                label={t("safetyPassword.currentPassword")}
                slotLabel={slotLabel}
                focusSignal={focusSignal}
                align={narrow ? "center" : "start"}
              />
            </div>
          )}
          <div className="otp-field-stack" style={{ alignItems: stackAlign }}>
            <OTPField
              value={newPin}
              onValueChange={setNewPin}
              label={configured ? t("safetyPassword.newPassword") : t("safetyPassword.setPassword")}
              slotLabel={slotLabel}
              focusSignal={focusSignal}
              align={narrow ? "center" : "start"}
            />
            <OTPField
              value={confirmPin}
              onValueChange={setConfirmPin}
              label={t("safetyPassword.confirmPassword")}
              slotLabel={slotLabel}
              align={narrow ? "center" : "start"}
            />
          </div>
          <div style={{ marginTop: 10, display: "flex", justifyContent: stackAlign }}>
            <Button
              className="recycle-btn"
              onClick={() => void applyPassword()}
              disabled={setPassword.isPending}
            >
              {configured ? t("safetyPassword.change") : t("safetyPassword.set")}
            </Button>
          </div>
          <hr style={{ margin: narrow ? "16px auto" : "16px 0", opacity: 0.25, width: narrow ? "70%" : undefined }} />
          <p style={{ textAlign }}>{t("safetyPassword.resetHelp")}</p>
          <div
            style={{
              display: "flex",
              gap: 8,
              alignItems: "center",
              justifyContent: stackAlign,
              flexWrap: "wrap",
              flexDirection: narrow ? "column" : "row",
            }}
          >
            <Input
              type="password"
              value={loginPassword}
              onValueChange={setLoginPassword}
              placeholder={t("safetyPassword.loginPassword")}
              aria-label={t("safetyPassword.loginPassword")}
              style={{ maxWidth: 260 }}
            />
            <Button
              className="recycle-btn"
              onClick={() => void applyReset()}
              disabled={resetPassword.isPending || loginPassword.length === 0}
            >
              {t("safetyPassword.reset")}
            </Button>
          </div>
          {error && <p className="otp-field-error">{error}</p>}
          {leftovers > 0 && (
            <p style={{ textAlign }}>{t("recycleBin.leftovers", { count: leftovers })}</p>
          )}
        </Row>
      </div>
    </CollapsibleSection>
  )
}

function passwordError(err: unknown, t: (key: string) => string): string {
  if (err instanceof ApiError) {
    const body = err.body as { error?: string } | undefined
    if (body?.error === "safety_password_locked") return t("safetyPassword.locked")
    if (body?.error === "safety_password_incorrect") return t("safetyPassword.incorrect")
    if (body?.error === "invalid_safety_password_format") return t("safetyPassword.fourDigits")
    if (body?.error === "wrong_password") return t("safetyPassword.wrongLoginPassword")
  }
  return t("safetyPassword.actionFailed")
}
