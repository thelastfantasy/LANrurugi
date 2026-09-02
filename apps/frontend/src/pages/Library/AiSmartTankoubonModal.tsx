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
import { Modal, Tooltip } from "@/components/common-ui/Display"
import { IconButton } from "@/components/common-ui/Form"
import { AiSkeleton, ArchiveChecklistItem } from "@/components/Display"
import { routes } from "@/lib/routes"
import { Z_OVERLAY_ABOVE_LEGACY_MODAL } from "@/theme"
import { toast } from "@/toast"

/** One AI-suggested group's card — members start all checked; "Create Tankoubon" is disabled
 * below 2 checked (nothing to group). */
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
  const [autoRename, setAutoRename] = useState(true)

  const selectedIds = suggestion.archive_ids.filter((id) => checked.has(id))
  const isAddToExisting = !!suggestion.existing_tankoubon_id
  const canConfirm = selectedIds.length >= (isAddToExisting ? 1 : 2) && !creating

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
      } else {
        const name = selectedIds
          .map((id) => titleById.get(id))
          .find((title) => !!title) ?? t("library.newTankoubon") ?? "New Tankoubon"
        const { tankoubon_id } = await createTankoubon.mutateAsync(name)
        await sendJson("PUT", `/tankoubons/${tankoubon_id}`, { archives: selectedIds })

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
          <span style={{ fontSize: 10, opacity: 0.4 }}>
            <i className="fa fa-robot" aria-hidden="true" />
          </span>
        </div>

        <ul style={{ listStyle: "none", padding: "4px 16px", margin: 0, maxHeight: 260, overflowY: "auto" }}>
          {suggestion.archive_ids.map((id) => (
            <ArchiveChecklistItem
              key={id}
              className="ai-group-checklist-item"
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
              trailing={
                <IconButton
                  variant="ghost-btn"
                  icon={<i className="fa fa-book-open" />}
                  size={22}
                  title={t("common.read") ?? undefined}
                  onClick={() => window.open(routes.reader(id), "_blank", "noreferrer")}
                />
              }
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
      </div>
    </div>
  )
}

/** AI-smart-create-Tankoubon modal — skeleton while the request runs, then one suggested group
 * at a time in a paginated card. Creating a group removes it and clamps the page index. */
export function AiSmartTankoubonModal({ onClose }: { onClose: () => void }) {
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
