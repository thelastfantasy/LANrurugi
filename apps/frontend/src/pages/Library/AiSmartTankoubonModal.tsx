import { useQueryClient } from "@tanstack/react-query"
import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import { sendJson } from "@/api/client"
import {
  type AiGroupSuggestion,
  useAiGroupSuggestions,
  useAiRenameTankoubon,
  useArchives,
  useCreateTankoubon,
  useIgnoredGroupSuggestions,
  useIgnoreGroupSuggestion,
  useLlmKeyStatus,
  useTankoubons,
  useUnignoreGroupSuggestion,
} from "@/api/hooks"
import { AiSkeleton, ArchiveChecklistItem, Modal, Tooltip } from "@/components/Display"
import { Z_OVERLAY_ABOVE_LEGACY_MODAL } from "@/theme"
import { toast } from "@/toast"

/** One AI-suggested group's card body — member archives start all checked (the suggestion IS the
 * default), user can uncheck any that don't belong before confirming. A group with fewer than 2
 * archives still checked can't become a real Tankoubon (there'd be nothing to group), so "Create
 * Tankoubon" is disabled in that case rather than silently no-op-ing or erroring server-side. */
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
  const [checked, setChecked] = useState<Set<string>>(() => new Set(suggestion.archive_ids))
  const [creating, setCreating] = useState(false)
  // Only meaningful for the "new Tankoubon" branch (not "add to existing" — renaming/reordering
  // an existing Tankoubon that already has its own name/chapter order the user chose isn't the
  // same one-click convenience this is for), and only offered at all once an LLM key is
  // configured (`useAiRenameTankoubon` needs one — same gate `TankoubonEdit.tsx`'s own AI button
  // uses). Defaults on when available: the checkbox exists specifically so this convenience is
  // opt-OUT, not opt-in, for the common case of creating a Tankoubon from an AI grouping
  // suggestion in the first place.
  const [autoRename, setAutoRename] = useState(true)

  const selectedIds = suggestion.archive_ids.filter((id) => checked.has(id))
  const isAddToExisting = !!suggestion.existing_tankoubon_id
  // A brand-new Tankoubon needs at least 2 members (there'd be nothing to group with just one),
  // but adding to an ALREADY-existing Tankoubon is meaningful with just 1 new member — the
  // Tankoubon itself already supplies the "at least 2 total" the new-group case needs.
  const canConfirm = selectedIds.length >= (isAddToExisting ? 1 : 2) && !creating

  async function handleConfirm() {
    setCreating(true)
    try {
      if (isAddToExisting && suggestion.existing_tankoubon_id) {
        const tankId = suggestion.existing_tankoubon_id
        // No bulk "add N archives" endpoint — `useAddToTankoubon`'s own `PUT
        // /tankoubons/{id}/{archiveId}` only takes one at a time (see that hook's own docs), so
        // this suggestion's new members are added sequentially rather than in parallel: parallel
        // PUTs racing against the same Tankoubon's archive-list read-modify-write could drop a
        // member if two responses interleave.
        for (const archiveId of selectedIds) {
          await sendJson("PUT", `/tankoubons/${tankId}/${archiveId}`)
        }
        void queryClient.invalidateQueries({ queryKey: ["tankoubon", tankId] })
        void queryClient.invalidateQueries({ queryKey: ["tankoubons"] })
        void queryClient.invalidateQueries({ queryKey: ["archives"] })
        void queryClient.invalidateQueries({ predicate: (query) => query.queryKey[0] === "search" })
        toast({ heading: t("library.addedToTankoubon") ?? undefined, icon: "success" })
      } else {
        // Two-step, same shape TankoubonEdit's own create-then-populate flow would need to use
        // (there's no single "create with initial members" endpoint) — `useCreateTankoubon` only
        // takes a `name`, so the new Tankoubon's id isn't known until that first call resolves.
        const name = selectedIds
          .map((id) => titleById.get(id))
          .find((title) => !!title) ?? t("library.newTankoubon") ?? "New Tankoubon"
        const { tankoubon_id } = await createTankoubon.mutateAsync(name)
        await sendJson("PUT", `/tankoubons/${tankoubon_id}`, { archives: selectedIds })

        let finalName = name
        if (autoRename && llmKeyStatus.data?.configured) {
          // One-click convenience: run the same AI-rename flow TankoubonEdit.tsx's own "AI
          // Suggestions" button offers, auto-applying its FIRST suggestion (see this component's
          // own docs on why suggestions[0] rather than presenting a picker) — same mapping
          // TankoubonEdit.tsx's `onApply` uses (`original_index` matches `original_member_names`'
          // own 1-based `index` field, not the array position). Best-effort: a rename failure
          // (e.g. no LLM credit left) must not roll back the Tankoubon that was already
          // successfully created — the user still gets a real Tankoubon, just without the rename,
          // same as if they'd left the checkbox unchecked.
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
              // `name`/`chapter_names` must be nested under `metadata` — `UpdateTankoubonBody`
              // (backend) only has top-level `archives`/`metadata` fields; a top-level `name` or
              // `chapter_names` is silently dropped by serde (unknown field, not an error),
              // confirmed live: an earlier version of this call sent them flat and the PUT
              // returned success:1 while Redis kept the Tankoubon's original name/empty chapter
              // list unchanged. Same shape TankoubonEdit.tsx's own `handleSave` uses.
              await sendJson("PUT", `/tankoubons/${tankoubon_id}`, {
                metadata: { name: finalName, chapter_names: chapterNames },
                // Only reorder if every member was accounted for — a partial mapping (the AI
                // response missing an original_index) silently dropping an archive from the
                // Tankoubon would be worse than just keeping the original creation order.
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

        // `useCreateTankoubon`'s own `onSuccess` already invalidated `["tankoubons"]` right after
        // the create call above resolved — before this archives PUT had even run — so that fetch
        // would've grabbed the tankoubon before its members were attached. Invalidate again now
        // that both steps are done. Also invalidate every `['search', ...]` query, not just
        // `['archives']` — the Library page's own main grid goes through `useSearch` (query key
        // `['search', options]`, one per distinct filter/sort combination), not `['archives']`,
        // same bug/fix already documented on `useSetArchiveProgress` — otherwise the grid still
        // shows the just-grouped archives as loose, ungrouped entries until an unrelated refetch.
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
      {/* Header with navigation — mirrors TankoubonEdit.tsx's BookPages layout */}
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

      {/* `.ai-suggestion-card-add`/`-new` give the whole card a distinct border/header tint per
       * suggestion kind (blue for "add to an existing Tankoubon", green for "create a new one" —
       * see each theme file's own rule pair for the actual colors) so a user paging through a mix
       * of both can tell which kind they're looking at from the card's own framing alone, not just
       * by reading the header text closely. Real classes, not inline hex values, per this
       * project's own custom-colors-must-be-theme-aware convention. */}
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
          <span style={{ fontSize: 10, opacity: 0.4 }}>
            <i className="fa fa-robot" aria-hidden="true" />
          </span>
        </div>

        <ul style={{ listStyle: "none", padding: "4px 16px", margin: 0, maxHeight: 260, overflowY: "auto" }}>
          {suggestion.archive_ids.map((id) => (
            <ArchiveChecklistItem
              key={id}
              className="ai-group-checklist-item"
              // Hover-thumbnail tooltip, same component/settings as TankoubonEdit.tsx's own
              // ArchiveTitle (`anchor="cursor"` follows the pointer since these rows can be tall
              // enough to wrap across two lines; `maxWidth={480}` caps the bubble itself, while
              // the `<img>`'s own `maxHeight`/`maxWidth: "100%"` is what actually keeps a
              // portrait-oriented cover from overflowing that cap — the two work together, not
              // redundantly: maxWidth bounds the box, maxHeight+100% bounds the image inside it).
              // `zIndex={Z_OVERLAY_ABOVE_LEGACY_MODAL}` — this tooltip's trigger lives inside
              // Modal.tsx, whose own hardcoded `zIndex: 9001` otherwise wins the stacking fight
              // against Tooltip's default `Z_OVERLAY_TOOLTIP` (1100), rendering the bubble behind
              // the modal instead of on top of it (confirmed live).
              title={
                <Tooltip
                  anchor="cursor"
                  wrapperStyle={{ display: "inline" }}
                  maxWidth={480}
                  zIndex={Z_OVERLAY_ABOVE_LEGACY_MODAL}
                  label={
                    <img
                      src={`/api/archives/${id}/thumbnail?no_fallback=true`}
                      alt=""
                      style={{ maxHeight: 420, maxWidth: "100%", display: "block" }}
                    />
                  }
                >
                  <span>{titleById.get(id) ?? id}</span>
                </Tooltip>
              }
              checked={checked.has(id)}
              onChange={(isChecked) => {
                setChecked((prev) => {
                  const next = new Set(prev)
                  if (isChecked) next.add(id)
                  else next.delete(id)
                  return next
                })
              }}
            />
          ))}
        </ul>

        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "10px 16px",
            borderTop: "1px solid rgba(128,128,128,0.15)",
            background: "rgba(128,128,128,0.05)",
          }}
        >
          <div style={{ display: "flex", gap: 4 }}>
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
                }}
              />
            ))}
          </div>
          <div style={{ display: "flex", gap: 8 }}>
            <input
              type="button"
              className="stdbtn ai-pill-btn"
              value={t("library.donTSuggestThisAgain") ?? undefined}
              disabled={ignoreGroupSuggestion.isPending}
              onClick={() => void handleIgnore()}
            />
            {/* Checkable button: the "Create Tankoubon" action itself, plus an embedded checkbox
             * toggling whether that action also auto-applies an AI rename/reorder — clicking the
             * button's own label/background confirms with whatever the checkbox is currently set
             * to, while clicking the checkbox only flips the toggle without confirming (the
             * checkbox's own onClick stops propagation so it doesn't also fire the button's
             * onClick). Only shown at all once an LLM key is configured (no rename capability
             * without one) and only for the "new Tankoubon" branch (see `autoRename`'s own docs
             * for why "add to existing" doesn't get this). */}
            {!isAddToExisting && llmKeyStatus.data?.configured ? (
              // Same left-label/right-action-with-a-divider shape as ArchiveOverviewOverlayPanel's
              // own real category chip (`.gt.category-chip` — label span + `borderLeft:
              // "1px solid currentColor"` + a right-side action region), not a custom invention —
              // `currentColor` means the divider automatically matches this button's own
              // text/border color per-theme with no extra theme-file rules needed, unlike the
              // earlier circular-icon-overlay version's own bespoke per-theme colors.
              <button
                type="button"
                className="stdbtn ai-checkable-btn"
                disabled={!canConfirm}
                onClick={() => void handleConfirm()}
              >
                {/* `flex: 1` — this button inherits `.stdbtn`'s own `min-width: 150px`, wider than
                 * the label + toggle's own combined content width; without this the leftover
                 * space collects entirely to the right of the toggle (flex's default
                 * `justify-content: flex-start` packs both items to the left), making the toggle
                 * read as sitting in the middle of a wide empty region rather than pinned to the
                 * button's own right edge. Growing the label instead pushes the toggle all the way
                 * right, matching the category-chip layout this is modeled on. */}
                <span style={{ flex: 1, textAlign: "center" }}>{t("library.createTankoubon")}</span>
                {/* `stopPropagation` on the Tooltip's own wrapper — without it, hovering to read
                 * the tooltip is harmless, but a stray click landing on the wrapper's own padding
                 * (just outside the checkbox's actual hit area) would bubble up to the button and
                 * fire handleConfirm, same class of bug the checkbox's own controlled `onChange`
                 * relies on staying scoped to just the checkbox. */}
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
                    {/* Must come AFTER the checkbox in DOM order — the CSS `~` general-sibling
                     * selector that fades this out on `:checked` only matches later siblings. */}
                    <i className="fa fa-robot" aria-hidden="true" />
                    <span className="ai-checkable-btn-toggle-circle">
                      <i className="fa fa-check" aria-hidden="true" />
                    </span>
                  </span>
                </Tooltip>
              </button>
            ) : (
              // Same `.ai-pill-btn` shape as the checkable button above (and "Don't suggest this
              // again" beside it) even though this branch has no checkbox of its own — a plain
              // rectangular button here, switching in and out as suggestions page between "add to
              // existing" and "new group", read as a bug/inconsistency rather than a deliberate
              // feature difference (confirmed live via a real screenshot: the button visibly
              // changed shape between adjacent suggestion cards). Only the checkable-button
              // FUNCTIONALITY is intentionally new-Tankoubon-only (see `autoRename`'s own docs) —
              // the pill shape itself is just this modal's baseline button style now.
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
      </div>
    </div>
  )
}

