import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { ApiError } from "@/api/client"
import { useSetSafetyPassword } from "@/api/hooks"
import type { SafetyPasswordStatus } from "@/api/types"
import { Modal } from "@/components/common-ui/Display/Modal"
import { Button } from "@/components/common-ui/Form"
import { OTPField } from "@/components/common-ui/Form/OTPField"

/** Shared dialog for every irreversible recycle-bin action (spec 019). It owns the four-digit
 * entry, the first-use setup flow, the lockout countdown and the error copy; the caller only
 * supplies the action it should run once a password is accepted. */
export function SafetyPasswordDialog({
  title,
  description,
  confirmLabel,
  configured,
  status,
  onClose,
  onConfirm,
}: {
  title: string
  description?: string
  confirmLabel: string
  configured: boolean
  status: SafetyPasswordStatus | undefined
  onClose: () => void
  /** May throw; an `ApiError` body is interpreted for the lockout/incorrect cases. */
  onConfirm: (pin: string) => Promise<void>
}) {
  const { t } = useTranslation()
  const setSafetyPassword = useSetSafetyPassword()
  const [pin, setPin] = useState("")
  const [newPin, setNewPin] = useState("")
  const [confirmPin, setConfirmPin] = useState("")
  const [error, setError] = useState<string | null>(null)
  const [submitting, setSubmitting] = useState(false)
  const [lockedUntil, setLockedUntil] = useState<number | null>(status?.locked_until ?? null)
  // Bumped on every failure so the digit field takes focus back (the confirm button keeps it
  // otherwise, leaving the user to click a slot before retrying).
  const [focusSignal, setFocusSignal] = useState(0)
  const [now, setNow] = useState(() => Date.now())
  const confirmRef = useRef<HTMLButtonElement | null>(null)

  useEffect(() => {
    if (!lockedUntil) return
    const id = window.setInterval(() => setNow(Date.now()), 1000)
    return () => window.clearInterval(id)
  }, [lockedUntil])

  useEffect(() => {
    function onKey(event: KeyboardEvent) {
      if (event.key === "Escape" && !submitting) onClose()
    }
    document.addEventListener("keydown", onKey)
    return () => document.removeEventListener("keydown", onKey)
  }, [onClose, submitting])

  const lockedRemaining =
    lockedUntil && lockedUntil * 1000 > now ? Math.ceil((lockedUntil * 1000 - now) / 1000) : 0

  async function handleError(err: unknown) {
    if (err instanceof ApiError) {
      const body = err.body as { error?: string; locked_until?: number } | undefined
      if (body?.error === "safety_password_locked" && body.locked_until) {
        setLockedUntil(body.locked_until)
        setError(t("safetyPassword.locked"))
        return
      }
      if (body?.error === "safety_password_incorrect") {
        setPin("")
        setError(t("safetyPassword.incorrect"))
        setFocusSignal((n) => n + 1)
        return
      }
      if (body?.error === "invalid_safety_password_format") {
        setError(t("safetyPassword.fourDigits"))
        setFocusSignal((n) => n + 1)
        return
      }
    }
    setError(t("safetyPassword.actionFailed"))
  }

  async function submit() {
    setError(null)
    if (!configured) {
      if (newPin.length !== 4) {
        setError(t("safetyPassword.fourDigits"))
        return
      }
      if (newPin !== confirmPin) {
        setError(t("safetyPassword.mismatch"))
        return
      }
      setSubmitting(true)
      try {
        await setSafetyPassword.mutateAsync({ new_password: newPin })
        await onConfirm(newPin)
      } catch (err) {
        await handleError(err)
      } finally {
        setSubmitting(false)
      }
      return
    }
    if (pin.length !== 4) {
      setError(t("safetyPassword.fourDigits"))
      return
    }
    setSubmitting(true)
    try {
      await onConfirm(pin)
    } catch (err) {
      await handleError(err)
    } finally {
      setSubmitting(false)
    }
  }

  const disabled = submitting || lockedRemaining > 0

  return (
    <Modal onClose={onClose} width={460}>
      <h2 className="ih" style={{ margin: "0 0 8px" }}>
        {title}
      </h2>
      {description && <p style={{ margin: "0 0 12px" }}>{description}</p>}

      {!configured ? (
        <div className="otp-field-stack">
          <OTPField
            value={newPin}
            onValueChange={(value) => {
              setNewPin(value)
              setError(null)
            }}
            onComplete={() => confirmPin.length === 4 && confirmRef.current?.focus()}
            label={t("safetyPassword.newPassword")}
            slotLabel={(index, total) => t("safetyPassword.slot", { index: index + 1, total })}
            autoFocus
            focusSignal={focusSignal}
          />
          <OTPField
            value={confirmPin}
            onValueChange={(value) => {
              setConfirmPin(value)
              setError(null)
            }}
            onComplete={() => confirmRef.current?.focus()}
            label={t("safetyPassword.confirmPassword")}
            slotLabel={(index, total) => t("safetyPassword.slot", { index: index + 1, total })}
          />
        </div>
      ) : (
        <OTPField
          value={pin}
          onValueChange={(value) => {
            setPin(value)
            setError(null)
          }}
          onComplete={() => confirmRef.current?.focus()}
          label={t("safetyPassword.safetyPassword")}
          slotLabel={(index, total) => t("safetyPassword.slot", { index: index + 1, total })}
          invalid={Boolean(error)}
          autoFocus
          disabled={disabled}
          focusSignal={focusSignal}
        />
      )}

      {lockedRemaining > 0 ? (
        <p className="otp-field-error" role="alert">
          {t("safetyPassword.tryAgainIn", { seconds: lockedRemaining })}
        </p>
      ) : (
        error && (
          <p className="otp-field-error" role="alert">
            {error}
          </p>
        )
      )}

      <div style={{ display: "flex", justifyContent: "center", gap: 10, marginTop: 18 }}>
        <Button
          ref={confirmRef}
          className="recycle-btn"
          onClick={() => void submit()}
          disabled={disabled}
        >
          {submitLabel(configured, confirmLabel, t)}
        </Button>
        <Button className="recycle-btn" onClick={onClose} disabled={submitting}>
          {t("common.cancel")}
        </Button>
      </div>
    </Modal>
  )
}

function submitLabel(
  configured: boolean,
  confirmLabel: string,
  t: (key: string, options?: Record<string, unknown>) => string,
): string {
  if (!configured) return t("safetyPassword.setAndContinue")
  return confirmLabel
}
