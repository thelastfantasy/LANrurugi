import { type CSSProperties, useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { useJudgePreview, usePreviewSubscription } from "@/api/hooks"
import type { SubscriptionBody, SubscriptionSource } from "@/api/types"
import { Modal } from "@/components/common-ui/Display"

import { SubscriptionForm } from "./SubscriptionForm"
import { previewGridColumns, SubscriptionPreview } from "./SubscriptionPreview"

/** Below this the two columns cannot both be read, so the preview moves to its own tab. */
const TWO_COLUMN_MIN = 1024

function useIsWide() {
  const [wide, setWide] = useState(
    () => typeof window !== "undefined" && window.innerWidth >= TWO_COLUMN_MIN,
  )
  useEffect(() => {
    const query = window.matchMedia(`(min-width: ${TWO_COLUMN_MIN}px)`)
    const update = (e: MediaQueryListEvent | MediaQueryList) => setWide(e.matches)
    query.addEventListener("change", update)
    return () => query.removeEventListener("change", update)
  }, [])
  return wide
}

/** Stand-in rows while the first listing is being fetched.
 *
 * Shaped like the real rows rather than a spinner, so the pane does not jump from one line of text to
 * a full table once the answer arrives. */
/** Height of one placeholder row, matching `.preview-skeleton-row`'s own padding and bar height. */
const SKELETON_ROW_HEIGHT = 31

export function PreviewSkeleton({ fields }: { fields?: string[] }) {
  const host = useRef<HTMLDivElement>(null)
  const includeField = (field: string) => fields === undefined || fields.includes(field)
  const columns = previewGridColumns(fields)
  // Start from enough rows to fill the preview pane at its tallest layout. Desktop pins the pane
  // at 65vh; narrow layouts give it `max-height: 70vh`. Using the larger value here means the
  // skeleton already fills either pane, and the ResizeObserver below fine-tunes it on desktop.
  const [rows, setRows] = useState(() =>
    typeof window === "undefined"
      ? 8
      : Math.max(8, Math.ceil((window.innerHeight * 0.7) / SKELETON_ROW_HEIGHT)),
  )

  useEffect(() => {
    const el = host.current
    if (!el) return
    const fit = () => {
      const available = el.parentElement?.clientHeight ?? el.clientHeight
      if (available > 0) {
        // Never shrink below the startup count: on a height-auto pane the first layout can report a
        // smaller value than the pane eventually gets, and reducing there left only a few rows.
        setRows((current) => Math.max(current, 3, Math.floor(available / SKELETON_ROW_HEIGHT)))
      }
    }
    fit()
    // The pane is sized from the viewport, so a resize changes how many fit.
    const observer = new ResizeObserver(fit)
    if (el.parentElement) observer.observe(el.parentElement)
    return () => observer.disconnect()
  }, [])

  return (
    <div className="preview-skeleton" ref={host} aria-hidden="true">
      {Array.from({ length: rows }, (_, i) => (
        <div
          key={i}
          className="preview-skeleton-row"
          style={{ "--preview-grid-columns": columns } as CSSProperties}
        >
          {/* `maxWidth: "100%"` is what keeps a bar inside its own column: the posted/uploader
              tracks are `minmax(0, 10%)`/`minmax(0, 12%)`, which on the modal's ~640px pane are
              narrower than the 5em (80px) a bar asked for — so the bar spilled over the next
              column instead of stopping at the track edge (reported live, 2026-10-09: columns 3
              and 4 appeared to overlap). Real rows never did this only because their text is
              truncated; a fixed-width placeholder needs the clamp. */}
          <span className="preview-skeleton-bar" style={{ width: "2em", maxWidth: "100%" }} />
          <span
            className="preview-skeleton-bar"
            style={{ width: `${55 + ((i * 7) % 35)}%`, maxWidth: "100%" }}
          />
          {includeField("posted_at") && (
            <span className="preview-skeleton-bar" style={{ width: "5em", maxWidth: "100%" }} />
          )}
          {includeField("uploader") && (
            <span className="preview-skeleton-bar" style={{ width: "5em", maxWidth: "100%" }} />
          )}
          {includeField("rating") && (
            <span className="preview-skeleton-bar" style={{ width: "4em", maxWidth: "100%" }} />
          )}
          <span className="preview-skeleton-bar" style={{ width: "6em", maxWidth: "100%" }} />
        </div>
      ))}
    </div>
  )
}

/** The subscription form, with its preview beside it.
 *
 * In a modal rather than inline because the preview lists a whole page of works — inline it pushed
 * everything below it far off screen, and the rules it was explaining scrolled out of view.
 */
export function SubscriptionFormModal({
  sources,
  initial,
  subscriptionId,
  onClose,
}: {
  sources: SubscriptionSource[]
  initial: SubscriptionBody
  subscriptionId?: string
  onClose: () => void
}) {
  const { t } = useTranslation()
  const wide = useIsWide()
  const preview = usePreviewSubscription()
  const judge = useJudgePreview()
  const [tab, setTab] = useState<"form" | "preview">("form")
  /** The draft as currently typed, kept here so the preview can run against it. */
  const [draft, setDraft] = useState<SubscriptionBody>(initial)

  // Two separate triggers, because only one of them is a network call.
  //
  // `criteria` and `source` are what the request to the source is built from, so changing them means
  // fetching again. Conditions are judged after the fact, so changing one re-judges the listing
  // already in hand — no request, and the rows do not blank out while a rule is being typed.
  const fetchKey = JSON.stringify({ source: draft.source, criteria: draft.criteria })
  const judgeKey = JSON.stringify({ filters: draft.filters })

  /** The listing the last fetch returned, replayed when only a condition changed. */
  const listing = useRef<unknown>(null)
  const latest = useRef(draft)
  latest.current = draft

  const fetchTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => {
    if (!draft.source) return
    if (fetchTimer.current) clearTimeout(fetchTimer.current)
    fetchTimer.current = setTimeout(() => {
      preview.mutate(
        { id: subscriptionId, draft: latest.current },
        { onSuccess: (data) => (listing.current = data.listing ?? null) },
      )
    }, 600)
    return () => {
      if (fetchTimer.current) clearTimeout(fetchTimer.current)
    }
    // Mutations are stable and the draft is covered by `fetchKey`.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [fetchKey, subscriptionId])

  const judgeTimer = useRef<ReturnType<typeof setTimeout> | null>(null)
  useEffect(() => {
    // Nothing fetched yet, so there is nothing to re-judge — the fetch above will cover it.
    if (!listing.current) return
    if (judgeTimer.current) clearTimeout(judgeTimer.current)
    judgeTimer.current = setTimeout(() => {
      judge.mutate({ subscription: latest.current, listing: listing.current })
    }, 300)
    return () => {
      if (judgeTimer.current) clearTimeout(judgeTimer.current)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [judgeKey])

  // The newest result from either path. A re-judge supersedes the fetch that produced it, and a fresh
  // fetch supersedes an older judgment.
  const shown = judge.data ?? preview.data

  // Which optional columns the source actually reports. Plugins without uploader/rating/posted_at
  // must not show empty columns (or skeleton bars for them).
  const selectedSource = sources.find((source) => source.namespace === draft.source)
  const candidateFields = selectedSource?.candidate_fields

  const previewPane = (
    <div className="sub-modal-preview">
      <h4 className="sub-form-section" style={{ marginTop: 0 }}>
        {t("subscriptions.previewPaneTitle")}
      </h4>
      {shown ? (
        // Kept on screen while a re-judge is in flight, so changing a rule updates the verdicts in
        // place instead of blanking the list and rebuilding it.
        <div style={{ opacity: judge.isPending ? 0.6 : 1, transition: "opacity 120ms" }}>
          <SubscriptionPreview preview={shown} subscriptionId={subscriptionId} fields={candidateFields} />
        </div>
      ) : preview.isPending ? (
        <PreviewSkeleton fields={candidateFields} />
      ) : preview.isError ? (
        <p style={{ color: "red" }}>{String(preview.error)}</p>
      ) : (
        <p className="sub-form-hint">{t("subscriptions.previewIdle")}</p>
      )}
    </div>
  )

  const formPane = (
    <SubscriptionForm
      sources={sources}
      initial={initial}
      subscriptionId={subscriptionId}
      onDraftChange={setDraft}
      onDone={onClose}
    />
  )

  return (
    <Modal onClose={onClose} width={wide ? 1180 : 620} textAlign="left">
      <h3 className="ih" style={{ fontSize: "1.1em", margin: "0 0 10px", textAlign: "center" }}>
        {subscriptionId ? t("subscriptions.edit") : t("subscriptions.addNew")}
      </h3>

      {wide ? (
        <div className="sub-modal-split">
          <div>{formPane}</div>
          {previewPane}
        </div>
      ) : (
        <>
          {/* Tabs rather than stacking: stacked, the preview would sit below a form tall enough that
              nobody would scroll to it while still editing the rules it explains. */}
          <div className="sub-modal-tabs" role="tablist">
            <button
              type="button"
              role="tab"
              aria-selected={tab === "form"}
              className={tab === "form" ? "stdbtn is-active" : "stdbtn"}
              onClick={() => setTab("form")}
            >
              {t("subscriptions.tabSettings")}
            </button>
            <button
              type="button"
              role="tab"
              aria-selected={tab === "preview"}
              className={tab === "preview" ? "stdbtn is-active" : "stdbtn"}
              onClick={() => setTab("preview")}
            >
              {t("subscriptions.tabPreview")}
              {preview.data ? ` (${preview.data.candidates.length})` : ""}
            </button>
          </div>
          {/* Both stay mounted: unmounting the form would discard what has been typed whenever the
              preview is looked at. */}
          <div style={{ display: tab === "form" ? "block" : "none" }}>{formPane}</div>
          <div style={{ display: tab === "preview" ? "block" : "none" }}>{previewPane}</div>
        </>
      )}
    </Modal>
  )
}
