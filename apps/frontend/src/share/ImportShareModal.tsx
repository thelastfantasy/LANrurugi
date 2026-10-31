import "./share.css"

import { useMutation } from "@tanstack/react-query"
import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { ApiError, sendJson } from "@/api/client"
import { useCategories } from "@/api/hooks"
import { Modal } from "@/components/common-ui/Display"
import { Checkbox, CheckboxField, Input, Select } from "@/components/common-ui/Form"
import { formatBytes } from "@/components/Display/JobProgress"
import { sortCategories } from "@/lib/utils/sortCategories"
import { Z_OVERLAY_ABOVE_LEGACY_MODAL } from "@/theme"
import { toast } from "@/toast"

import { formatShareCode, parseShareRef, type ShareRef, toTextForm } from "./code"
import { detectQr } from "./qr"
import type { ArchiveEntry, FilenamePolicy, ImportPreview } from "./types"

/** Live scanning needs a secure context; plain-HTTP home instances fall back to a photo. */
export function liveScanAvailable(): boolean {
  return window.isSecureContext && Boolean(navigator.mediaDevices?.getUserMedia)
}

/** Every distinct share reference among the QR payloads, in order; anything else is ignored, never
 * opened (FR-017). A bare code counts only when no payload is an explicit `lrrg:` form or link,
 * since any 8–10 character alphanumeric QR (a product number) also reads as a short code. The same
 * code seen twice is kept once, with its route hint if either copy had one. */
export function shareRefsFromPayloads(payloads: string[]): ShareRef[] {
  const explicit = new Map<string, ShareRef>()
  const bare = new Map<string, ShareRef>()
  for (const payload of payloads) {
    const ref = parseShareRef(payload)
    if (!ref) continue
    const text = payload.trim()
    const target = /^lrrg:/i.test(text) || text.includes("://") ? explicit : bare
    const seen = target.get(ref.code)
    if (!seen || (!seen.via && !seen.hint && (ref.via || ref.hint))) target.set(ref.code, ref)
  }
  return [...(explicit.size > 0 ? explicit : bare).values()]
}

function refusalText(t: (k: string, o?: Record<string, unknown>) => string, e: unknown): string {
  // A refusal is its stable code; some carry a technical detail after a colon (a version mismatch
  // names the other side's protocol versions), which the localized text shows as `{{detail}}`.
  const [code, detail] = (e instanceof ApiError ? e.message : "internal").split(/:\s*/, 2)
  return t(`share.refusal.${code}`, { defaultValue: t("share.import.failed"), detail })
}

/** Live camera scan; the stream is released on a hit, on close and on unmount (FR-017). */
function CameraScanner({ onFound, onClose }: { onFound: (refs: ShareRef[]) => void; onClose: () => void }) {
  const { t } = useTranslation()
  const video = useRef<HTMLVideoElement>(null)
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    let stream: MediaStream | null = null
    let frame = 0
    let stopped = false
    let busy = false
    const stop = () => {
      stopped = true
      cancelAnimationFrame(frame)
      stream?.getTracks().forEach((track) => track.stop())
    }
    const tick = () => {
      if (stopped) return
      const el = video.current
      if (el && el.readyState >= 2 && !busy) {
        busy = true
        detectQr(el)
          .then((payloads) => {
            const refs = shareRefsFromPayloads(payloads)
            if (refs.length > 0 && !stopped) {
              stop()
              onFound(refs)
            }
          })
          .catch(() => {})
          .finally(() => {
            busy = false
          })
      }
      frame = requestAnimationFrame(tick)
    }
    navigator.mediaDevices
      .getUserMedia({ video: { facingMode: "environment" } })
      .then((s) => {
        if (stopped) {
          s.getTracks().forEach((track) => track.stop())
          return
        }
        stream = s
        if (video.current) {
          video.current.srcObject = s
          void video.current.play()
        }
        tick()
      })
      .catch(() => setFailed(true))
    return stop
  }, [onFound])

  return (
    <div className="share-camera">
      {failed ? <p>{t("share.import.cameraDenied")}</p> : <video ref={video} className="share-camera-video" muted playsInline />}
      <button type="button" className="stdbtn" onClick={onClose}>
        {t("share.action.close")}
      </button>
    </div>
  )
}

