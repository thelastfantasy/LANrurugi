import type { TFunction } from "i18next"
import { type CSSProperties, useLayoutEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { useBlockSubscriptionSources, useForgetSeen } from "@/api/hooks"
import type { CandidateRecord, SubscriptionPreview as Preview } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { StarRatingDisplay } from "@/components/common-ui/Form"
import { DateTimeStack } from "@/components/Display"
import { TagTable } from "@/components/Display/TagTable"
import { useIsNarrowViewport } from "@/hooks"
import { useLanguageOrder } from "@/i18n/useLanguageOrder"
import { ICON_BUTTON_STYLE } from "@/pages/Upload/shared"

import { CandidateTitleLink } from "./CandidateTitleLink"
import { tooSoonText } from "./subscriptionVerdicts"

/** Which of the three bands a candidate falls into.
 *
 * Three rather than two: "would be taken" and "ruled out" are the question asked, but a candidate
 * still inside a waiting period is neither — it will be reconsidered, and colouring it as excluded
 * would say the opposite of what happens. */
function bandOf(record: CandidateRecord): "match" | "waiting" | "excluded" | "held" {
  const v = record.verdict
  switch (v.verdict) {
    case "queued":
    case "awaiting_approval":
      return "match"
    case "too_soon":
      return "waiting"
    // Already in the library — either this very work or a newer revision of it: nothing was turned
    // away, so it does not belong in the rejection band.
    case "already_held":
    case "superseded":
      return "held"
    default:
      return "excluded"
  }
}

/** A pair of row actions has to read as a pair: the block button is a 24×24 icon
 *  (`ICON_BUTTON_STYLE`), so the text button beside it is given that same height and centred text —
 *  otherwise the two sit at 24px and 21px and the row looks misaligned, which is exactly what it did
 *  on a phone-sized window. */
const ROW_ACTION_BUTTON_STYLE: React.CSSProperties = {
  height: 24,
  boxSizing: "border-box",
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
}

const ROW_CLASS = {
  match: "preview-row-match",
  waiting: "preview-row-waiting",
  excluded: "preview-row-excluded",
  held: "preview-row-held",
} as const

/** The shared preview grid's track list, driven by the fields the source actually reports. Title,
 * index and verdict are always present; source-specific columns (uploader, rating, publication
 * time) are omitted for plugins that never populate them.
 *
 * The title carries a percentage *minimum*, not a bare `1fr`: the other columns take their own
 * maxima first, so on a narrow pane — the form's right-hand split, where this is most often read —
 * they used to consume the whole width and leave the one column that says what a row *is* with
 * nothing, wrapping every title over two or three lines while the columns beside it sat half
 * empty. A minimum share is what keeps that from happening at any pane width.
 *
 * The optional columns are sized in percentages for the same reason: they scale with the pane, so
 * the title's share no longer depends on how wide the dialog happens to be. Each is a little wider
 * than its own content needs (the five-star row is the widest fixed thing here, at a fraction of
 * 12%), which is what makes the remaining slack go to the title. */
export function previewGridColumns(fields?: string[]): string {
  const includes = (field: string) => fields === undefined || fields.includes(field)
  // The title keeps a *share*, not a fixed width, so it cannot starve the columns beside it.
  const columns = ["2.5em", "minmax(40%, 1fr)"]
  if (includes("posted_at")) columns.push("minmax(0, 10%)")
  // A real floor, not `minmax(0, …)`: with only a zero minimum the uploader track was squeezed to
  // nothing whenever the title's own share and the rating's/verdict's fixed floors took the pane
  // (reported live, 2026-10-09: "上传者栏太窄了"). 100px covers an ordinary handle — longer ones
  // still ellipsize into `UploaderCell`'s tooltip, which is the intended behaviour.
  if (includes("uploader")) columns.push("minmax(100px, 18%)")
  // The five-star row is 70px of fixed-width sprites (5 × 14px, `StarRatingDisplay`'s own `size`),
  // and a bare `minmax(0, 12%)` let the track shrink below that on the form's own split pane: the
  // stars then overflowed their cell and landed on top of the verdict's long, right-aligned first
  // line (confirmed live, 2026-10-09). The floor is that 70px plus the cell's own 16px padding —
  // the same "floor at the content's own width" rule the verdict column below records.
  if (includes("rating")) columns.push("minmax(86px, 12%)")
  // The verdict column holds a *short* label (see `Reason`, which moved the full sentence into a
  // tooltip) plus this row's two actions (block + reconsider) on one line. Sizing it for the old
  // full sentence was what forced the long wraps this column used to show; the floor now only has
  // to fit the two icon buttons (≈56px) and the label beside them, and `flex-wrap` on
  // `.preview-row-actions` remains the fallback for anything narrower still.
  columns.push("minmax(80px, 11%)")
  return columns.join(" ")
}

/** Why this candidate landed where it did: a short label in the column, the sentence in a tooltip.
 *
 * The full sentences ("先前检查中已见过", "被规则跳过：…") are what made this column wide enough to
 * wrap — or to squeeze the uploader track to nothing, since both are competing for the same pane
 * (reported live, 2026-10-09). The label answers "what happens to this row" at a glance; the reason
 * itself is one hover away, unchanged. */
function Reason({ record }: { record: CandidateRecord }) {
  const { t } = useTranslation()
  const { short, full } = reasonText(t, record)
  return (
    // Above `Modal`'s own 9001 — see this file's other in-dialog tooltips.
    <Tooltip label={full} zIndex={9700} wrapperStyle={{ display: "inline-flex", minWidth: 0 }}>
      <span className="preview-verdict-short">{short}</span>
    </Tooltip>
  )
}

/** The short column label and the full sentence behind it, for one verdict. */
function reasonText(
  t: TFunction,
  record: CandidateRecord,
): { short: string; full: string } {
  const v = record.verdict
  switch (v.verdict) {
    case "queued":
      return {
        short: t("subscriptions.verdictShort.queued"),
        full: t("subscriptions.previewWouldDownload"),
      }
    case "awaiting_approval":
      return {
        short: t("subscriptions.verdictShort.awaitingApproval"),
        full: t("subscriptions.previewWouldAsk"),
      }
    case "too_soon":
      return {
        short: t("subscriptions.verdictShort.tooSoon"),
        full: tooSoonText(t, v.reason),
      }
    case "rejected":
      return {
        short: t("subscriptions.verdictShort.rejected"),
        full: t("subscriptions.verdictRejected", { rule: v.rule }),
      }
    case "reserved":
      return {
        short: t("subscriptions.verdictShort.reserved"),
        full: t("subscriptions.verdictReserved", { reason: v.reason }),
      }
    case "already_held":
      return {
        short: t("subscriptions.verdictShort.alreadyHeld"),
        full: t("subscriptions.verdictAlreadyHeld"),
      }
    case "superseded":
      return {
        short: t("subscriptions.verdictShort.superseded"),
        full: t("subscriptions.verdictSuperseded", { source: v.newer_source }),
      }
    default:
      return {
        short: t("subscriptions.verdictShort.alreadySeen"),
        full: t("subscriptions.verdictAlreadySeen"),
      }
  }
}

/** Everything the source listed, split into what the rules would take and what they would not.
 *
 * Shows the whole listing rather than only the matches: a rule that is too strict looks identical to
 * a source with nothing new unless the works it turned away are visible alongside the reason. */
/** Whether `ref`'s content is currently wider than its box — i.e. the ellipsis is showing.
 *
 * Measured rather than guessed from a character count: the column is a percentage of the pane, so
 * the very same name is truncated in the form's split view and not in a maximised dialog, and a
 * tooltip on an untruncated name would just be noise over text already fully visible.
 *
 * Re-measured after every render (cheap: one layout read) and on resize, since the pane's width
 * changes with the dialog, and after the webfont settles, which changes the text's own width. */
export function useIsTruncated<T extends HTMLElement>() {
  const ref = useRef<T | null>(null)
  const [truncated, setTruncated] = useState(false)
  // Runs after every render, not just the first: the element this measures is replaced whenever the
  // tooltip wrapper appears or disappears (see `UploaderCell`), so a once-only effect would end up
  // measuring a node that is no longer in the document. `ResizeObserver` covers the pane-width and
  // webfont cases, where the same node's box changes without a re-render.
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const measure = () => {
      const next = el.scrollWidth > el.clientWidth + 1
      setTruncated((prev) => (prev === next ? prev : next))
    }
    measure()
    const observer = new ResizeObserver(measure)
    observer.observe(el)
    void document.fonts?.ready?.then(measure).catch(() => {})
    return () => observer.disconnect()
  })
  return { ref, truncated }
}