/** Modal body for the Library page's AI-smart-create-Tankoubon button — skeleton while the
 * request runs (shared `AiSkeleton`, same "AI is thinking" treatment `TankoubonEdit.tsx` uses),
 * then one suggested group at a time in a paginated card (← N / M →, dot indicators), matching
 * `TankoubonEdit.tsx`'s own `BookPages` AI-suggestion layout rather than stacking every group
 * vertically — much easier to scan one at a time when there are several suggestions. Creating the
 * current page's group removes it from the list and the page index clamps to stay in range (e.g.
 * creating the last group steps back to the new last page instead of pointing past the end).
 *
 * This is a Tankoubon-creation feature (issue #74's "智能分组建议" — analyzes archives not yet in
 * any Tankoubon and suggests which ones likely belong to the same series, recommending a
 * Tankoubon per group), not a Category feature — it lives next to the Library page's own
 * "Select Archives" control (`SearchBar.tsx`), not on the Categories page, since it operates on
 * Tankoubon membership, an entity Categories.tsx has no authority over. */
export function AiSmartTankoubonModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()
  const archives = useArchives()
  const aiGroupSuggestions = useAiGroupSuggestions()
  const [suggestions, setSuggestions] = useState<AiGroupSuggestion[] | null>(null)
  const [page, setPage] = useState(0)
  // "Show ignored combinations" — default unchecked, per this feature's own design: the whole
  // point of "Don't suggest this again" is that a dismissed combination normally stays out of
  // sight, this checkbox is an explicit opt-in to review/undo past dismissals rather than the
  // default view.
  const [showIgnored, setShowIgnored] = useState(false)

  const tankoubons = useTankoubons()
  // Always enabled (not gated on `showIgnored`) — the checkbox itself needs to know the count
  // up front to decide whether it's worth showing at all (see that checkbox's own render logic
  // below), not just once the user has already opted to view the list.
  const ignoredSuggestions = useIgnoredGroupSuggestions(true)

  const titleById = new Map<string, string>()
  for (const a of archives.data ?? []) {
    titleById.set(a.arcid, a.title)
  }
  const tankoubonNameById = new Map<string, string>()
  for (const tank of tankoubons.data?.result ?? []) {
    tankoubonNameById.set(tank.id, tank.name)
  }

  // StrictMode double-invokes effects in dev (mount → cleanup → mount again). A plain
  // `useRef(false)` guard set to `true` inside the effect body survives that cleanup (refs aren't
  // reset by unmount), so it blocked the *second* mount's request entirely — but the second mount
  // is the instance that actually stays on screen, and the first mount's `onSuccess` closure
  // updates the *first* (already-unmounted) instance's state, which React silently drops. Net
  // effect: exactly one real request fires (looks correct in the network panel), but the update
  // never reaches the visible component — permanently stuck on the skeleton. Real fix: track
  // "did this specific mount's effect run" with a `let` local to the effect closure (naturally
  // scoped per invocation, unlike a ref) and explicitly ignore the response if a cleanup already
  // ran by the time it resolves — the standard React-recommended pattern for effects with an
  // async result, and the only version where the *second* mount (the one that stays mounted)
  // is the one whose request result actually lands.
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

  const current = suggestions?.[page]

  return (
    <Modal onClose={onClose}>
      <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
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
            // Forces a remount per suggestion — otherwise navigating pages reuses the same
            // component instance and its `checked` Set stays initialized to page 1's archive
            // ids forever, silently rendering every later page's checkboxes as unchecked.
            key={current.archive_ids.join(",")}
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

        {/* Nothing to review/undo when the ignored list is empty — showing an always-unchecked,
         * permanently-inert checkbox in that case is worse than just not rendering it at all. */}
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
    </Modal>
  )
}

/** "Show ignored combinations" view — every dismissed suggestion, each with its own "Un-ignore"
 * button restoring it to `useAiGroupSuggestions()`'s normal output on the next fetch. Flat list
 * (not paginated like the main suggestion cards) since this is a review/undo tool, not the primary
 * flow — a user checking this box is specifically looking for one past dismissal to reverse, not
 * paging through them one at a time. */
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
