/** Sending a note to the configured tracker (specs/018) — its own tab, since it is the one action
 * here a user takes without reading anything else first. */

import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { collectClientReportedInfo } from "@/api/clientReportedInfo"

import { submitFeedback } from "./api"

// As loose as the server's own check: one `@` with something on each side. Anything stricter
// rejects real addresses for no benefit.
const EMAIL = /^[^\s@]+@[^\s@]+\.[^\s@]+$/

// Mirrored from the server (`diagnostics::attachment`): the form can reject obvious mistakes
// before a 12 MB upload starts, but the server remains the authority.
const MAX_IMAGES = 3
const MAX_IMAGE_BYTES = 12 * 1024 * 1024

/** A picked file plus the object URL that previews it; the URL must be revoked by hand. */
interface PickedImage {
  file: File
  url: string
}

export function FeedbackView() {
  const { t } = useTranslation()
  const [name, setName] = useState("")
  const [email, setEmail] = useState("")
  const [message, setMessage] = useState("")
  const [sending, setSending] = useState(false)
  const [note, setNote] = useState<string | null>(null)
  const [nameTouched, setNameTouched] = useState(false)
  const [emailTouched, setEmailTouched] = useState(false)
  const [images, setImages] = useState<PickedImage[]>([])
  const [imageError, setImageError] = useState<string | null>(null)
  const fileInput = useRef<HTMLInputElement>(null)
  // Latest selection, for the unmount-time revocation below (an effect may not depend on every
  // change without re-running).
  const imagesRef = useRef<PickedImage[]>([])

  useEffect(() => {
    imagesRef.current = images
  }, [images])

  // Object URLs outlive the component unless revoked; drop any still live when the dialog closes.
  useEffect(() => {
    return () => {
      for (const image of imagesRef.current) URL.revokeObjectURL(image.url)
    }
  }, [])

  const addImages = (picked: FileList | null) => {
    if (!picked || picked.length === 0) return
    setImageError(null)
    const room = MAX_IMAGES - images.length
    const files = Array.from(picked)
    const tooLarge = files.find((file) => file.size > MAX_IMAGE_BYTES)
    if (tooLarge) {
      setImageError(
        t("diagnostics.feedback.image_too_large", {
          name: tooLarge.name,
          size: MAX_IMAGE_BYTES / 1024 / 1024,
        }),
      )
      return
    }
    if (files.length > room) {
      setImageError(t("diagnostics.feedback.image_limit", { count: MAX_IMAGES }))
    }
    const kept = files
      .slice(0, room)
      .map((file) => ({ file, url: URL.createObjectURL(file) }))
    setImages((current) => [...current, ...kept])
    // Let the same file be picked again after a removal.
    if (fileInput.current) fileInput.current.value = ""
  }

  const removeImage = (index: number) => {
    setImageError(null)
    const removed = images[index]
    if (removed) URL.revokeObjectURL(removed.url)
    setImages((current) => current.filter((_, at) => at !== index))
  }

  const clearImages = () => {
    for (const image of images) URL.revokeObjectURL(image.url)
    setImages([])
  }

  const trimmedName = name.trim()
  const trimmedEmail = email.trim()
  const emailValid = EMAIL.test(trimmedEmail)
  const ready = trimmedName.length > 0 && emailValid && message.trim().length > 0

  const nameError =
    nameTouched && trimmedName.length === 0 ? t("diagnostics.feedback.name_required") : null
  const emailError =
    !emailTouched || emailValid
      ? null
      : trimmedEmail.length === 0
        ? t("diagnostics.feedback.email_required")
        : t("diagnostics.feedback.email_invalid")

  /// The server owns the limits: per-account daily allowance, burst window and duplicate check.
  const send = async () => {
    setNameTouched(true)
    setEmailTouched(true)
    if (!ready) return
    setSending(true)
    setNote(null)
    try {
      // Only the browser can report these: screen, locale, time zone, client hints. The server
      // adds the User-Agent's OS/browser plus its own version, deploy time and uptime.
      const client = await collectClientReportedInfo()
      // The server attaches the newest event the tracker actually accepted, if any — a local id is
      // only meaningful there once it was forwarded.
      const result = await submitFeedback({
        message,
        name: trimmedName,
        email: trimmedEmail,
        client,
        url: window.location.pathname,
        images: images.map((image) => image.file),
      })
      setMessage("")
      clearImages()
      setNote(t("diagnostics.feedback.sent", { remaining: result.remaining_today }))
    } catch (error) {
      setNote(String(error))
    } finally {
      setSending(false)
    }
  }

  return (
    <div className="diag-group">
      <p className="diag-tagline">{t("diagnostics.feedback.hint")}</p>
      <div className="diag-grid">
        <label className="diag-field">
          <span>{t("diagnostics.feedback.name")}</span>
          <input
            className="stdinput"
            value={name}
            maxLength={128}
            autoComplete="name"
            aria-invalid={nameError !== null}
            aria-describedby={nameError ? "feedback-name-error" : undefined}
            onChange={(event) => setName(event.target.value)}
            onBlur={() => setNameTouched(true)}
          />
        </label>
        <label className="diag-field">
          <span>{t("diagnostics.feedback.email")}</span>
          <input
            className="stdinput"
            type="email"
            value={email}
            maxLength={254}
            autoComplete="email"
            aria-invalid={emailError !== null}
            aria-describedby={emailError ? "feedback-email-error" : undefined}
            onChange={(event) => setEmail(event.target.value)}
            onBlur={() => setEmailTouched(true)}
          />
        </label>
      </div>
      {nameError && (
        <p className="diag-field-error" id="feedback-name-error" role="alert">
          {nameError}
        </p>
      )}
      {emailError && (
        <p className="diag-field-error" id="feedback-email-error" role="alert">
          {emailError}
        </p>
      )}
      <p className="diag-usage">{t("diagnostics.feedback.attached")}</p>
      <textarea
        className="stdinput diag-message"
        rows={6}
        maxLength={2000}
        placeholder={t("diagnostics.feedback.placeholder")}
        value={message}
        onChange={(event) => setMessage(event.target.value)}
      />
      <div className="diag-field diag-files">
        <span>{t("diagnostics.feedback.images", { count: MAX_IMAGES })}</span>
        <button
          type="button"
          className="stdbtn"
          disabled={sending || images.length >= MAX_IMAGES}
          onClick={() => fileInput.current?.click()}
        >
          {t("diagnostics.feedback.add_image")}
        </button>
        <input
          ref={fileInput}
          className="diag-file-input"
          type="file"
          accept="image/*"
          multiple
          tabIndex={-1}
          onChange={(event) => addImages(event.target.files)}
        />
      </div>
      {images.length > 0 && (
        <ul className="diag-attachments">
          {images.map((image, index) => (
            <li
              key={`${image.file.name}-${image.file.lastModified}-${index}`}
              className="diag-attachment"
            >
              <img src={image.url} alt={image.file.name} />
              <div className="diag-attachment-foot">
                <span className="diag-attachment-name" title={image.file.name}>
                  {image.file.name}
                </span>
                <button
                  type="button"
                  className="stdbtn diag-attachment-remove"
                  aria-label={t("diagnostics.feedback.remove_image")}
                  title={t("diagnostics.feedback.remove_image")}
                  onClick={() => removeImage(index)}
                >
                  ×
                </button>
              </div>
            </li>
          ))}
        </ul>
      )}
      {imageError && (
        <p className="diag-field-error" role="alert">
          {imageError}
        </p>
      )}
      <div className="diag-inline">
        <button type="button" className="stdbtn" disabled={sending || !ready} onClick={send}>
          {t("diagnostics.feedback.send")}
        </button>
        <span className="diag-usage">{t("diagnostics.feedback.limit")}</span>
      </div>
      {note && <p className="diag-note">{note}</p>}
    </div>
  )
}
