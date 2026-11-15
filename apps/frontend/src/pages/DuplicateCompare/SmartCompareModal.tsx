import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { useDeleteArchive, useDuplicateCompare, useSaveDuplicateDecisions } from "@/api/hooks"
import type {
  ComparisonSide,
  DuplicateCompare,
  DuplicateGroupView,
  DuplicatePatchResult,
  DuplicateReview,
} from "@/api/types"
import { IconButton } from "@/components/common-ui/Form"
import { formatBytes } from "@/components/Display"
import { useIsNarrowViewport } from "@/hooks"
import { routes } from "@/lib/routes"
import { EntryTreePopover } from "@/pages/Upload/EntryTreePopover"
import { createTouchMagnifyStore, TouchMagnifyStoreContext, useTouchMagnifyStore } from "@/store"
import { FONT_SIZE_MD, FONT_SIZE_SM, FONT_SIZE_XS } from "@/theme"
import { toast } from "@/toast"

import { HoldButtons, MultiCompare } from "./MultiCompare"
import { OnlyHerePages, PageLightbox } from "./OnlyHerePages"
import { OverlayPage } from "./OverlayPage"
import { type JumpTarget, PageAlignmentBand } from "./PageAlignmentBand"
import { PatchAssignment } from "./PatchAssignment"
import { SideBadge } from "./SideBadge"
import { SIDE_COLORS } from "./sideColors"

export type CompareDecision = "keep-a" | "keep-b" | "keep-both" | "not-duplicate"

/** Full-viewport comparison of two group members (spec 024 US2/US3). Always dark, like the reader:
 *  it is an image viewer, and the theme's own text colors were unreadable on it. On a phone the
 *  decision buttons move to a bottom bar within thumb reach and the archive details fold away. */