/** The uploader name, with its full value in a tooltip whenever the column had to truncate it.
 * A handle is often the only thing telling two rows apart, and "kingdomc…" is neither readable nor
 * copyable — the bubble's text is selectable, so the full name can be taken from it. */
function UploaderCell({ name }: { name: string }) {
  const { ref, truncated } = useIsTruncated<HTMLSpanElement>()
  const cell = (
    <span ref={ref} className="preview-uploader">
      {name}
    </span>
  )
  if (!name || !truncated) return cell
  return (
    // `zIndex` 9700: this grid is read inside a modal (Modal.tsx's own 9001), where the bubble's
    // default 1100 renders behind it. `wrapperStyle` only turns the trigger into a block that fills
    // its cell — deliberately *not* `width: 100%`, which under `content-box` would make the wrapper
    // 16px wider than the cell (it carries `.preview-row > *`'s padding) and give the name inside a
    // box wide enough not to truncate: the measurement would then flip back, unwrap, and flip
    // again, forever.
    <Tooltip label={name} zIndex={9700} wrapperStyle={{ display: "block", minWidth: 0 }}>
      {cell}
    </Tooltip>
  )
}

export function SubscriptionPreview({
  preview,
  subscriptionId,
  fields,
}: {
  preview: Preview
  /** Omitted when previewing unsaved rules — there is no stored seen set to act on yet. */
  subscriptionId?: string
  /** The source's declared candidate fields. Drives which optional columns are rendered. */
  fields?: string[]
}) {
  const { t } = useTranslation()
  const forget = useForgetSeen()
  const block = useBlockSubscriptionSources()
  const titleOrder = useLanguageOrder()
  const narrow = useIsNarrowViewport("(max-width: 760px)")
  const rows = preview.candidates
  const includeField = (field: string) => fields === undefined || fields.includes(field)
  const columns = previewGridColumns(fields)

  if (rows.length === 0) {
    return <p>{t("subscriptions.previewFoundNothing")}</p>
  }

  const counts: Record<ReturnType<typeof bandOf>, number> = {
    match: 0,
    waiting: 0,
    excluded: 0,
    held: 0,
  }
  for (const r of rows) counts[bandOf(r)]++

  return (
    <div>
      <p style={{ margin: "0 0 6px" }}>
        {t("subscriptions.previewSummary", {
          total: rows.length,
          matched: counts.match,
          excluded: counts.excluded,
        })}
        {counts.waiting > 0 && ` ${t("subscriptions.previewWaitingCount", { count: counts.waiting })}`}
      </p>

      {preview.listing_pages !== undefined && preview.check_listing_pages !== undefined && (
        // Why this list is shorter than a check's: a preview fetches only the newest listing page
        // (each page is another request to the source), while the hourly check reads them all.
        <p className="sub-form-hint" style={{ margin: "0 0 6px" }}>
          {t("subscriptions.previewWindow", {
            pages: preview.listing_pages,
            checkPages: preview.check_listing_pages,
          })}
        </p>
      )}

      {preview.outcome !== "completed" && (
        // An incomplete view must not read as "this is everything that exists" — the preview is a
        // smaller answer than the one asked for.
        <p className="preview-row-waiting" style={{ padding: 4, margin: "0 0 6px" }}>
          {t("subscriptions.previewIncomplete")}
        </p>
      )}

      <div
        className="preview-grid"
        style={{ "--preview-grid-columns": columns } as CSSProperties}
      >
        {/* Named columns: "would ask you first" says nothing on its own about what it is answering. */}
        <div className="preview-row preview-head">
          <span className="preview-index">#</span>
          <span className="preview-title">{t("subscriptions.colWork")}</span>
          {includeField("posted_at") && (
            <span className="preview-posted">{t("subscriptions.field.posted_at")}</span>
          )}
          {includeField("uploader") && (
            <span className="preview-uploader">{t("subscriptions.colUploader")}</span>
          )}
          {includeField("rating") && (
            <span className="preview-rating">{t("subscriptions.field.rating")}</span>
          )}
          <span className="preview-verdict">
            {/* Short for the same reason the column's own values are: the full wording is the
                tooltip, so the header cannot be what sets this track's minimum width. */}
            <Tooltip
              label={t("subscriptions.colVerdict")}
              zIndex={9700}
              wrapperStyle={{ display: "inline-flex", minWidth: 0 }}
            >
              <span className="preview-verdict-short">{t("subscriptions.colVerdictShort")}</span>
            </Tooltip>
          </span>
        </div>
        {rows.map((r, i) => (
          <div key={r.source_url} className={`preview-row ${ROW_CLASS[bandOf(r)]}`}>
            {/* Numbered so a position in a long listing can be referred to, and unselectable so
                copying a row does not drag the numbering along with it. */}
            <span className="preview-index">{i + 1}</span>

            <span className="preview-title">
              <CandidateTitleLink record={r} order={titleOrder} />
              {(r.tags?.length ?? 0) > 0 && (
                // The shared tooltip rather than a `title` attribute: it is themed, appears without
                // the browser's delay, and can hold the tag list as real content rather than one
                // unstyled line.
                <Tooltip
                  label={<TagTable tags={(r.tags ?? []).join(",")} links={false} />}
                  wrapperStyle={{ display: "block", minWidth: 0 }}
                  // Above `Modal`'s own 9001: the bubble is portaled to the body, so without this it
                  // renders behind the dialog that contains its trigger — see Tooltip's own note.
                  zIndex={9700}
                >
                  {/* `span.tags` is legacy's own one-line-with-ellipsis rule. */}
                  <span className="tags">{(r.tags ?? []).join(", ")}</span>
                </Tooltip>
              )}
            </span>

            {includeField("posted_at") && (
              <span className="preview-posted">
                {r.posted_at != null ? (
                  narrow ? (
                    new Date(r.posted_at * 1000).toLocaleString()
                  ) : (
                    <DateTimeStack epochSeconds={r.posted_at} />
                  )
                ) : (
                  "—"
                )}
              </span>
            )}

            {includeField("uploader") && (
              // Its own column so uploaders line up down the page, the way the verdicts do.
              <UploaderCell name={r.uploader ?? ""} />
            )}

            {includeField("rating") && (
              <span className="preview-rating">
                {r.rating != null ? (
                  // The stars are a shape, not a number: a 4.5 and a 5 are hard to tell apart at
                  // 14px, and "which rating did it actually have" is what a rating floor is judged
                  // against. The exact value is one hover away rather than a column of digits.
                  <Tooltip
                    label={t("subscriptions.ratingValue", { rating: r.rating })}
                    // Above `Modal`'s own 9001 — see this file's other in-dialog tooltips.
                    zIndex={9700}
                  >
                    <span style={{ display: "inline-flex" }}>
                      <StarRatingDisplay rating={r.rating} size={14} />
                    </span>
                  </Tooltip>
                ) : (
                  "—"
                )}
              </span>
            )}

            <span className="preview-verdict">
              <Reason record={r} />
              {/* The two row actions share one inline-flex line so they are centred on each other:
                  as sibling inline elements the icon button and the text button aligned on their own
                  baselines and sat 3px apart at phone widths. */}
              {subscriptionId &&
                r.verdict.verdict !== "queued" &&
                r.verdict.verdict !== "awaiting_approval" && (
                  <span className="preview-row-actions">
                    {/* Blocking is offered on anything the rules did *not* already take, i.e. exactly
                        the rows a user reads this list to triage. A blocked work is settled for good
                        — the one rejection that is an answer rather than this host's guess at a value
                        that can still change — so it is an icon with a tooltip, not a button
                        competing with "reconsider". */}
                    <Tooltip
                          label={t("subscriptions.blockWork") ?? ""}
                          // Above `Modal`'s own 9001: the bubble is portalled to `body`, so with the
                          // default 1100 the dialog painted over all but the overflowing sliver.
                          zIndex={9700}
                        >
                          <button
                            type="button"
                            className="stdbtn"
                            style={ICON_BUTTON_STYLE}
                            aria-label={t("subscriptions.blockWork") ?? "Never download this"}
                            disabled={block.isPending}
                            onClick={() =>
                              void block.mutateAsync({ id: subscriptionId, sources: [r.source_url] })
                            }
                          >
                            <i className="fa fa-ban" aria-hidden="true"></i>
                    </button>
                    </Tooltip>
                    {r.verdict.verdict === "already_seen" && (
                      // The only way back: being seen is permanent otherwise. This covers both a work
                      // whose archive was deleted and a work rejected under a rule that has since
                      // been widened — both were recorded as seen, so both need the same escape
                      // hatch. `rejected` candidates are not shown this: they are being rejected by
                      // the current rules, not blocked by a previous "seen" decision.
                      <input
                        type="button"
                        className="stdbtn"
                        style={ROW_ACTION_BUTTON_STYLE}
                        disabled={forget.isPending}
                        value={t("subscriptions.reconsider") ?? undefined}
                        onClick={() => {
                          void forget.mutateAsync({
                            id: subscriptionId,
                            sources: [r.source_url],
                          })
                        }}
                      />
                    )}
                  </span>
                )}
            </span>
          </div>
        ))}
      </div>
    </div>
  )
}
