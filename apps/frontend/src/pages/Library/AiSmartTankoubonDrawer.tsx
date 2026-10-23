import { useQueryClient } from "@tanstack/react-query"
import { type CSSProperties, useEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import { sendJson } from "@/api/client"
import {
  type AiGroupSuggestion,
  type GroupReview,
  type PairEvidence,
  useAiGroupSuggestions,
  useAiRenameTankoubon,
  useArchives,
  useCreateTankoubon,
  useGroupSuggestionReview,
  useIgnoredGroupSuggestions,
  useIgnoreGroupSuggestion,
  useLlmKeyStatus,
  useRecordGroupLabels,
  useTankoubons,
  useUnignoreGroupSuggestion,
} from "@/api/hooks"
import { Drawer, DrawerFooter, Tooltip } from "@/components/common-ui/Display"
import { IconButton } from "@/components/common-ui/Form"
import { AiSkeleton, ArchiveChecklistItem } from "@/components/Display"
import { useHorizontalSwipe } from "@/hooks"
import { routes } from "@/lib/routes"
import { Z_OVERLAY_ABOVE_LEGACY_MODAL } from "@/theme"
import { toast } from "@/toast"

/** Members the reviewer rejected, with its reason. */
function rejectedReasons(review: GroupReview | undefined): Map<string, string> {
  const out = new Map<string, string>()
  for (const v of review?.verdicts ?? []) {
    if (!v.keep) out.set(v.archive_id, v.reason)
  }
  return out
}

/** One line saying what the AI review did with this card. */
function ReviewStatusLine({
  review,
  pending,
  total,
  rejected,
  minimum,
}: {
  review: GroupReview | undefined
  pending: boolean
  total: number
  rejected: number
  minimum: number
}) {
  const { t } = useTranslation()
  let icon = "fa-robot"
  let text: string
  if (pending) {
    icon = "fa-spinner fa-spin"
    text = t("library.groupReviewPending")
  } else if (!review) {
    return null
  } else if (review.status === "settled") {
    text = t("library.groupReviewSettled")
  } else if (review.status === "not_configured") {
    text = t("library.groupReviewNotConfigured")
  } else if (review.status === "fell_back") {
    text = t("library.groupReviewFellBack", { reason: review.reason ?? "" })
  } else if (total - rejected < minimum) {
    text = t("library.groupReviewNoSeries")
  } else if (rejected === 0) {
    text = t("library.groupReviewAllKept")
  } else {
    text = t("library.groupReviewKept", { kept: total - rejected, total })
  }
  return (
    <div style={{ padding: "6px 16px", fontSize: 12, opacity: 0.75, display: "flex", gap: 6, alignItems: "baseline" }}>
      <i className={`fa ${icon}`} aria-hidden="true" />
      <span>{text}</span>
    </div>
  )
}

const MAX_EVIDENCE_ROWS = 30

function pct(v: number) {
  return `${Math.round(v * 100)}%`
}

/** "Why are these grouped": per-pair scores and creator match, strongest pairs first. */
function EvidencePanel({ evidence, titleById }: { evidence: PairEvidence[]; titleById: Map<string, string> }) {
  const { t } = useTranslation()
  const rows = [...evidence].sort((x, y) => y.score - x.score)
  const shown = rows.slice(0, MAX_EVIDENCE_ROWS)
  const creatorLabel = (e: PairEvidence) =>
    e.shared_cosplayer
      ? t("library.groupEvidenceCosplayer")
      : e.creator_match === "same"
        ? t("library.groupEvidenceCreatorSame")
        : e.creator_match === "different"
          ? t("library.groupEvidenceCreatorDifferent")
          : t("library.groupEvidenceCreatorUnknown")
  const cell: CSSProperties = { padding: "3px 6px", fontVariantNumeric: "tabular-nums", whiteSpace: "nowrap" }
  return (
    <div style={{ padding: "4px 16px 8px", maxHeight: 220, overflowY: "auto" }}>
      <table className="itg" style={{ width: "100%", tableLayout: "fixed", fontSize: 11 }}>
        <colgroup>
          <col style={{ width: "40%" }} />
          <col style={{ width: "10%" }} />
          <col style={{ width: "10%" }} />
          <col style={{ width: "10%" }} />
          <col style={{ width: "10%" }} />
          <col style={{ width: "20%" }} />
        </colgroup>
        <thead>
          <tr>
            <th style={{ ...cell, textAlign: "left" }}>{t("library.groupEvidencePair")}</th>
            <th style={{ ...cell, textAlign: "right" }}>{t("library.groupEvidenceScore")}</th>
            <th style={{ ...cell, textAlign: "right" }}>{t("library.groupEvidenceTitle")}</th>
            <th style={{ ...cell, textAlign: "right" }}>{t("library.groupEvidenceTag")}</th>
            <th style={{ ...cell, textAlign: "right" }}>{t("library.groupEvidenceTokens")}</th>
            <th style={{ ...cell, textAlign: "left" }}>{t("library.groupEvidenceCreator")}</th>
          </tr>
        </thead>
        <tbody>
          {shown.map((e, i) => (
            <tr key={`${e.a}:${e.b}`} className={i % 2 ? "gtr1" : "gtr0"}>
              <td style={{ ...cell, overflow: "hidden", textOverflow: "ellipsis" }}>
                <div style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{titleById.get(e.a) ?? e.a}</div>
                <div style={{ overflow: "hidden", textOverflow: "ellipsis" }}>{titleById.get(e.b) ?? e.b}</div>
                {e.shared_top_tags.length > 0 && (
                  <div style={{ opacity: 0.6, overflow: "hidden", textOverflow: "ellipsis" }}>
                    {t("library.groupEvidenceSharedTags")}: {e.shared_top_tags.join(", ")}
                  </div>
                )}
              </td>
              <td style={{ ...cell, textAlign: "right", fontWeight: 700 }}>{pct(e.score)}</td>
              <td style={{ ...cell, textAlign: "right" }}>{pct(e.title)}</td>
              <td style={{ ...cell, textAlign: "right" }}>{pct(e.tag)}</td>
              <td style={{ ...cell, textAlign: "right" }}>{pct(e.tokens)}</td>
              <td style={{ ...cell, opacity: e.creator_match === "unknown" ? 0.55 : undefined }}>{creatorLabel(e)}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {rows.length > shown.length && (
        <div style={{ fontSize: 11, opacity: 0.6, marginTop: 4 }}>
          {t("library.groupEvidenceMore", { count: rows.length - shown.length })}
        </div>
      )}
    </div>
  )
}

/** A member's title. Hover shows its thumbnail, plus the full title when the row cut it off. */
function MemberTitle({ id, title, muted }: { id: string; title: string; muted: boolean }) {
  const [truncated, setTruncated] = useState(false)
  return (
    <Tooltip
      anchor="cursor"
      wrapperStyle={{ display: "inline" }}
      maxWidth={480}
      zIndex={Z_OVERLAY_ABOVE_LEGACY_MODAL}
      label={
        <div>
          {truncated && <div style={{ marginBottom: 6, wordBreak: "break-word" }}>{title}</div>}
          <img
            src={`/api/archives/${id}/thumbnail?no_fallback=true`}
            alt=""
            style={{ maxHeight: 420, maxWidth: "100%", display: "block" }}
          />
        </div>
      }
    >
      <span
        style={muted ? { opacity: 0.6 } : undefined}
        onPointerEnter={(e) => {
          const box = e.currentTarget.closest("[data-truncate-box]")
          setTruncated(!!box && box.scrollWidth > box.clientWidth)
        }}
      >
        {title}
      </span>
    </Tooltip>
  )
}

/** One AI-suggested group's card — members start checked, except those the AI review rejected
 * (unless the user already toggled them); "Create Tankoubon" is disabled below 2 checked. */
function SuggestionCard({
  suggestion,
  page,
  total,
  titleById,
  tankoubonNameById,
  onDismissed,
  onPrev,
  onNext,
  onJump,
}: {
  suggestion: AiGroupSuggestion
  page: number
  total: number
  titleById: Map<string, string>
  tankoubonNameById: Map<string, string>
  onDismissed: () => void
  onPrev: () => void
  onNext: () => void
  onJump: (page: number) => void
}) {
  const { t } = useTranslation()
  const createTankoubon = useCreateTankoubon()
  const ignoreGroupSuggestion = useIgnoreGroupSuggestion()
  const aiRenameTankoubon = useAiRenameTankoubon()
  const llmKeyStatus = useLlmKeyStatus()
  const queryClient = useQueryClient()
  const recordGroupLabels = useRecordGroupLabels()
  const review = useGroupSuggestionReview(suggestion, true)
  const rejected = rejectedReasons(review.data)
  // The user's own toggles; every other member follows the review (rejected → unticked), so a
  // review arriving after the user acted never overrides them.
  const [overrides, setOverrides] = useState<Map<string, boolean>>(() => new Map())
  const checked = new Set(suggestion.archive_ids.filter((id) => overrides.get(id) ?? !rejected.has(id)))
  const [creating, setCreating] = useState(false)
  const [autoRename, setAutoRename] = useState(true)
  const [showEvidence, setShowEvidence] = useState(false)

  const selectedIds = suggestion.archive_ids.filter((id) => checked.has(id))
  const isAddToExisting = !!suggestion.existing_tankoubon_id
  const minimum = isAddToExisting ? 1 : 2
  const canConfirm = selectedIds.length >= minimum && !creating

  /** Best effort: labels never block or fail the create. */
  function recordLabels() {
    recordGroupLabels.mutate({
      archive_ids: suggestion.archive_ids,
      existing_tankoubon_id: suggestion.existing_tankoubon_id,
      kept: selectedIds,
      unticked: suggestion.archive_ids.filter((id) => !checked.has(id)),
      reviewer_rejected_kept: selectedIds.filter((id) => rejected.has(id)),
    })
  }

  async function handleConfirm() {
    setCreating(true)
    try {
      if (isAddToExisting && suggestion.existing_tankoubon_id) {
        const tankId = suggestion.existing_tankoubon_id
        for (const archiveId of selectedIds) {
          await sendJson("PUT", `/tankoubons/${tankId}/${archiveId}`)
        }
        void queryClient.invalidateQueries({ queryKey: ["tankoubon", tankId] })
        void queryClient.invalidateQueries({ queryKey: ["tankoubons"] })
        void queryClient.invalidateQueries({ queryKey: ["archives"] })
        void queryClient.invalidateQueries({ predicate: (query) => query.queryKey[0] === "search" })
        toast({ heading: t("library.addedToTankoubon") ?? undefined, icon: "success" })
        recordLabels()
      } else {
        const name = selectedIds
          .map((id) => titleById.get(id))
          .find((title) => !!title) ?? t("library.newTankoubon") ?? "New Tankoubon"
        const { tankoubon_id } = await createTankoubon.mutateAsync(name)
        await sendJson("PUT", `/tankoubons/${tankoubon_id}`, { archives: selectedIds })
        recordLabels()

        let finalName = name
        if (autoRename && llmKeyStatus.data?.configured) {
          try {
            const renameResult = await aiRenameTankoubon.mutateAsync(tankoubon_id)
            const sug = renameResult.suggestions[0]
            if (sug) {
              const chapterNames: { id: string; name: string }[] = []
              const sorted = [...sug.chapters].sort((a, b) => a.sorted_index - b.sorted_index)
              const reordered: string[] = []
              for (const ch of sorted) {
                const mem = renameResult.original_member_names.find((m) => m.index === ch.original_index)
                if (mem) {
                  if (ch.name) chapterNames.push({ id: mem.id, name: ch.name })
                  reordered.push(mem.id)
                }
              }
              finalName = sug.tank_name || name
              // `name`/`chapter_names` must nest under `metadata` — a top-level field is
              // silently dropped by serde (unknown field, not an error).
              await sendJson("PUT", `/tankoubons/${tankoubon_id}`, {
                metadata: { name: finalName, chapter_names: chapterNames },
                archives: reordered.length === selectedIds.length ? reordered : selectedIds,
              })
            }
          } catch (renameErr) {
            toast({
              heading: t("library.aiRenameFailedTankoubon") ?? undefined,
              text: String(renameErr),
              icon: "warning",
            })
          }
        }

        void queryClient.invalidateQueries({ queryKey: ["tankoubons"] })
        void queryClient.invalidateQueries({ queryKey: ["archives"] })
        void queryClient.invalidateQueries({ predicate: (query) => query.queryKey[0] === "search" })
        toast({ heading: `${t("library.tankoubonCreated")}: ${finalName}`, icon: "success" })
      }
      onDismissed()
    } catch (err) {
      toast({
        heading: (isAddToExisting ? t("library.errorAddingToTankoubon") : t("library.errorCreatingTankoubon")) ?? undefined,
        text: String(err),
        icon: "error",
      })
    } finally {
      setCreating(false)
    }
  }

  async function handleIgnore() {
    try {
      await ignoreGroupSuggestion.mutateAsync({
        archive_ids: suggestion.archive_ids,
        existing_tankoubon_id: suggestion.existing_tankoubon_id,
      })
      onDismissed()
    } catch (err) {
      toast({ heading: t("library.errorIgnoringSuggestion") ?? undefined, text: String(err), icon: "error" })
    }
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between" }}>
        <button
          type="button"
          className="stdbtn"
          disabled={page === 0}
          onClick={onPrev}
          style={{ minWidth: 28, visibility: page === 0 ? "hidden" : undefined }}
        >
          <i className="fa fa-chevron-left" aria-hidden="true"></i>
        </button>
        <span style={{ fontSize: 12, opacity: 0.6 }}>
          {page + 1} / {total}
        </span>
        <button
          type="button"
          className="stdbtn"
          disabled={page === total - 1}
          onClick={onNext}
          style={{ minWidth: 28, visibility: page === total - 1 ? "hidden" : undefined }}
        >
          <i className="fa fa-chevron-right" aria-hidden="true"></i>
        </button>
      </div>

      <div className={isAddToExisting ? "ai-suggestion-card-add" : "ai-suggestion-card-new"} style={{ borderRadius: 8, overflow: "hidden", textAlign: "left" }}>
        <div
          className="ai-suggestion-card-header"
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "10px 16px",
          }}
        >
          <span style={{ fontSize: 13, fontWeight: 700 }}>
            {isAddToExisting
              ? `${t("library.suggestAddingTo")} "${suggestion.existing_tankoubon_id ? tankoubonNameById.get(suggestion.existing_tankoubon_id) ?? suggestion.existing_tankoubon_id : ""}"`
              : t("library.suggestedGroup")}{" "}
            {page + 1} · {suggestion.archive_ids.length} {t("library.archives")}
          </span>
          <button
            type="button"
            className="stdbtn ai-pill-btn"
            style={{ fontSize: 11 }}
            aria-expanded={showEvidence}
            onClick={() => setShowEvidence((v) => !v)}
          >
            <i className={`fa ${showEvidence ? "fa-chevron-up" : "fa-question-circle"}`} aria-hidden="true" />{" "}
            {t("library.groupEvidenceToggle")}
          </button>
        </div>

        <ReviewStatusLine
          review={review.data}
          pending={review.isFetching}
          total={suggestion.archive_ids.length}
          rejected={rejected.size}
          minimum={minimum}
        />

        {showEvidence && <EvidencePanel evidence={suggestion.evidence} titleById={titleById} />}

        <ul style={{ listStyle: "none", padding: "4px 16px", margin: 0 }}>
          {suggestion.archive_ids.map((id) => (
            <ArchiveChecklistItem
              key={id}
              className="ai-group-checklist-item"
              title={<MemberTitle id={id} title={titleById.get(id) ?? id} muted={rejected.has(id)} />}
              checked={checked.has(id)}
              onChange={(isChecked) => {
                setOverrides((prev) => new Map(prev).set(id, isChecked))
              }}
              trailing={
                <span style={{ display: "inline-flex", alignItems: "center", gap: 4, flexShrink: 0 }}>
                  {rejected.has(id) && (
                    <Tooltip
                      label={`${t("library.groupReviewRejected")}: ${rejected.get(id) ?? ""}`}
                      maxWidth={360}
                      zIndex={Z_OVERLAY_ABOVE_LEGACY_MODAL}
                    >
                      <span style={{ fontSize: 11, opacity: 0.7, whiteSpace: "nowrap" }}>
                        <i className="fa fa-robot" aria-hidden="true" /> {t("library.groupReviewRejectedShort")}
                      </span>
                    </Tooltip>
                  )}
                  <IconButton
                    variant="ghost-btn"
                    icon={<i className="fa fa-book-open" />}
                    size={22}
                    title={t("common.read") ?? undefined}
                    onClick={() => window.open(routes.reader(id), "_blank", "noreferrer")}
                  />
                </span>
              }
            />
          ))}
        </ul>
      </div>

      <DrawerFooter>
        <div style={{ display: "flex", alignItems: "center", justifyContent: "space-between", gap: 12 }}>
          <div style={{ display: "flex", flexWrap: "wrap", gap: 4, maxWidth: "60%" }}>
            {Array.from({ length: total }, (_, i) => (
              <span
                key={i}
                onClick={() => onJump(i)}
                style={{
                  width: 6,
                  height: 6,
                  borderRadius: "50%",
                  background: i === page ? "rgba(128,128,128,0.7)" : "rgba(128,128,128,0.2)",
                  cursor: "pointer",
                  transition: "background 0.2s",
                  flexShrink: 0,
                }}
              />
            ))}
          </div>
          <div style={{ display: "flex", gap: 8, flexShrink: 0 }}>
            <input
              type="button"
              className="stdbtn ai-pill-btn"
              value={t("library.donTSuggestThisAgain") ?? undefined}
              disabled={ignoreGroupSuggestion.isPending}
              onClick={() => void handleIgnore()}
            />
            {!isAddToExisting && llmKeyStatus.data?.configured ? (
              <button
                type="button"
                className="stdbtn ai-checkable-btn"
                disabled={!canConfirm}
                onClick={() => void handleConfirm()}
              >
                <span style={{ flex: 1, textAlign: "center" }}>{t("library.createTankoubon")}</span>
                <Tooltip
                  label={t("library.autorenameAndReorderChaptersWith")}
                  wrapperStyle={{ display: "inline-flex", flexShrink: 0 }}
                  zIndex={Z_OVERLAY_ABOVE_LEGACY_MODAL}
                >
                  <span className="ai-checkable-btn-toggle" onClick={(e) => e.stopPropagation()}>
                    <input
                      type="checkbox"
                      checked={autoRename}
                      onChange={(e) => setAutoRename(e.target.checked)}
                    />
                    {/* Must follow the checkbox in DOM order for the `~` sibling selector. */}
                    <i className="fa fa-robot" aria-hidden="true" />
                    <span className="ai-checkable-btn-toggle-circle">
                      <i className="fa fa-check" aria-hidden="true" />
                    </span>
                  </span>
                </Tooltip>
              </button>
            ) : (
              <input
                type="button"
                className="stdbtn ai-pill-btn"
                value={(isAddToExisting ? t("library.addToTankoubon") : t("library.createTankoubon")) ?? undefined}
                disabled={!canConfirm}
                onClick={() => void handleConfirm()}
              />
            )}
          </div>
        </div>
      </DrawerFooter>
    </div>
  )
}

/** AI-smart-create-Tankoubon, as a right-side drawer: top-aligned, so the pager stays put however
 * tall the current card is. Its body mounts on each open, which is what requests fresh suggestions. */
export function AiSmartTankoubonDrawer({ open, onClose }: { open: boolean; onClose: () => void }) {
  const { t } = useTranslation()
  return (
    <Drawer open={open} onClose={onClose} title={t("library.aiSmartCreateTankoubon")} width={600} swipeToClose={false}>
      <SuggestionsBody onClose={onClose} />
    </Drawer>
  )
}

/** Skeleton while the request runs, then one suggested group at a time in a paginated card.
 * Creating a group removes it and clamps the page index. */
function SuggestionsBody({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()
  const archives = useArchives()
  const aiGroupSuggestions = useAiGroupSuggestions()
  const [suggestions, setSuggestions] = useState<AiGroupSuggestion[] | null>(null)
  const [page, setPage] = useState(0)
  const [showIgnored, setShowIgnored] = useState(false)

  const tankoubons = useTankoubons()
  const ignoredSuggestions = useIgnoredGroupSuggestions(true)

  const titleById = new Map<string, string>()
  for (const a of archives.data ?? []) {
    titleById.set(a.arcid, a.title)
  }
  const tankoubonNameById = new Map<string, string>()
  for (const tank of tankoubons.data?.result ?? []) {
    tankoubonNameById.set(tank.id, tank.name)
  }

  // A `let cancelled` local (not a ref) so StrictMode's double-invoke in dev doesn't leave the
  // second, actually-mounted instance permanently stuck on the skeleton.
  const { mutate: requestSuggestions } = aiGroupSuggestions
  useEffect(() => {
    let cancelled = false
    requestSuggestions(undefined, {
      onSuccess: (data) => {
        if (!cancelled) setSuggestions(data.suggestions)
      },
      onError: (err) => {
        if (cancelled) return
        onClose()
        toast({ heading: t("library.aiGroupingFailed") ?? undefined, text: String(err), icon: "error" })
      },
    })
    return () => {
      cancelled = true
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  useEffect(() => {
    if (!suggestions || suggestions.length <= 1 || showIgnored) return
    function onKeyDown(e: KeyboardEvent) {
      if ((e.target as HTMLElement)?.tagName === "INPUT") return
      if (e.key === "ArrowLeft") {
        setPage((p) => Math.max(0, p - 1))
      } else if (e.key === "ArrowRight") {
        setPage((p) => Math.min((suggestions?.length ?? 1) - 1, p + 1))
      }
    }
    document.addEventListener("keydown", onKeyDown)
    return () => document.removeEventListener("keydown", onKeyDown)
  }, [suggestions, showIgnored])

  const current = suggestions?.[page]
  // On touch, a sideways swipe anywhere in the body pages through suggestions (the drawer's own
  // swipe-to-close is off); the body fills the drawer so short cards leave no dead area.
  const count = suggestions?.length ?? 0
  const swipe = useHorizontalSwipe({
    onSwipeLeft: () => !showIgnored && setPage((p) => Math.min(count - 1, p + 1)),
    onSwipeRight: () => !showIgnored && setPage((p) => Math.max(0, p - 1)),
  })
  // Prefetch the next two reviews so paging forward rarely waits; the card reviews itself.
  useGroupSuggestionReview(suggestions?.[page + 1], !showIgnored)
  useGroupSuggestionReview(suggestions?.[page + 2], !showIgnored)

  return (
    <div {...swipe} style={{ ...swipe.style, display: "flex", flexDirection: "column", gap: 12, minHeight: "100%" }}>
        {aiGroupSuggestions.isPending || !suggestions ? (
          <AiSkeleton />
        ) : showIgnored ? (
          <IgnoredSuggestionsList
            ignoredSuggestions={ignoredSuggestions}
            titleById={titleById}
            tankoubonNameById={tankoubonNameById}
          />
        ) : suggestions.length === 0 ? (
          <div style={{ padding: "20px 0" }}>
            <i className="fas fa-3x fa-check-circle" aria-hidden="true" style={{ opacity: 0.5 }} />
            <div style={{ marginTop: 12 }}>{t("library.noGroupingSuggestionsNothing")}</div>
          </div>
        ) : current ? (
          <SuggestionCard
            key={current.suggestion_id}
            suggestion={current}
            page={page}
            total={suggestions.length}
            titleById={titleById}
            tankoubonNameById={tankoubonNameById}
            onPrev={() => setPage((p) => Math.max(0, p - 1))}
            onNext={() => setPage((p) => Math.min(suggestions.length - 1, p + 1))}
            onJump={setPage}
            onDismissed={() => {
              setSuggestions((prev) => (prev ?? []).filter((s) => s !== current))
              setPage((p) => Math.max(0, Math.min(p, suggestions.length - 2)))
            }}
          />
        ) : null}

        {ignoredSuggestions.data && ignoredSuggestions.data.ignored.length > 0 && (
          <label style={{ display: "flex", alignItems: "center", gap: 6, fontSize: 12, opacity: 0.8, alignSelf: "flex-start" }}>
            <input
              type="checkbox"
              checked={showIgnored}
              onChange={(e) => setShowIgnored(e.target.checked)}
              style={{ margin: 0 }}
            />
            {t("library.showIgnoredCombinations")} ({ignoredSuggestions.data.ignored.length})
          </label>
        )}
    </div>
  )
}

/** Flat (unpaginated) list of dismissed suggestions, each with its own "Un-ignore" button. */
function IgnoredSuggestionsList({
  ignoredSuggestions,
  titleById,
  tankoubonNameById,
}: {
  ignoredSuggestions: ReturnType<typeof useIgnoredGroupSuggestions>
  titleById: Map<string, string>
  tankoubonNameById: Map<string, string>
}) {
  const { t } = useTranslation()
  const unignoreGroupSuggestion = useUnignoreGroupSuggestion()

  if (ignoredSuggestions.isPending) return <AiSkeleton />

  const entries = ignoredSuggestions.data?.ignored ?? []
  if (entries.length === 0) {
    return (
      <div style={{ padding: "20px 0" }}>
        <div>{t("library.noIgnoredCombinations")}</div>
      </div>
    )
  }

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 8, maxHeight: 360, overflowY: "auto" }}>
      {entries.map((entry) => {
        const key = `${entry.existing_tankoubon_id ?? ""}:${entry.archive_ids.join(",")}`
        return (
          <div
            key={key}
            style={{
              border: "1px solid rgba(128,128,128,0.3)",
              borderRadius: 8,
              padding: "10px 16px",
              textAlign: "left",
              display: "flex",
              alignItems: "center",
              justifyContent: "space-between",
              gap: 12,
            }}
          >
            <div style={{ fontSize: 12 }}>
              {entry.existing_tankoubon_id && (
                <div style={{ fontWeight: 700, marginBottom: 4 }}>
                  {t("library.addTo")} "{tankoubonNameById.get(entry.existing_tankoubon_id) ?? entry.existing_tankoubon_id}"
                </div>
              )}
              {entry.archive_ids.map((id) => (
                <div key={id}>{titleById.get(id) ?? id}</div>
              ))}
            </div>
            <input
              type="button"
              className="stdbtn"
              value={t("library.unignore") ?? undefined}
              disabled={unignoreGroupSuggestion.isPending}
              onClick={() =>
                void unignoreGroupSuggestion.mutateAsync({
                  archive_ids: entry.archive_ids,
                  existing_tankoubon_id: entry.existing_tankoubon_id,
                })
              }
            />
          </div>
        )
      })}
    </div>
  )
}
