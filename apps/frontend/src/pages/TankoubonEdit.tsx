import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate, useParams } from "react-router-dom"

import {
  type TankoubonAiRenameResponse,
  useAiRenameChapter,
  useAiRenameTankoubon,
  useDeleteTankoubon,
  useLlmKeyStatus,
  useSearch,
  useStats,
  useTankoubonFull,
  useUpdateTankoubon,
} from "@/api/hooks"
import type { TankoubonMetadata } from "@/api/types"
import { AiSkeleton, IconButton, IconButtonWithTooltip, Modal, PopupMenu, PopupMenuItem } from "@/components/Display"
import { SortableList } from "@/components/Display"
import { Tooltip } from "@/components/Display"
import { TagInput } from "@/components/Form"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { toast } from "@/toast"

type Suggestion = TankoubonAiRenameResponse["suggestions"][number]
type OriginalMember = TankoubonAiRenameResponse["original_member_names"][number]

function BookPages({
  suggestions,
  originalMembers,
  onApply,
  t,
}: {
  suggestions: Suggestion[]
  originalMembers: OriginalMember[]
  onApply: (sug: Suggestion) => void
  t: ReturnType<typeof useTranslation>["t"]
}) {
  const [page, setPage] = useState(0)
  const total = suggestions.length
  const sug = suggestions[page]
  if (!sug) return null

  // Display chapters sorted by AI's suggested order
  const sortedChapters = [...sug.chapters].sort((a, b) => a.sorted_index - b.sorted_index)

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
      {/* Book header with navigation */}
      <div
        style={{
          display: "flex",
          alignItems: "center",
          justifyContent: "space-between",
        }}
      >
        <button
          type="button"
          className="stdbtn"
          disabled={page === 0}
          onClick={() => setPage((p) => p - 1)}
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
          onClick={() => setPage((p) => p + 1)}
          style={{ minWidth: 28, visibility: page === total - 1 ? "hidden" : undefined }}
        >
          <i className="fa fa-chevron-right" aria-hidden="true"></i>
        </button>
      </div>

      {/* Book page card */}
      <div
        style={{
          border: "1px solid rgba(128,128,128,0.3)",
          borderRadius: 8,
          overflow: "hidden",
        }}
      >
        {/* Page top bar */}
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
            {t("AI Suggestions")} · {t("Suggestion")} {page + 1}
          </span>
          <span style={{ fontSize: 10, opacity: 0.4 }}>
            <i className="fa fa-robot" aria-hidden="true" />
          </span>
        </div>

        {/* Tank name */}
        <div style={{ padding: "14px 16px 8px" }}>
          <div style={{ fontSize: 10, opacity: 0.5, marginBottom: 4, textTransform: "uppercase", letterSpacing: "0.5px" }}>
            {t("Title")}
          </div>
          <div style={{ fontSize: 15, fontWeight: 700, lineHeight: 1.3 }}>
            {sug.tank_name}
          </div>
        </div>

        {/* Chapter mappings — sorted by AI, left=original right=suggested */}
        <div style={{ padding: "6px 16px 12px" }}>
          <div style={{ fontSize: 10, opacity: 0.5, marginBottom: 6, textTransform: "uppercase", letterSpacing: "0.5px" }}>
            {t("Chapters")} · {t("sorted order")}
          </div>
          {sortedChapters.map((ch, i) => {
            const mem = originalMembers[ch.original_index - 1]
            return (
            <div
              key={ch.original_index}
              style={{
                display: "flex",
                alignItems: "center",
                gap: 8,
                padding: "4px 0",
                fontSize: 12,
                borderBottom: i < sortedChapters.length - 1 ? "1px solid rgba(128,128,128,0.1)" : undefined,
              }}
            >
              <span style={{ fontSize: 10, opacity: 0.35, minWidth: 16, flexShrink: 0 }}>
                {ch.sorted_index}
              </span>
              {/* Original (left) — struck through, multiline */}
              <span
                style={{
                  flex: 1,
                  opacity: 0.35,
                  textDecoration: "line-through",
                  wordBreak: "break-word",
                  lineHeight: 1.4,
                }}
              >
                {mem?.title ?? "—"}
              </span>
              <i className="fa fa-arrow-right" aria-hidden="true" style={{ opacity: 0.25, flexShrink: 0, fontSize: 10, alignSelf: "flex-start", marginTop: 3 }}></i>
              {/* Suggested (right) — multiline */}
              <span
                style={{
                  flex: 1,
                  fontWeight: 500,
                  wordBreak: "break-word",
                  lineHeight: 1.4,
                }}
              >
                {ch.name}
              </span>
            </div>
            )
          })}
        </div>

        {/* Page footer with Apply */}
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
            {suggestions.map((_, i) => (
              <span
                key={i}
                onClick={() => setPage(i)}
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
            value={t("Apply") ?? undefined}
            onClick={() => onApply(sug)}
          />
        </div>
      </div>
    </div>
  )
}

/** Archive title with hover-thumbnail tooltip — title comes from the tankoubon's own
 * `full_data` (no per-archive fetch needed), matching real legacy's own `edit.html.tt2`
 * (`is_tank` branch, line 147: `archive.title` with `onmouseover="IndexTable.buildImageTooltip(this)"`). */
function ArchiveTitle({ archiveId, title }: { archiveId: string; title: string }) {
  return (
    <Tooltip
      anchor="cursor"
      wrapperStyle={{ display: "inline" }}
      maxWidth={480}
      label={
        <img
          src={`/api/archives/${archiveId}/thumbnail?no_fallback=true`}
          alt=""
          style={{ maxHeight: 420, maxWidth: "100%", display: "block" }}
        />
      }
    >
      <span>{title}</span>
    </Tooltip>
  )
}

export function TankoubonEdit() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { tankId = "" } = useParams<{ tankId: string }>()
  const tankoubonFull = useTankoubonFull(tankId)
  const tankoubon = tankoubonFull.data?.result
  // Title lookup built from full_data + manually-added archives. Ref so additions
  // persist across re-renders (new Map on every render would lose them).
  const titleById = useRef(new Map<string, string>())
  // Populate on first data load — useRef only uses the initial value once.
  useEffect(() => {
    if (tankoubon) {
      for (const a of tankoubon.full_data ?? []) {
        titleById.current.set(a.arcid, a.title)
      }
    }
  }, [tankoubon])

  if (tankoubonFull.isLoading) {
    return (
      <div className="ido" style={{ textAlign: "center", maxWidth: 800, margin: "10px auto", color: "var(--theme-muted)" }}>
        {t("Loading library…")}
      </div>
    )
  }

  if (tankoubonFull.isError || !tankoubon) {
    return (
      <div className="ido" style={{ textAlign: "center", maxWidth: 800, margin: "10px auto", display: "flex", flexDirection: "column", gap: 12, alignItems: "center" }}>
        <p className="text-red-500">
          {t("Failed to load archives: {{error}}", { error: String(tankoubonFull.error) })}
        </p>
        <input
          className="stdbtn"
          type="button"
          value={t("Return to Library") ?? undefined}
          onClick={() => navigate(routes.library())}
        />
      </div>
    )
  }

  // Keyed by tankId so navigating between two different tankoubons' edit pages remounts this
  // form with fresh initial state, rather than needing an effect to re-sync it.
  return <TankoubonForm key={tankId} tankId={tankId} tankoubon={tankoubon} titleById={titleById.current} />
}

function TankoubonForm({ tankId, tankoubon, titleById }: { tankId: string; tankoubon: TankoubonMetadata; titleById: Map<string, string> }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  // Matches this page's own real heading text below ("Editing %1 (Tankoubon)") — no legacy
  // equivalent to cross-check against (Tankoubon editing is additive to this rewrite), so this
  // just keeps the tab title and the on-page heading in sync with each other.
  useDocumentTitle(t("Editing %1 (Tankoubon)").replace("%1", tankoubon.name))
  const updateTankoubon = useUpdateTankoubon(tankId)
  const deleteTankoubon = useDeleteTankoubon()
  const stats = useStats(2)
  // Same source/shape as `Edit.tsx`'s own `tagSuggestions` (every tag used at least twice across
  // the library) — this page's Tags field now uses the same `TagInput` chip editor as the
  // archive edit page, not a plain textarea.
  const tagSuggestions = (stats.data ?? []).map((s) => (s.namespace ? `${s.namespace}:${s.text}` : s.text))

  const [name, setName] = useState(tankoubon.name)
  const [summary, setSummary] = useState(tankoubon.summary)
  const [tags, setTags] = useState(tankoubon.tags)
  const [archives, setArchives] = useState(tankoubon.archives)
  const [newArchiveId, setNewArchiveId] = useState("")
  const [archiveSearchOpen, setArchiveSearchOpen] = useState(false)
  const aiRename = useAiRenameTankoubon()
  const aiRenameChapter = useAiRenameChapter()
  const [chapterAiLoading, setChapterAiLoading] = useState<number | null>(null)
  const llmStatus = useLlmKeyStatus()
  const hasLlmKey = llmStatus.data?.configured ?? false
  const [aiOverlayOpen, setAiOverlayOpen] = useState(false)
  const [aiSuggestions, setAiSuggestions] = useState<TankoubonAiRenameResponse | null>(null)
  const initChapters: Record<string, string> = {}
  for (const c of tankoubon.chapter_names ?? []) {
    initChapters[c.id] = c.name
  }
  const [chapterNames, setChapterNames] = useState<Record<string, string>>(initChapters)

  // Debounced so the title-search dropdown below doesn't fire one request per keystroke —
  // additive on top of the raw-ID input, which still works unchanged (see `addArchiveId`).
  const [debouncedQuery, setDebouncedQuery] = useState("")
  useEffect(() => {
    const timeout = setTimeout(() => setDebouncedQuery(newArchiveId.trim()), 250)
    return () => clearTimeout(timeout)
  }, [newArchiveId])
  const archiveSearch = useSearch({ filter: debouncedQuery, enabled: debouncedQuery.length > 0 })
  // Excludes archives already in this Tankoubon, and any synthetic Tankoubon-aggregate rows the
  // search endpoint can return (`archive_count !== null`, matching `ArchiveMetadata`'s own doc
  // comment) — a Tankoubon can't usefully contain another Tankoubon. Capped at 15, same as the
  // Library search bar's own tag-autocomplete dropdown (`Library/index.tsx`'s `tagSuggestions`)
  // — the underlying `/search` endpoint's own page size (100) is far too many to usefully scroll
  // through in a suggestion dropdown.
  const archiveSearchResults = (archiveSearch.data?.data ?? [])
    .filter((a) => a.archive_count === null && !archives.includes(a.arcid))
    .slice(0, 15)

  // Legacy's own `Edit.saveMetadata` (`edit.js`) shows a "Metadata saved!" toast on every
  // successful save via `Server.callAPIBody`'s built-in success-message handling — this port's
  // `updateTankoubon` doesn't have that generic per-call toasting, so it's shown explicitly here.
  async function handleSave() {
    await updateTankoubon.mutateAsync({
      archives,
      metadata: { name, summary, tags, chapter_names: Object.entries(chapterNames).map(([id, name]) => ({ id, name })) },
    })
    toast({ heading: t("Metadata saved!") ?? undefined, icon: "success" })
  }

  // Real legacy's own `edit.html.tt2` (`is_tank` branch) reorders this list via drag (`Sortable.
  // min.js`, `.drag-handle`), not up/down buttons — reusing `SortableList` (already used by
  // `SortablePluginGroup.tsx`) rather than the earlier button-based reorder this page started
  // with. The dropped order becomes the Tankoubon's own volume order (`archives`, order-
  // significant), persisted the same way `moveArchive` used to.
  function handleReorder(next: string[]) {
    setArchives(next)
  }

  function removeArchive(id: string) {
    setArchives((prev) => prev.filter((a) => a !== id))
  }

  async function handleDelete() {
    await deleteTankoubon.mutateAsync(tankId)
    navigate(routes.library())
  }

  // Shared by the raw-ID "Add" button and picking a row from the title-search dropdown below —
  // same mutation either way, just a different source for the ID. `title` is used for newly-added
  // archives that aren't in the initial `full_data` (otherwise the row shows the raw hash).
  function addArchiveId(archiveId: string, title?: string) {
    setArchives((prev) => [...prev, archiveId])
    if (title) titleById.set(archiveId, title)
    // Auto-scroll so the newly-added row stays visible at the bottom
    setTimeout(() => {
      window.scrollBy({ top: 52, behavior: "smooth" })
    }, 50)
  }

  function handleAddArchive() {
    const id = newArchiveId.trim()
    if (!id) return
    addArchiveId(id)
  }

  return (
    <div className="ido" style={{ textAlign: "center", maxWidth: 800, margin: "10px auto" }}>
      <h2 className="ih" style={{ textAlign: "center" }}>
        {t("Editing %1 (Tankoubon)").replace("%1", tankoubon.name)}
      </h2>

      <form
        autoComplete="off"
        style={{ width: "98%", maxWidth: 700, margin: "0 auto", fontSize: "8pt" }}
        onSubmit={(e) => e.preventDefault()}
      >
        <div style={{ display: "flex", flexDirection: "column", gap: 10, textAlign: "left" }}>
          <div style={{ display: "grid", gridTemplateColumns: "120px 1fr", alignItems: "center", gap: 6 }}>
            <span>{t("Title:")}</span>
            <input
              className="stdinput"
              type="text"
              style={{ width: "100%", maxWidth: "none" }}
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "120px 1fr", alignItems: "start", gap: 6 }}>
            <span>{t("Summary:")}</span>
            <textarea
              className="stdinput"
              style={{ width: "100%", maxWidth: "none", minHeight: 72, boxSizing: "border-box" }}
              value={summary}
              onChange={(e) => setSummary(e.target.value)}
            />
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "120px 1fr", alignItems: "start", gap: 6 }}>
            <span>
              {t("Tags")} <span style={{ fontSize: "6pt" }}>{t("(separated by hyphens, i.e : tag1, tag2)")}</span> :
            </span>
            <TagInput value={tags} onChange={setTags} suggestions={tagSuggestions} />
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "120px 1fr", alignItems: "start", gap: 6 }}>
            <span>{t("Archives:")}</span>
            <div style={{ display: "flex", flexDirection: "column", gap: 4, width: "100%" }}>
            {hasLlmKey && (
              <IconButtonWithTooltip
                icon="fa fa-robot"
                title={t("AI Smart Rename")}
                description={t("Analyze the archive list and suggest a tankoubon title and chapter names for each volume")}
                style={{ alignSelf: "flex-start" }}
                disabled={aiRename.isPending}
                onClick={() => {
                  setAiOverlayOpen(true)
                  setAiSuggestions(null)
                  aiRename.mutate(tankId, {
                    onSuccess: (data) => { setAiSuggestions(data) },
                    onError: (err) => {
                      setAiOverlayOpen(false)
                      toast({ heading: t("AI rename failed") ?? undefined, text: String(err), icon: "error" })
                    },
                  })
                }}
              />
            )}
            {/* `SortableList`'s own `DndContext`/`SortableContext` render transparently (no DOM
                wrapper of their own), so without this wrapping div each row's bare element would
                land as a direct child of this `grid` container and get independently
                auto-placed instead of staying confined to this one column — a real observed bug
                (the two rows ended up at unrelated x-positions instead of stacked in column 2). */}
            <div style={{ width: "100%" }}>
              <SortableList
                items={archives}
                getId={(archiveId) => archiveId}
                onReorder={handleReorder}
                renderItem={(archiveId, dragHandleProps) => {
                  const twoRow = hasLlmKey
                  const memberIndex = tankoubon.archives.indexOf(archiveId) + 1
                  return (
                  <div
                    style={{
                      display: "grid",
                      gridTemplateColumns: "auto 1fr auto",
                      gridTemplateRows: twoRow ? "auto auto" : "auto",
                      gap: "2px 8px",
                      alignItems: "center",
                      padding: "2px 0 4px 0",
                      borderBottom: "1px solid rgba(128,128,128,0.12)",
                    }}
                  >
                    {/* Drag handle — column 1, spans both rows when two-row */}
                    <span
                      {...dragHandleProps.attributes}
                      {...dragHandleProps.listeners}
                      style={{
                        gridColumn: "1",
                        gridRow: twoRow ? "1 / 3" : undefined,
                        flexShrink: 0,
                        display: "flex",
                        cursor: dragHandleProps.isDragging ? "grabbing" : "grab",
                        touchAction: "none",
                        opacity: 0.6,
                      }}
                    >
                      <i className="fa fa-grip-vertical" aria-hidden="true"></i>
                    </span>

                    {/* Archive title — row 1, column 2 */}
                    <span
                      style={{
                        gridColumn: "2",
                        overflow: "hidden",
                        textOverflow: "ellipsis",
                        whiteSpace: "nowrap",
                      }}
                    >
                      <ArchiveTitle archiveId={archiveId} title={titleById.get(archiveId) ?? archiveId} />
                    </span>

                    {/* Edit + ✕ — column 3, spans both rows */}
                    <div
                      style={{
                        gridColumn: "3",
                        gridRow: twoRow ? "1 / 3" : undefined,
                        display: "flex",
                        gap: 6,
                        flexShrink: 0,
                        alignItems: "center",
                      }}
                    >
                      <button
                        type="button"
                        className="stdbtn"
                        onClick={() => navigate(routes.edit(archiveId))}
                        style={{ minWidth: 32 }}
                      >
                        {t("Edit")}
                      </button>
                      <IconButton
                        icon="fa fa-times"
                        onClick={() => removeArchive(archiveId)}
                        title={t("Remove from Tankoubon") ?? undefined}
                      />
                    </div>

                    {/* Chapter input + AI Rename — row 2, column 2 */}
                    {twoRow && (
                      <div style={{ gridColumn: "2", display: "flex", gap: 4, alignItems: "center" }}>
                        {chapterAiLoading === memberIndex ? (
                          <div
                            style={{
                              flex: 1,
                              height: 18,
                              borderRadius: 4,
                              background: "repeating-linear-gradient(-45deg, rgba(128,128,128,0.15), rgba(128,128,128,0.15) 4px, rgba(128,128,128,0.06) 4px, rgba(128,128,128,0.06) 8px)",
                              animation: "ai-shimmer 1s linear infinite",
                              backgroundSize: "200% 100%",
                            }}
                          />
                        ) : (
                          <input
                            className="stdinput"
                            type="text"
                            placeholder={t("Chapter name") ?? undefined}
                            value={chapterNames[archiveId] ?? ""}
                            onChange={(e) => setChapterNames((prev) => ({ ...prev, [archiveId]: e.target.value }))}
                            style={{ flex: 1, maxWidth: "none", height: 18, fontSize: "7pt" }}
                          />
                        )}
                        <IconButtonWithTooltip
                          icon="fa fa-robot"
                          title={t("AI Chapter Name")}
                          description={t("Suggest a chapter title based on series context and volume numbers")}
                          size="small"
                          style={{ fontSize: "7pt" }}
                          disabled={chapterAiLoading !== null}
                          onClick={() => {
                            setChapterAiLoading(memberIndex)
                            aiRenameChapter.mutate(
                              { tankId, archiveIndex: memberIndex },
                              {
                                onSuccess: (data) => {
                                  setChapterNames((prev) => ({ ...prev, [archiveId]: data.name }))
                                  setChapterAiLoading(null)
                                  const prevName = chapterNames[archiveId] || tankoubon.chapter_names?.find((c) => c.id === archiveId)?.name
                                  if (data.name === prevName) {
                                    toast({ heading: t("Chapter name unchanged"), text: data.name, icon: "info" })
                                  } else {
                                    toast({ heading: t("Chapter name suggested"), text: data.name, icon: "success" })
                                  }
                                },
                                onError: (err) => {
                                  setChapterAiLoading(null)
                                  toast({ heading: t("AI rename failed") ?? undefined, text: String(err), icon: "error" })
                                },
                              },
                            )
                          }}
                        />
                      </div>
                    )}
                  </div>
                  )
                }}
              />
            </div>
            </div>
          </div>

          {/* AI Suggestions overlay — skeleton while loading, book-page cards on success */}
          {aiOverlayOpen && (
            <Modal onClose={() => setAiOverlayOpen(false)} textAlign="left">
              {aiRename.isPending ? (
                <AiSkeleton />
              ) : aiSuggestions ? (
                <BookPages
                  suggestions={aiSuggestions.suggestions}
                  originalMembers={aiSuggestions.original_member_names}
                  onApply={(sug) => {
                    setName(sug.tank_name)
                    const next: Record<string, string> = {}
                    const sorted = [...sug.chapters].sort((a, b) => a.sorted_index - b.sorted_index)
                    const reordered: string[] = []
                    for (const ch of sorted) {
                      // original_index matches the `index` field on original_member_names (1-based)
                      const mem = aiSuggestions.original_member_names.find((m) => m.index === ch.original_index)
                      if (mem) {
                        if (ch.name) next[mem.id] = ch.name
                        reordered.push(mem.id)
                      }
                    }
                    setChapterNames((prev) => ({ ...prev, ...next }))
                    // Only update order if all members are accounted for
                    if (reordered.length === archives.length) {
                      setArchives(reordered)
                    }
                    setAiOverlayOpen(false)
                  }}
                  t={t}
                />
              ) : null}
            </Modal>
          )}

          {/* Inline keyframe for the per-chapter shimmer placeholder above (line ~543) — the
              overlay's own "AI is thinking" shimmer/pulse styles now live inside the shared
              `AiSkeleton` component instead of being duplicated here. */}
          <style>{`
            @keyframes ai-shimmer {
              0%   { background-position: -200% 0; }
              100% { background-position: 200% 0; }
            }
          `}</style>

          <div style={{ display: "grid", gridTemplateColumns: "120px 1fr", alignItems: "center", gap: 6 }}>
            <span>{t("Add Archive to Tankoubon:")}</span>
            <div style={{ display: "flex", gap: 6 }}>
              {/* The raw-ID paste-and-click-Add flow below is unchanged; this additionally
                  live-searches by title as the user types (debounced, `archiveSearch` above) and
                  offers a click-to-add dropdown with a thumbnail preview per match — for anyone
                  who doesn't already have the 40-char ID copied. */}
              <span style={{ position: "relative", width: "100%" }}>
                <input
                  className="stdinput"
                  type="text"
                  // The surrounding `<form autoComplete="off">` doesn't reliably stop Chrome's
                  // own value-history autofill dropdown on a plain text input — it needs
                  // `autoComplete="off"` set directly on the field itself. That native dropdown
                  // is browser-chrome UI, not page DOM, so it can never get a hover-thumbnail
                  // like the custom `PopupMenu` search dropdown below can; turning it off avoids
                  // the two dropdowns visually fighting each other instead.
                  autoComplete="off"
                  // `.stdinput`'s own legacy height (18px) is 3px shorter than `.stdbtn`'s
                  // (21px, border-box, both already `boxSizing: 'border-box'` from the theme
                  // CSS) — matches the button next to it exactly rather than sitting visibly
                  // shorter.
                  style={{ width: "100%", maxWidth: "none", height: 21, boxSizing: "border-box" }}
                  value={newArchiveId}
                  onChange={(e) => {
                    setNewArchiveId(e.target.value)
                    // Also needed here, not just `onFocus` below — `addArchiveId` closes the
                    // dropdown after a click-to-add, but focus never actually leaves the input
                    // (the dropdown item's `onMouseDown` calls `preventDefault()` specifically to
                    // stop that), so `onFocus` never fires again on subsequent keystrokes. A real,
                    // confirmed bug: after adding one archive via the dropdown, typing a new query
                    // right after produced zero visible results despite the search request itself
                    // firing and returning real matches (`archiveSearchOpen` just stayed `false`).
                    setArchiveSearchOpen(true)
                  }}
                  onFocus={() => setArchiveSearchOpen(true)}
                  onBlur={() => setTimeout(() => setArchiveSearchOpen(false), 150)}
                  onKeyDown={(e) => {
                    if (e.key === "Escape") setArchiveSearchOpen(false)
                  }}
                  placeholder={t("Archive ID (40-character long)") ?? undefined}
                />
                {archiveSearchOpen && archiveSearchResults.length > 0 && (
                  <PopupMenu
                    portal={false}
                    // `PopupMenu`'s own `m-[.3em]` class (a deliberate small gap for its usual
                    // context-menu use) otherwise offsets this flush-to-the-input dropdown a few
                    // px right/down from the input's real edge — a real, confirmed
                    // `getBoundingClientRect()` mismatch (input left 138.67 vs. menu left
                    // 140.86 at a 375px-wide viewport), not just a screenshot artifact.
                    // `boxSizing: 'border-box'` on top of that: `PopupMenu`'s `<ul>` has no
                    // box-sizing of its own (content-box default), so its `border: 1px solid`
                    // was rendering 1px *outside* the `minWidth: 100%` content box on each
                    // side — 2px wider overall than the input than it should be.
                    // `left: 1` (not `0`): `.stdinput`'s own legacy CSS margin is
                    // `4px 1px 0px` — a real 1px left margin that shifts the input's own border
                    // box 1px right of this wrapping `<span>`'s left edge (which has no margin
                    // of its own), confirmed via `getBoundingClientRect()` (input left 497 vs.
                    // an unadjusted `left: 0` menu at 496).
                    style={{
                      position: "absolute",
                      top: "100%",
                      left: 1,
                      margin: 0,
                      zIndex: 1,
                      minWidth: "100%",
                      maxHeight: 320,
                      overflowY: "auto",
                      boxSizing: "border-box",
                    }}
                  >
                    {archiveSearchResults.map((a) => (
                      <Tooltip
                        anchor="cursor"
                        wrapperStyle={{ display: "contents" }}
                        maxWidth={480}
                        label={
                          <img
                            src={`/api/archives/${a.arcid}/thumbnail?no_fallback=true`}
                            alt=""
                            style={{ maxHeight: 420, maxWidth: "100%", display: "block" }}
                          />
                        }
                      >
                        <PopupMenuItem
                          key={a.arcid}
                          onMouseDown={(e) => {
                            e.preventDefault()
                            void addArchiveId(a.arcid, a.title)
                          }}
                        >
                          <span>{a.title}</span>{" "}
                          <span style={{ opacity: 0.45, fontSize: "0.9em" }}>({a.pagecount}p)</span>
                        </PopupMenuItem>
                      </Tooltip>
                    ))}
                  </PopupMenu>
                )}
              </span>
              <input
                className="stdbtn"
                type="button"
                style={{ minWidth: 32 }}
                value={t("Add") ?? undefined}
                onClick={() => void handleAddArchive()}
              />
            </div>
          </div>

          {/* Order matches real legacy exactly (`edit.html.tt2`'s `is_tank` branch: save / Delete
              Tankoubon / Read Tankoubon / Return to Library) — all four always shown, no
              conditional on `archives` being non-empty. "Read Tankoubon" navigates to the tank's
              own ID (`routes.reader(tankId)`, same as legacy's `#read-archive` handler navigating
              to `/reader?id=<the ID field's value>`, which for this branch is the tank ID, not any
              one member archive) — the reader route already resolves a Tankoubon ID into its
              member archives (`ArchiveCard.tsx` uses the same `routes.reader(id)` for both archive
              and Tankoubon cards in the Library grid).
              The label itself is a deliberate departure from legacy, though: legacy reuses the
              exact same "Save Metadata" wording for both archive and Tankoubon edit pages (a
              single shared `#save-metadata` button in one template) — this page uses "Update"
              instead, since a Tankoubon's own archive add/remove/reorder actions already persist
              immediately on each action (unlike legacy, which only saves them as part of this same
              click), so "Save Metadata" undersells what's already been saved by the time this
              button exists purely to commit name/summary/tags. */}
          <div style={{ display: "flex", flexWrap: "wrap", justifyContent: "center", gap: 8, marginTop: 10 }}>
            <input
              className="stdbtn"
              type="button"
              value={t("Update") ?? undefined}
              onClick={() => void handleSave()}
            />
            <input
              className="stdbtn"
              type="button"
              value={t("Delete Tankoubon") ?? undefined}
              onClick={() => void handleDelete()}
            />
            <input
              className="stdbtn"
              type="button"
              value={t("Read Tankoubon") ?? undefined}
              onClick={() => navigate(routes.reader(tankId))}
            />
            <input
              className="stdbtn"
              type="button"
              value={t("Return to Library") ?? undefined}
              onClick={() => navigate(routes.library())}
            />
          </div>
        </div>
      </form>
    </div>
  )
}