export function SmartCompareModal({
  group,
  pair,
  onDecided,
  onClose,
}: {
  group: DuplicateGroupView
  pair: [string, string]
  /** The keep/discard states just saved, so the card's checkboxes can follow. */
  onDecided: (decisions: Record<string, "keep" | "discard">) => void
  onClose: () => void
}) {
  const { t } = useTranslation()
  const narrow = useIsNarrowViewport()
  const [[aId, bId], setPair] = useState<[string, string]>(pair)
  const compare = useDuplicateCompare(aId, bId)
  const save = useSaveDuplicateDecisions()
  const deleteArchive = useDeleteArchive()
  const data = compare.data
  const samples = data?.samples ?? []
  const [touchMagnifyStore] = useState(() => createTouchMagnifyStore())
  const [detailsOpen, setDetailsOpen] = useState(!narrow)
  // Three or more copies are always compared together; the pair state then only backs patching.
  const multi = group.members.length > 2
  const [anchor, setAnchor] = useState(pair[0])
  const [held, setHeld] = useState<number | null>(null)
  const [refresh, setRefresh] = useState(0)
  const [current, setCurrent] = useState<number | null>(null)
  const [flash, setFlash] = useState<number | null>(null)
  const scrollRef = useRef<HTMLDivElement | null>(null)
  const sampleRefs = useRef<(HTMLDivElement | null)[]>([])
  const onlyTiles = useRef(new Map<string, HTMLButtonElement>())
  const [flashOnly, setFlashOnly] = useState<{ side: ComparisonSide; page: number } | null>(null)
  const [lightbox, setLightbox] = useState<{ side: ComparisonSide; index: number } | null>(null)
  // `source` is the side whose only-here pages get copied into the other side's patch; `then` is
  // the keep decision to record once that is done.
  const [patchFor, setPatchFor] = useState<{ source: ComparisonSide; then?: CompareDecision } | null>(null)
  const [confirmKeep, setConfirmKeep] = useState<CompareDecision | null>(null)

  useEffect(() => {
    function onKeyDown(e: KeyboardEvent) {
      if (e.key !== "Escape") return
      if (patchFor) setPatchFor(null)
      else if (confirmKeep) setConfirmKeep(null)
      else if (lightbox) setLightbox(null)
      else onClose()
    }
    window.addEventListener("keydown", onKeyDown)
    return () => window.removeEventListener("keydown", onKeyDown)
  }, [onClose, lightbox, patchFor, confirmKeep])

  useEffect(() => {
    const prevOverflow = document.body.style.overflow
    const prevOverscroll = document.body.style.overscrollBehavior
    document.body.style.overflow = "hidden"
    document.body.style.overscrollBehavior = "none"
    return () => {
      document.body.style.overflow = prevOverflow
      document.body.style.overscrollBehavior = prevOverscroll
    }
  }, [])

  // The band highlights whichever sample is most in view, so scrolling and the band stay in step.
  useEffect(() => {
    const root = scrollRef.current
    if (!root || samples.length === 0) return
    const ratios = new Map<number, number>()
    const observer = new IntersectionObserver(
      (entries) => {
        for (const entry of entries) {
          ratios.set(Number((entry.target as HTMLElement).dataset.sample), entry.intersectionRatio)
        }
        let best: number | null = null
        let bestRatio = 0
        for (const [index, ratio] of ratios) {
          if (ratio > bestRatio) {
            best = index
            bestRatio = ratio
          }
        }
        setCurrent(best)
      },
      { root, threshold: [0, 0.25, 0.5, 0.75, 1] },
    )
    for (const el of sampleRefs.current) if (el) observer.observe(el)
    return () => observer.disconnect()
  }, [samples.length, aId, bId])

  useEffect(() => {
    if (flash === null && flashOnly === null) return
    const timer = setTimeout(() => {
      setFlash(null)
      setFlashOnly(null)
    }, 1200)
    return () => clearTimeout(timer)
  }, [flash, flashOnly])

  function jump(target: JumpTarget) {
    if (target.kind === "sample") {
      sampleRefs.current[target.index]?.scrollIntoView({ behavior: "smooth", block: "center" })
      setCurrent(target.index)
      setFlash(target.index)
    } else {
      onlyTiles.current.get(`${target.side}:${target.page}`)?.scrollIntoView({ behavior: "smooth", block: "center" })
      setFlashOnly({ side: target.side, page: target.page })
    }
  }

  const onlyPages = {
    a: data?.a_unmatched_pages.map((p) => p.page_index) ?? [],
    b: data?.b_unmatched_pages.map((p) => p.page_index) ?? [],
  }

  /** Keeping one side removes the other, so that goes through one confirmation (which also offers
   *  to patch in the pages only the removed side has). The other decisions only record. */
  function requestDecision(decision: CompareDecision) {
    if (decision === "keep-a" || decision === "keep-b") setConfirmKeep(decision)
    else void decide(decision)
  }

  function patchDone(result: DuplicatePatchResult, then?: CompareDecision) {
    setPatchFor(null)
    setRefresh((r) => r + 1)
    toast({
      heading: t("duplicates.patchDone", { added: result.added, skipped: result.skipped }),
      text: result.renamed > 0 ? t("duplicates.patchRenamed", { count: result.renamed }) : undefined,
      icon: "success",
    })
    if (then) void decide(then)
  }

  async function decide(decision: CompareDecision) {
    const decisions: Record<string, "keep" | "discard"> = {}
    if (decision === "keep-a") Object.assign(decisions, { [aId]: "keep", [bId]: "discard" })
    if (decision === "keep-b") Object.assign(decisions, { [aId]: "discard", [bId]: "keep" })
    if (decision === "keep-both") Object.assign(decisions, { [aId]: "keep", [bId]: "keep" })
    try {
      await save.mutateAsync({
        group_key: group.key,
        decisions,
        not_duplicate_pairs: decision === "not-duplicate" ? [[aId, bId]] : undefined,
      })
      onDecided(decisions)
    } catch (error) {
      toast({ heading: t("duplicates.decisionFailed"), text: String(error), icon: "error" })
      return
    }
    const discarded = decision === "keep-a" ? bId : decision === "keep-b" ? aId : null
    if (discarded) {
      try {
        // Into the recycle bin: restorable with its categories, tankoubons and progress.
        await deleteArchive.mutateAsync(discarded)
        toast({ heading: t("duplicates.keptAndRecycled", { title: memberTitle(discarded) }), icon: "success" })
      } catch (error) {
        toast({ heading: t("duplicates.recycleFailed"), text: String(error), icon: "error" })
        return
      }
    } else {
      toast({ heading: t("duplicates.decisionSaved"), icon: "success" })
    }
    onClose()
  }

  /** Multi view: patch `from`'s pages that `into` lacks into `into`, through the pair view's dialog. */
  function patchBetween(from: string, into: string) {
    const other = from === anchor ? into : from
    setPair([anchor, other])
    setPatchFor({ source: from === anchor ? "a" : "b" })
  }

  async function decideMany(decisions: Record<string, "keep" | "discard">, notDuplicate: boolean) {
    const ids = group.members.map((m) => m.id)
    const pairs = notDuplicate ? ids.flatMap((a, i) => ids.slice(i + 1).map((b) => [a, b] as [string, string])) : undefined
    try {
      await save.mutateAsync({ group_key: group.key, decisions, not_duplicate_pairs: pairs })
      onDecided(decisions)
    } catch (error) {
      toast({ heading: t("duplicates.decisionFailed"), text: String(error), icon: "error" })
      return
    }
    const discarded = ids.filter((id) => decisions[id] === "discard")
    for (const id of discarded) {
      try {
        await deleteArchive.mutateAsync(id)
      } catch (error) {
        toast({ heading: t("duplicates.recycleFailed"), text: `${memberTitle(id)}: ${String(error)}`, icon: "error" })
        return
      }
    }
    toast({
      heading: discarded.length
        ? t("duplicates.multiRecycled", { keep: ids.length - discarded.length, discard: discarded.length })
        : t("duplicates.decisionSaved"),
      icon: "success",
    })
    onClose()
  }

  const memberTitle = (id: string) => group.members.find((m) => m.id === id)?.title ?? id
  const suggestion = data ? pairSuggestion(group.review, data, aId, bId) : null
  const decisionBar = (
    <DecisionBar
      suggested={suggestion?.decision ?? null}
      disabled={save.isPending || deleteArchive.isPending || !data}
      narrow={narrow}
      onDecide={requestDecision}
    />
  )

  return (
    <TouchMagnifyStoreContext.Provider value={touchMagnifyStore}>
      <div
        role="dialog"
        aria-modal="true"
        aria-label={t("duplicates.smartCompare")}
        style={{
          position: "fixed",
          inset: 0,
          height: "100dvh",
          overflow: "hidden",
          overscrollBehavior: "none",
          boxSizing: "border-box",
          // Above legacy's own `.base-overlay` (9000); EntryTreePopover sits at 9600.
          zIndex: 9500,
          background: "#141416",
          color: "#f2f2f2",
          display: "flex",
          flexDirection: "column",
          gap: 8,
          padding: narrow
            ? "max(8px, env(safe-area-inset-top)) max(8px, env(safe-area-inset-right)) 0 max(8px, env(safe-area-inset-left))"
            : 16,
          fontSize: FONT_SIZE_MD,
          textAlign: "left",
        }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 8 }}>
          <h3
            style={{
              margin: 0,
              flex: 1,
              minWidth: 0,
              fontSize: narrow ? FONT_SIZE_MD : "1rem",
              color: "inherit",
              whiteSpace: "nowrap",
              overflow: "hidden",
              textOverflow: "ellipsis",
            }}
          >
            <i className="fa fa-columns" aria-hidden="true" style={{ marginRight: 6, opacity: 0.8 }} />
            {t("duplicates.smartCompare")}
            {data && !multi && (
              <span style={{ fontWeight: 400, opacity: 0.7, marginLeft: 8, fontSize: FONT_SIZE_SM }}>
                {t("duplicates.aligned", { count: data.aligned_pairs })}
              </span>
            )}
          </h3>
          {multi && (
            <HoldButtons count={group.members.length} held={held} narrow={narrow} onHeld={setHeld} />
          )}
          {narrow && !multi && (
            <IconButton
              icon={detailsOpen ? "fa fa-chevron-up" : "fa fa-info-circle"}
              onClick={() => setDetailsOpen((v) => !v)}
              size={36}
              title={t("duplicates.details")}
              style={{ borderRadius: "50%", touchAction: "manipulation" }}
            />
          )}
          <IconButton
            icon="fa fa-times"
            onClick={onClose}
            size={36}
            title={t("common.close")}
            className="modal-close-btn"
            style={{ borderRadius: "50%", touchAction: "manipulation" }}
          />
        </div>

        {multi ? (
          <MultiCompare
            group={group}
            anchor={anchor}
            narrow={narrow}
            held={held}
            onHeld={setHeld}
            refreshKey={refresh}
            busy={save.isPending || deleteArchive.isPending}
            onAnchor={setAnchor}
            onPatch={patchBetween}
            onExecute={(decisions, notDuplicate) => void decideMany(decisions, notDuplicate)}
          />
        ) : (
          <>
        {detailsOpen && (
          <div style={{ display: "flex", flexDirection: "column", gap: 4, maxHeight: narrow ? "40dvh" : undefined, overflowY: "auto" }}>
            <SideLine side="a" id={aId} title={data?.a.title ?? memberTitle(aId)} data={data} note={group.review?.notes?.[aId]} />
            <SideLine side="b" id={bId} title={data?.b.title ?? memberTitle(bId)} data={data} note={group.review?.notes?.[bId]} />
          </div>
        )}

        {data && (
          <Verdict
            data={data}
            suggestion={suggestion}
            reason={group.review && !group.review.fallback ? group.review.reason || null : null}
            trailing={narrow ? null : decisionBar}
          />
        )}

        {data && (
          <PageAlignmentBand
            aPages={data.a.pages}
            bPages={data.b.pages}
            aUnmatched={data.a_unmatched_pages.map((p) => p.page_index)}
            bUnmatched={data.b_unmatched_pages.map((p) => p.page_index)}
            aPatch={data.a_patch_pages ?? []}
            bPatch={data.b_patch_pages ?? []}
            samples={samples}
            current={current}
            compact={narrow}
            onJump={jump}
          />
        )}

        {samples.length > 0 && (
          <div style={{ fontSize: FONT_SIZE_XS, opacity: 0.7, textAlign: "center" }}>
            <i className="fa fa-info-circle" aria-hidden="true" style={{ marginRight: 4 }} />
            {t(narrow ? "duplicates.compareHintTouch" : "duplicates.compareHint")}
          </div>
        )}

        <div
          ref={scrollRef}
          style={{
            flex: 1,
            minHeight: 0,
            overflowY: "auto",
            overscrollBehavior: "contain",
            paddingBottom: 8,
          }}
        >
          {compare.isLoading && (
            <div style={{ display: "flex", flexDirection: "column", alignItems: "center", gap: 8, paddingTop: 48, opacity: 0.8 }}>
              <i className="fa fa-2x fa-spinner fa-spin" aria-hidden="true" />
              {t("duplicates.comparing")}
            </div>
          )}
          {compare.isError && (
            <div style={{ padding: 24, textAlign: "center" }}>
              <p style={{ margin: 0 }}>{String(compare.error)}</p>
              <p style={{ margin: "8px 0 0", opacity: 0.7, fontSize: FONT_SIZE_SM }}>{t("duplicates.compareFailedHint")}</p>
            </div>
          )}
          {data && samples.length === 0 && onlyPages.a.length + onlyPages.b.length === 0 && (
            <p style={{ textAlign: "center", opacity: 0.7, paddingTop: 32 }}>{t("duplicates.noSamples")}</p>
          )}
          {samples.length > 0 && (
            <div
              style={{
                display: "grid",
                // Each cell keeps a usable width; the grid wraps instead of shrinking pages to stamps.
                gridTemplateColumns: narrow ? "1fr" : "repeat(auto-fill, minmax(min(100%, 360px), 1fr))",
                gap: narrow ? 20 : 24,
                padding: 4,
              }}
            >
              {samples.map((sample, i) => (
                <div
                  key={`${aId}-${bId}-${sample.a_page_index}-${sample.b_page_index}`}
                  ref={(el) => {
                    sampleRefs.current[i] = el
                  }}
                  data-sample={i}
                  style={{ minWidth: 0, scrollMarginTop: 8 }}
                >
                  <OverlayPage aId={aId} bId={bId} sample={sample} highlighted={flash === i} />
                </div>
              ))}
            </div>
          )}
          {data &&
            (["a", "b"] as const).map((side) => (
              <OnlyHerePages
                key={`${side}-${side === "a" ? aId : bId}`}
                side={side}
                archiveId={side === "a" ? aId : bId}
                pages={onlyPages[side]}
                patchPages={new Set((side === "a" ? data.a_patch_pages : data.b_patch_pages) ?? [])}
                flashPage={flashOnly?.side === side ? flashOnly.page : null}
                registerTile={(page, el) => {
                  const key = `${side}:${page}`
                  if (el) onlyTiles.current.set(key, el)
                  else onlyTiles.current.delete(key)
                }}
                onOpen={(index) => setLightbox({ side, index })}
                onPatch={() => setPatchFor({ source: side })}
              />
            ))}
        </div>

        {narrow && (
          <div
            style={{
              borderTop: "1px solid rgba(255,255,255,0.15)",
              padding: "8px 0 max(8px, env(safe-area-inset-bottom))",
            }}
          >
            {decisionBar}
          </div>
        )}
          </>
        )}
        {confirmKeep && (
          <KeepConfirm
            keep={confirmKeep === "keep-a" ? "a" : "b"}
            discardTitle={memberTitle(confirmKeep === "keep-a" ? bId : aId)}
            onlyCount={onlyPages[confirmKeep === "keep-a" ? "b" : "a"].length}
            busy={save.isPending || deleteArchive.isPending}
            onPatch={() => {
              setPatchFor({ source: confirmKeep === "keep-a" ? "b" : "a", then: confirmKeep })
              setConfirmKeep(null)
            }}
            onConfirm={() => {
              void decide(confirmKeep)
              setConfirmKeep(null)
            }}
            onCancel={() => setConfirmKeep(null)}
          />
        )}
        {patchFor && data && (
          <PatchAssignment
            source={{
              side: patchFor.source,
              id: patchFor.source === "a" ? aId : bId,
              pages: data[patchFor.source].pages,
              patchPages: (patchFor.source === "a" ? data.a_patch_pages : data.b_patch_pages) ?? [],
            }}
            target={{
              side: patchFor.source === "a" ? "b" : "a",
              id: patchFor.source === "a" ? bId : aId,
              pages: data[patchFor.source === "a" ? "b" : "a"].pages,
              patchPages: (patchFor.source === "a" ? data.b_patch_pages : data.a_patch_pages) ?? [],
            }}
            unmatched={patchFor.source === "a" ? data.a_unmatched_pages : data.b_unmatched_pages}
            onDone={(result) => patchDone(result, patchFor.then)}
            onCancel={() => setPatchFor(null)}
          />
        )}
        {lightbox && (
          <PageLightbox
            side={lightbox.side}
            archiveId={lightbox.side === "a" ? aId : bId}
            pages={onlyPages[lightbox.side]}
            index={lightbox.index}
            onIndex={(index) => setLightbox({ side: lightbox.side, index })}
            onClose={() => setLightbox(null)}
          />
        )}
        <TouchMagnifyToggleButton bottomOffset={narrow ? 120 : 24} />
      </div>
    </TouchMagnifyStoreContext.Provider>
  )
}