function PreviewRow({
  entry,
  previewId,
  present,
  selected,
  onToggle,
}: {
  entry: ArchiveEntry
  previewId: string
  present: boolean
  selected: boolean
  onToggle: (on: boolean) => void
}) {
  const { t } = useTranslation()
  const [failed, setFailed] = useState(false)
  return (
    <tr className="share-preview-row" onClick={() => onToggle(!selected)}>
      <td className="share-preview-check" onClick={(e) => e.stopPropagation()}>
        <Checkbox checked={selected} onCheckedChange={onToggle} aria-label={entry.title} />
      </td>
      <td className="share-preview-cover">
        <img
          src={
            failed
              ? "/legacy/img/noThumb.png"
              : `/api/shares/imports/preview/${encodeURIComponent(previewId)}/thumbnail/${encodeURIComponent(entry.archive_id)}`
          }
          alt=""
          loading="lazy"
          onError={() => setFailed(true)}
        />
      </td>
      <td className="share-preview-title" title={entry.title}>
        {entry.title}
        {present && <span className="share-badge share-status-active">{t("share.import.alreadyInLibrary")}</span>}
      </td>
      <td className="share-preview-size">{formatBytes(entry.size)}</td>
    </tr>
  )
}

/** Upload page → Import share (017 US4, FR-016–FR-018). */
export function ImportShareModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()
  const categories = useCategories()
  const [code, setCode] = useState("")
  const [scanning, setScanning] = useState(false)
  /** Several share codes in one image: the user picks, since which one is still valid only the
   * sharer can say. */
  const [candidates, setCandidates] = useState<ShareRef[]>([])
  const [category, setCategory] = useState("")
  const [helpDeliver, setHelpDeliver] = useState(false)
  const [inheritCategories, setInheritCategories] = useState(false)
  // Only meaningful when the preview reports name conflicts; rename is the never-blocking default.
  const [filenamePolicy, setFilenamePolicy] = useState<FilenamePolicy>("rename")
  /** A view-only ordering; the import itself follows the sharer's manifest order. */
  const [sort, setSort] = useState<{ key: "title" | "size"; desc: boolean } | null>(null)
  /** Archives left out of the import; empty means the whole share, the default. */
  const [deselected, setDeselected] = useState<Set<string>>(() => new Set())
  const photoInput = useRef<HTMLInputElement>(null)
  const fileInput = useRef<HTMLInputElement>(null)
  const canScan = liveScanAvailable()

  const resolve = useMutation({
    mutationFn: (c: string) => sendJson<ImportPreview>("POST", "/shares/imports/resolve", { code: c }),
    onSuccess: () => setDeselected(new Set()),
  })
  const confirm = useMutation({
    mutationFn: (body: {
      code: string
      preview_id: string
      category?: string
      help_deliver?: boolean
      inherit_categories?: boolean
      filename_policy?: FilenamePolicy
      archives?: string[]
    }) =>
      sendJson<{ import_id: string }>("POST", "/shares/imports", body),
    onSuccess: () => {
      toast({ heading: t("share.import.queued"), icon: "success", hideAfter: 4000 })
      onClose()
    },
  })

  const applyFound = (refs: ShareRef[]) => {
    setCandidates(refs.length > 1 ? refs : [])
    if (refs.length > 0) {
      setCode(toTextForm(refs[0]))
      resolve.reset()
    }
  }

  const decodeImage = async (blob: Blob) => {
    const warn = (key: string) => toast({ heading: t(key), icon: "warning", hideAfter: 4000 })
    let payloads: string[]
    try {
      payloads = await detectQr(blob)
    } catch {
      warn("share.import.noQrInImage")
      return
    }
    const refs = shareRefsFromPayloads(payloads)
    if (payloads.length === 0) warn("share.import.noQrInImage")
    else if (refs.length === 0) warn("share.import.noCodeInImage")
    applyFound(refs)
  }

  const onPaste = (e: React.ClipboardEvent) => {
    const image = [...e.clipboardData.items].find((item) => item.type.startsWith("image/"))?.getAsFile()
    if (image) {
      e.preventDefault()
      void decodeImage(image)
    }
  }

  const parsed = parseShareRef(code)
  const preview = resolve.data
  const items = preview ? [...new Map(preview.manifest.items.flatMap((item) =>
    "Archive" in item ? [item.Archive] : "Tankoubon" in item ? item.Tankoubon.members : []).map((a) => [a.archive_id, a])).values()] : []
  // The submitted list is `items` order (the sharer's manifest order), never the view's sort order.
  const chosen = items.filter((a) => !deselected.has(a.archive_id))
  const chosenSize = chosen.reduce((sum, a) => sum + a.size, 0)
  // A copy sorted for display only: `chosen` above stays in manifest order, and that is what the
  // create request sends.
  const shown = sort
    ? [...items].sort((a, b) =>
        sort.key === "title"
          ? a.title.localeCompare(b.title) * (sort.desc ? -1 : 1)
          : (a.size - b.size) * (sort.desc ? -1 : 1),
      )
    : items
  const toggleSort = (key: "title" | "size") =>
    setSort((s) => (s?.key === key ? (s.desc ? null : { key, desc: true }) : { key, desc: key === "size" }))
  const toggle = (id: string, on: boolean) =>
    setDeselected((s) => {
      const next = new Set(s)
      if (on) next.delete(id)
      else next.add(id)
      return next
    })

  return (
    <Modal onClose={onClose} width={640} textAlign="left">
      <div onPaste={onPaste}>
        <h2 className="ih">{t("share.import.heading")}</h2>
        {!preview && (
          <>
            <p>{t("share.import.intro")}</p>
            <Input
              value={code}
              onValueChange={setCode}
              placeholder="lrrg:XXXXX-XXXXX"
              autoFocus
            />
            <div className="share-card-actions" style={{ justifyContent: "flex-start", marginTop: 8 }}>
              <button type="button" className="stdbtn" onClick={() => fileInput.current?.click()}>
                {t("share.import.chooseImage")}
              </button>
              {canScan ? (
                <button type="button" className="stdbtn" onClick={() => setScanning(true)}>
                  {t("share.import.scan")}
                </button>
              ) : (
                <button type="button" className="stdbtn" onClick={() => photoInput.current?.click()}>
                  {t("share.import.takePhoto")}
                </button>
              )}
            </div>
            {candidates.length > 1 && (
              <div className="share-candidates">
                <p className="share-result-note">{t("share.import.severalCodes", { count: candidates.length })}</p>
                <ul>
                  {candidates.map((ref) => {
                    const text = toTextForm(ref)
                    return (
                      <li key={ref.code}>
                        <button
                          type="button"
                          className="stdbtn"
                          aria-pressed={parsed?.code === ref.code}
                          title={text}
                          onClick={() => {
                            setCode(text)
                            resolve.reset()
                          }}
                        >
                          {parsed?.code === ref.code && <i className="fa fa-check" aria-hidden="true"></i>} {formatShareCode(ref.code)}
                          {(ref.via || ref.hint) && <span className="share-candidate-via"> · {t("share.import.carriesRoute")}</span>}
                        </button>
                      </li>
                    )
                  })}
                </ul>
              </div>
            )}
            {!canScan && <p className="share-result-note">{t("share.import.noLiveScan")}</p>}
            <p className="share-result-note">{t("share.import.pasteHint")}</p>
            <p className="share-disclaimer">{t("share.disclaimer.import")}</p>
            <input
              ref={fileInput}
              type="file"
              accept="image/*"
              hidden
              onChange={(e) => {
                const file = e.target.files?.[0]
                if (file) void decodeImage(file)
                e.target.value = ""
              }}
            />
            <input
              ref={photoInput}
              type="file"
              accept="image/*"
              capture="environment"
              hidden
              onChange={(e) => {
                const file = e.target.files?.[0]
                if (file) void decodeImage(file)
                e.target.value = ""
              }}
            />
            {scanning && (
              <CameraScanner
                onFound={(refs) => {
                  applyFound(refs)
                  setScanning(false)
                }}
                onClose={() => setScanning(false)}
              />
            )}
            {resolve.isError && <p className="share-notice share-status-exhausted">{refusalText(t, resolve.error)}</p>}
            <div style={{ textAlign: "right", marginTop: 8 }}>
              <button
                type="button"
                className="stdbtn"
                disabled={!parsed || resolve.isPending}
                onClick={() => parsed && resolve.mutate(toTextForm(parsed))}
              >
                {resolve.isPending ? t("share.import.looking") : t("share.import.next")}
              </button>
            </div>
          </>
        )}
        {preview && parsed && (
          <>
            <div className="share-preview-header">
              <span>
                <strong>{preview.manifest.title}</strong> ·{" "}
                {chosen.length === items.length
                  ? t("share.import.summary", { count: preview.archive_count, size: formatBytes(preview.total_size) })
                  : t("share.import.selectedSummary", {
                      count: chosen.length,
                      total: items.length,
                      size: formatBytes(chosenSize),
                    })}
              </span>
              {items.length > 1 && (
                <CheckboxField
                  checked={chosen.length === items.length}
                  onCheckedChange={(on) => setDeselected(on ? new Set() : new Set(items.map((a) => a.archive_id)))}
                >
                  {t("upload.selectAll")}
                </CheckboxField>
              )}
            </div>
            <div className="share-preview-scroll">
              <table className="share-preview-table">
                <thead>
                  <tr>
                    <th className="share-preview-check" aria-hidden="true" />
                    <th aria-hidden="true" />
                    <th aria-sort={sort?.key === "title" ? (sort.desc ? "descending" : "ascending") : "none"}>
                      <button
                        type="button"
                        className="share-sort"
                        onClick={() => toggleSort("title")}
                        title={t("share.import.sortByTitle")}
                      >
                        {t("share.import.columnTitle")}
                        {sort?.key === "title" && <i className={`fa ${sort.desc ? "fa-sort-down" : "fa-sort-up"}`} />}
                      </button>
                    </th>
                    <th
                      className="share-preview-size"
                      aria-sort={sort?.key === "size" ? (sort.desc ? "descending" : "ascending") : "none"}
                    >
                      <button
                        type="button"
                        className="share-sort"
                        onClick={() => toggleSort("size")}
                        title={t("share.import.sortBySize")}
                      >
                        {t("share.import.columnSize")}
                        {sort?.key === "size" && <i className={`fa ${sort.desc ? "fa-sort-down" : "fa-sort-up"}`} />}
                      </button>
                    </th>
                  </tr>
                </thead>
                <tbody>
                  {shown.map((entry) => (
                    <PreviewRow
                      key={entry.archive_id}
                      entry={entry}
                      previewId={preview.preview_id}
                      present={preview.already_present.includes(entry.archive_id)}
                      selected={!deselected.has(entry.archive_id)}
                      onToggle={(on) => toggle(entry.archive_id, on)}
                    />
                  ))}
                </tbody>
              </table>
            </div>
            <div className="share-form">
              <span>{t("upload.addUploadedFilesToCategory")}</span>
              <Select
                popupZIndex={Z_OVERLAY_ABOVE_LEGACY_MODAL}
                value={category}
                onValueChange={setCategory}
                align="start"
                items={[
                  { value: "", label: t("common.NoCategory") },
                  ...sortCategories(categories.data ?? []).filter((c) => !c.search).map((c) => ({ value: c.id, label: c.name })),
                ]}
              />
            </div>
            <div className="share-form">
              <CheckboxField checked={inheritCategories} onCheckedChange={setInheritCategories}>
                {t("share.import.inheritCategories")}
              </CheckboxField>
              <span className="share-form-hint">{t("share.import.inheritCategoriesHint")}</span>
            </div>
            <div className="share-form">
              <span>{t("share.import.filenamePolicy")}</span>
              <Select
                popupZIndex={Z_OVERLAY_ABOVE_LEGACY_MODAL}
                value={filenamePolicy}
                onValueChange={(v) => setFilenamePolicy(v as FilenamePolicy)}
                align="start"
                items={[
                  { value: "rename", label: t("share.import.filenamePolicyRename") },
                  { value: "skip", label: t("share.import.filenamePolicySkip") },
                  { value: "overwrite", label: t("share.import.filenamePolicyOverwrite") },
                ]}
              />
              <span className="share-form-hint">
                {preview.filename_conflicts && preview.filename_conflicts.length > 0
                  ? t("share.import.filenameConflictCount", { count: preview.filename_conflicts.length })
                  : t(`share.import.filenamePolicyHint.${filenamePolicy}`)}
              </span>
            </div>
            {preview.allow_helpers === true && (
              <div className="share-form">
                <CheckboxField checked={helpDeliver} onCheckedChange={setHelpDeliver}>
                  {t("share.import.helpDeliver")}
                </CheckboxField>
                <span className="share-form-hint">{t("share.import.helpDeliverHint")}</span>
              </div>
            )}
            <p className="share-disclaimer">{t("share.disclaimer.import")}</p>
            {confirm.isError && <p className="share-notice share-status-exhausted">{refusalText(t, confirm.error)}</p>}
            <div style={{ textAlign: "right", marginTop: 8, display: "flex", gap: 8, justifyContent: "flex-end" }}>
              <button type="button" className="stdbtn" onClick={() => resolve.reset()}>
                {t("share.import.back")}
              </button>
              <button
                type="button"
                className="stdbtn"
                disabled={confirm.isPending || !preview.permissions.import || chosen.length === 0}
                onClick={() =>
                  confirm.mutate({
                    code: toTextForm(parsed),
                    preview_id: preview.preview_id,
                    category: category || undefined,
                    // Only ever asked for when the sharer allows helpers (the box is hidden otherwise).
                    help_deliver: preview.allow_helpers === true && helpDeliver,
                    inherit_categories: inheritCategories,
                    // What to do when a local archive already owns an imported filename.
                    filename_policy: filenamePolicy,
                    // Omitted when everything is chosen, so the whole share is imported as before.
                    archives: chosen.length === items.length ? undefined : chosen.map((a) => a.archive_id),
                  })
                }
              >
                {t("share.import.confirm")}
              </button>
            </div>
            {!preview.permissions.import && <p className="share-result-note">{t("share.refusal.permission_denied")}</p>}
          </>
        )}
      </div>
    </Modal>
  )
}
