import { useQueryClient } from "@tanstack/react-query"
import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import { sendJson } from "@/api/client"
import { type AiGroupSuggestion, useAiGroupSuggestions, useArchives, useCreateTankoubon } from "@/api/hooks"
import { AiSkeleton, ArchiveChecklistItem, Modal } from "@/components/Display"
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
  onCreated,
  onPrev,
  onNext,
  onJump,
}: {
  suggestion: AiGroupSuggestion
  page: number
  total: number
  titleById: Map<string, string>
  onCreated: () => void
  onPrev: () => void
  onNext: () => void
  onJump: (page: number) => void
}) {
  const { t } = useTranslation()
  const createTankoubon = useCreateTankoubon()
  const queryClient = useQueryClient()
  const [checked, setChecked] = useState<Set<string>>(() => new Set(suggestion.archive_ids))
  const [creating, setCreating] = useState(false)

  const selectedIds = suggestion.archive_ids.filter((id) => checked.has(id))
  const canCreate = selectedIds.length >= 2 && !creating

  async function handleCreate() {
    setCreating(true)
    try {
      // Two-step, same shape TankoubonEdit's own create-then-populate flow would need to use
      // (there's no single "create with initial members" endpoint) — `useCreateTankoubon` only
      // takes a `name`, so the new Tankoubon's id isn't known until that first call resolves.
      const name = selectedIds
        .map((id) => titleById.get(id))
        .find((title) => !!title) ?? t("New Tankoubon") ?? "New Tankoubon"
      const { tankoubon_id } = await createTankoubon.mutateAsync(name)
      await sendJson("PUT", `/tankoubons/${tankoubon_id}`, { archives: selectedIds })
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
      // `heading`, not `text` — `text` is raw `dangerouslySetInnerHTML` (fine for the
      // developer-authored strings every other call site feeds it), but `name` here comes from a
      // real archive title, which a user controls via upload/rename and could contain HTML.
      // `heading` renders through plain JSX instead, so React escapes it automatically.
      toast({ heading: `${t("Tankoubon created!")}: ${name}`, icon: "success" })
      onCreated()
    } catch (err) {
      toast({ heading: t("Error creating tankoubon") ?? undefined, text: String(err), icon: "error" })
    } finally {
      setCreating(false)
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

      <div style={{ border: "1px solid rgba(128,128,128,0.3)", borderRadius: 8, overflow: "hidden", textAlign: "left" }}>
        <div
          style={{
            display: "flex",
            alignItems: "center",
            justifyContent: "space-between",
            padding: "10px 16px",
            background: "rgba(128,128,128,0.08)",
            borderBottom: "1px solid rgba(128,128,128,0.15)",
          }}
        >
          <span style={{ fontSize: 13, fontWeight: 700 }}>
            {t("Suggested Group")} {page + 1} · {suggestion.archive_ids.length} {t("archives")}
          </span>
          <span style={{ fontSize: 10, opacity: 0.4 }}>
            <i className="fa fa-robot" aria-hidden="true" />
          </span>
        </div>

        <ul style={{ listStyle: "none", padding: "8px 16px", margin: 0, maxHeight: 260, overflowY: "auto" }}>
          {suggestion.archive_ids.map((id) => (
            <ArchiveChecklistItem
              key={id}
              title={titleById.get(id) ?? id}
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
          <input
            type="button"
            className="stdbtn"
            value={t("Create Tankoubon") ?? undefined}
            disabled={!canCreate}
            onClick={() => void handleCreate()}
          />
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

  const titleById = new Map<string, string>()
  for (const a of archives.data ?? []) {
    titleById.set(a.arcid, a.title)
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
        toast({ heading: t("AI grouping failed") ?? undefined, text: String(err), icon: "error" })
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
      {aiGroupSuggestions.isPending || !suggestions ? (
        <AiSkeleton />
      ) : suggestions.length === 0 ? (
        <div style={{ padding: "20px 0" }}>
          <i className="fas fa-3x fa-check-circle" aria-hidden="true" style={{ opacity: 0.5 }} />
          <div style={{ marginTop: 12 }}>{t("No grouping suggestions — nothing in your library looks like an ungrouped series right now.")}</div>
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
          onPrev={() => setPage((p) => Math.max(0, p - 1))}
          onNext={() => setPage((p) => Math.min(suggestions.length - 1, p + 1))}
          onJump={setPage}
          onCreated={() => {
            setSuggestions((prev) => (prev ?? []).filter((s) => s !== current))
            setPage((p) => Math.max(0, Math.min(p, suggestions.length - 2)))
          }}
        />
      ) : null}
    </Modal>
  )
}