function SideLine({
  side,
  id,
  title,
  data,
  note,
}: {
  side: ComparisonSide
  id: string
  title: string
  data: DuplicateCompare | undefined
  /** The model's note on this member from the group review. */
  note?: string
}) {
  const { t } = useTranslation()
  const info = data ? data[side] : null
  const sharpness = data && data.samples.length > 0
    ? data.samples.reduce((sum, s) => sum + (side === "a" ? s.a_sharpness : s.b_sharpness), 0) / data.samples.length
    : null
  const entries = data ? (side === "a" ? data.a_entries : data.b_entries) : null
  const patchCount = (data ? (side === "a" ? data.a_patch_pages : data.b_patch_pages) : null)?.length ?? 0
  return (
    <div style={{ display: "flex", alignItems: "flex-start", gap: 8, fontSize: FONT_SIZE_SM }}>
      <SideBadge side={side} />
      <div style={{ minWidth: 0, flex: 1 }}>
        <div
          title={title}
          style={{
            display: "-webkit-box",
            WebkitBoxOrient: "vertical",
            WebkitLineClamp: 2,
            overflow: "hidden",
            wordBreak: "break-all",
          }}
        >
          {title}
        </div>
        <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", columnGap: 10, opacity: 0.7, fontSize: FONT_SIZE_XS }}>
          {info && <span>{info.pages} {t("duplicates.pages")}</span>}
          {info && <span>{formatBytes(info.size)}</span>}
          {patchCount > 0 && (
            <span>
              <i className="fa fa-puzzle-piece" aria-hidden="true" style={{ marginRight: 3 }} />
              {t("duplicates.patchPagesIncluded", { count: patchCount })}
            </span>
          )}
          {sharpness !== null && <span>{t("duplicates.avgSharpness", { score: sharpness.toFixed(0) })}</span>}
          <a href={routes.reader(id)} target="_blank" rel="noreferrer" style={{ color: "inherit", textDecoration: "underline" }}>
            {t("common.read")}
          </a>
          {entries && entries.length > 0 && <EntryTreePopover entries={entries} />}
        </div>
        {note && (
          <div style={{ fontSize: FONT_SIZE_XS, opacity: 0.75, marginTop: 2 }}>
            <i className="fa fa-comment-dots" aria-hidden="true" style={{ marginRight: 4 }} />
            {note}
          </div>
        )}
      </div>
    </div>
  )
}

/** What this pair should become. The group's semantic review wins when a model actually ran: it
 *  reads titles and tags, so it can tell a translation from a rescan, which sharpness cannot. The
 *  page comparison only decides when there is no such review. */
export function pairSuggestion(
  review: DuplicateReview | null,
  data: DuplicateCompare,
  aId: string,
  bId: string,
): { decision: CompareDecision; fromReview: boolean } | null {
  if (review && !review.fallback) {
    if (review.verdict === "adjacent_volumes" || review.verdict === "unrelated") {
      return { decision: "not-duplicate", fromReview: true }
    }
    const judged = (id: string) => review.keep.includes(id) || review.discard.includes(id)
    if (judged(aId) && judged(bId)) {
      const keepA = review.keep.includes(aId)
      const keepB = review.keep.includes(bId)
      if (keepA && keepB) return { decision: "keep-both", fromReview: true }
      if (keepA) return { decision: "keep-a", fromReview: true }
      if (keepB) return { decision: "keep-b", fromReview: true }
    }
  }
  if (data.likely_different_language) return { decision: "keep-both", fromReview: false }
  if (data.recommendation) {
    return { decision: data.recommendation === "a" ? "keep-a" : "keep-b", fromReview: false }
  }
  return null
}

function Verdict({
  data,
  suggestion,
  reason,
  trailing,
}: {
  data: DuplicateCompare
  suggestion: ReturnType<typeof pairSuggestion>
  reason: string | null
  trailing: React.ReactNode
}) {
  const { t } = useTranslation()
  const bulb = <i className="fa fa-lightbulb" aria-hidden="true" />
  const bold = { display: "inline-flex", alignItems: "center", gap: 6, fontWeight: 700 } as const
  let headline: React.ReactNode
  switch (suggestion?.decision) {
    case "keep-a":
    case "keep-b":
      headline = (
        <span style={bold}>
          {bulb}
          {t("duplicates.suggestKeep")}
          <SideBadge side={suggestion.decision === "keep-a" ? "a" : "b"} />
        </span>
      )
      break
    case "keep-both":
      headline = <span style={bold}>{bulb}{t("duplicates.suggestKeepBoth")}</span>
      break
    case "not-duplicate":
      headline = <span style={bold}>{bulb}{t("duplicates.suggestNotDuplicate")}</span>
      break
    default:
      headline = <span style={{ opacity: 0.85 }}>{t("duplicates.notConfident")}</span>
  }
  // Sharpness is still worth knowing when the review overrode it, but as a fact, not the advice.
  const quality =
    data.recommendation && suggestion?.fromReview
      ? t("duplicates.qualityBetter", { side: data.recommendation.toUpperCase() })
      : null
  const explanation =
    reason || (suggestion && !suggestion.fromReview && data.likely_different_language ? t("duplicates.differentLanguageLong") : null)
  return (
    <div style={{ display: "flex", flexWrap: "wrap", alignItems: "center", gap: 12 }}>
      <div style={{ flex: "1 1 280px", minWidth: 0 }}>
        {headline}
        {explanation && <div style={{ fontSize: FONT_SIZE_XS, opacity: 0.7, marginTop: 2 }}>{explanation}</div>}
        {quality && <div style={{ fontSize: FONT_SIZE_XS, opacity: 0.6, marginTop: 2 }}>{quality}</div>}
      </div>
      {trailing}
    </div>
  )
}

function DecisionBar({
  suggested,
  disabled,
  narrow,
  onDecide,
}: {
  suggested: CompareDecision | null
  disabled: boolean
  narrow: boolean
  onDecide: (decision: CompareDecision) => void
}) {
  const { t } = useTranslation()
  const buttons: { decision: CompareDecision; label: string; side?: ComparisonSide }[] = [
    { decision: "keep-a", label: t("duplicates.keepSide", { side: "A" }), side: "a" },
    { decision: "keep-b", label: t("duplicates.keepSide", { side: "B" }), side: "b" },
    { decision: "keep-both", label: t("duplicates.keepBoth") },
    { decision: "not-duplicate", label: t("duplicates.notDuplicate") },
  ]
  return (
    <div
      style={{
        display: "grid",
        gridTemplateColumns: narrow ? "1fr 1fr" : "repeat(4, auto)",
        gap: 8,
      }}
    >
      {buttons.map((b) => {
        const recommended = b.decision === suggested
        return (
          <button
            key={b.decision}
            type="button"
            className="stdbtn"
            disabled={disabled}
            onClick={() => onDecide(b.decision)}
            title={recommended ? t("duplicates.recommended") : undefined}
            style={{
              margin: 0,
              minHeight: narrow ? 44 : 32,
              padding: "0 14px",
              display: "inline-flex",
              alignItems: "center",
              justifyContent: "center",
              gap: 6,
              fontSize: FONT_SIZE_SM,
              fontWeight: recommended ? 700 : undefined,
              touchAction: "manipulation",
              boxShadow: recommended ? `0 0 0 2px ${b.side ? SIDE_COLORS[b.side] : "rgba(255,255,255,0.85)"}` : undefined,
            }}
          >
            {recommended && <i className="fa fa-star" aria-hidden="true" />}
            {b.label}
          </button>
        )
      })}
    </div>
  )
}

/** The single fixed A/B toggle for whichever sample currently owns a touch-magnify session: the
 *  finger holding the lens cannot also press Shift. */
function TouchMagnifyToggleButton({ bottomOffset }: { bottomOffset: number }) {
  const { t } = useTranslation()
  const active = useTouchMagnifyStore((s) => s.active)
  if (!active) return null
  return (
    <button
      type="button"
      onTouchStart={(e) => {
        e.stopPropagation()
        active.toggleSide()
      }}
      aria-label={t("duplicates.switchSide")}
      style={{
        position: "fixed",
        bottom: `calc(${bottomOffset}px + env(safe-area-inset-bottom))`,
        left: active.toggleOnRight ? undefined : 24,
        right: active.toggleOnRight ? 24 : undefined,
        zIndex: 9800,
        width: 52,
        height: 52,
        padding: 0,
        border: 0,
        borderRadius: "50%",
        background: "transparent",
        boxShadow: "0 2px 8px rgba(0,0,0,0.5)",
      }}
    >
      <SideBadge side={active.showB ? "b" : "a"} size={52} active />
    </button>
  )
}

/** Keeping one side moves the other to the recycle bin. When the removed side has pages the kept
 *  one lacks, the same dialog offers to patch them in first. */
function KeepConfirm({
  keep,
  discardTitle,
  onlyCount,
  busy,
  onPatch,
  onConfirm,
  onCancel,
}: {
  keep: ComparisonSide
  discardTitle: string
  onlyCount: number
  busy: boolean
  onPatch: () => void
  onConfirm: () => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  const kept = keep.toUpperCase()
  const removed = keep === "a" ? "B" : "A"
  return (
    <div
      role="dialog"
      aria-modal="true"
      onClick={onCancel}
      style={{ position: "fixed", inset: 0, zIndex: 9600, background: "rgba(0,0,0,0.6)", display: "flex", alignItems: "center", justifyContent: "center", padding: 16 }}
    >
      <div
        onClick={(e) => e.stopPropagation()}
        style={{ background: "#1d1d21", color: "#f2f2f2", borderRadius: 8, padding: 16, maxWidth: 440, width: "100%", boxShadow: "0 8px 24px rgba(0,0,0,0.5)" }}
      >
        <div style={{ display: "flex", alignItems: "center", gap: 8, fontWeight: 700, marginBottom: 8 }}>
          <SideBadge side={keep} />
          {t("duplicates.keepConfirmTitle", { kept, removed })}
        </div>
        <p style={{ margin: "0 0 8px", fontSize: FONT_SIZE_SM, opacity: 0.85, wordBreak: "break-all" }}>
          {t("duplicates.keepConfirmBody", { removed, title: discardTitle })}
        </p>
        {onlyCount > 0 && (
          <p style={{ margin: "0 0 12px", fontSize: FONT_SIZE_SM, padding: "8px 10px", borderRadius: 6, background: "rgba(230,126,34,0.18)" }}>
            <i className="fa fa-puzzle-piece" aria-hidden="true" style={{ marginRight: 6 }} />
            {t("duplicates.keepConfirmOnlyPages", { removed, kept, count: onlyCount })}
          </p>
        )}
        <div style={{ display: "flex", flexDirection: "column", gap: 8, marginTop: 12 }}>
          {onlyCount > 0 && (
            <button type="button" className="stdbtn" disabled={busy} style={{ margin: 0, minHeight: 40, fontWeight: 700 }} onClick={onPatch}>
              {t("duplicates.askPatchYes", { target: kept, removed })}
            </button>
          )}
          <button
            type="button"
            className="stdbtn"
            disabled={busy}
            style={{ margin: 0, minHeight: 40, fontWeight: onlyCount > 0 ? undefined : 700 }}
            onClick={onConfirm}
          >
            {onlyCount > 0 ? t("duplicates.askPatchNo", { target: kept, removed }) : t("duplicates.keepConfirmYes", { kept, removed })}
          </button>
          <button type="button" className="stdbtn" style={{ margin: 0, minHeight: 40 }} onClick={onCancel}>
            {t("common.cancel")}
          </button>
        </div>
      </div>
    </div>
  )
}
