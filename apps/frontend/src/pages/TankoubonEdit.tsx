import { useEffect, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { FaRobot, FaXmark } from "react-icons/fa6"
import { useNavigate, useParams } from "react-router-dom"

import { ApiError } from "@/api/client"
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
import { Modal, PopupMenu, PopupMenuItem } from "@/components/common-ui/Display"
import { SortableList } from "@/components/common-ui/Display"
import { Tooltip } from "@/components/common-ui/Display"
import { Button, IconButton, IconButtonWithTooltip } from "@/components/common-ui/Form"
import { AiSkeleton } from "@/components/Display"
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

  const sortedChapters = [...sug.chapters].sort((a, b) => a.sorted_index - b.sorted_index)

  return (
    <div style={{ display: "flex", flexDirection: "column", gap: 12 }}>
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

      <div
        style={{
          border: "1px solid rgba(128,128,128,0.3)",
          borderRadius: 8,
          overflow: "hidden",
        }}
      >
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

        <div style={{ padding: "14px 16px 8px" }}>
          <div style={{ fontSize: 10, opacity: 0.5, marginBottom: 4, textTransform: "uppercase", letterSpacing: "0.5px" }}>
            {t("Title")}
          </div>
          <div style={{ fontSize: 15, fontWeight: 700, lineHeight: 1.3 }}>
            {sug.tank_name}
          </div>
        </div>

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

/** Archive title with hover-thumbnail tooltip, sourced from the tankoubon's own `full_data`. */
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
  const titleById = useRef(new Map<string, string>())
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
        {t("common.loadingLibrary")}
      </div>
    )
  }

  if (tankoubonFull.isError || !tankoubon) {
    if (tankoubonFull.error instanceof ApiError && tankoubonFull.error.status === 401) return null

    return (
      <div className="ido" style={{ textAlign: "center", maxWidth: 800, margin: "10px auto", display: "flex", flexDirection: "column", gap: 12, alignItems: "center" }}>
        <p className="text-red-500">
          {t("common.failedToLoadArchivesError", { error: String(tankoubonFull.error) })}
        </p>
        <input
          className="stdbtn"
          type="button"
          value={t("common.returnToLibrary") ?? undefined}
          onClick={() => navigate(routes.library())}
        />
      </div>
    )
  }

  // Keyed by tankId so switching tankoubons remounts the form with fresh initial state.
  return <TankoubonForm key={tankId} tankId={tankId} tankoubon={tankoubon} titleById={titleById.current} />
}

function TankoubonForm({ tankId, tankoubon, titleById }: { tankId: string; tankoubon: TankoubonMetadata; titleById: Map<string, string> }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  useDocumentTitle(t("tankoubonEdit.editing1Tankoubon").replace("%1", tankoubon.name))
  const updateTankoubon = useUpdateTankoubon(tankId)
  const deleteTankoubon = useDeleteTankoubon()
  const stats = useStats(2)
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

  const [debouncedQuery, setDebouncedQuery] = useState("")
  useEffect(() => {
    const timeout = setTimeout(() => setDebouncedQuery(newArchiveId.trim()), 250)
    return () => clearTimeout(timeout)
  }, [newArchiveId])
  const archiveSearch = useSearch({ filter: debouncedQuery, enabled: debouncedQuery.length > 0 })
  const archiveSearchResults = (archiveSearch.data?.data ?? [])
    .filter((a) => a.archive_count === null && !archives.includes(a.arcid))
    .slice(0, 15)

  async function handleSave() {
    await updateTankoubon.mutateAsync({
      archives,
      metadata: { name, summary, tags, chapter_names: Object.entries(chapterNames).map(([id, name]) => ({ id, name })) },
    })
    toast({ heading: t("edit.metadataSaved") ?? undefined, icon: "success" })
  }

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

  function addArchiveId(archiveId: string, title?: string) {
    setArchives((prev) => [...prev, archiveId])
    if (title) titleById.set(archiveId, title)
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
        {t("tankoubonEdit.editing1Tankoubon").replace("%1", tankoubon.name)}
      </h2>

      <form
        autoComplete="off"
        style={{ width: "98%", maxWidth: 700, margin: "0 auto", fontSize: "8pt" }}
        onSubmit={(e) => e.preventDefault()}
      >
        <div style={{ display: "flex", flexDirection: "column", gap: 10, textAlign: "left" }}>
          <div style={{ display: "grid", gridTemplateColumns: "120px 1fr", alignItems: "center", gap: 6 }}>
            <span>{t("edit.title")}</span>
            <input
              className="stdinput"
              type="text"
              style={{ width: "100%", maxWidth: "none" }}
              value={name}
              onChange={(e) => setName(e.target.value)}
            />
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "120px 1fr", alignItems: "start", gap: 6 }}>
            <span>{t("edit.summary")}</span>
            <textarea
              className="stdinput"
              style={{ width: "100%", maxWidth: "none", minHeight: 72, boxSizing: "border-box" }}
              value={summary}
              onChange={(e) => setSummary(e.target.value)}
            />
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "120px 1fr", alignItems: "start", gap: 6 }}>
            <span>
              {t("common.tags")} <span style={{ fontSize: "6pt" }}>{t("edit.separatedByHyphensIE")}</span> :
            </span>
            <TagInput value={tags} onChange={setTags} suggestions={tagSuggestions} />
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "120px 1fr", alignItems: "start", gap: 6 }}>
            <span>{t("tankoubonEdit.archives")}</span>
            <div style={{ display: "flex", flexDirection: "column", gap: 4, width: "100%" }}>
            {hasLlmKey && (
              <IconButtonWithTooltip
                icon={<FaRobot size={16} />}
                title={t("tankoubonEdit.aiSmartRename")}
                description={t("tankoubonEdit.analyzeTheArchiveListAnd")}
                size={26}
                style={{ alignSelf: "flex-start" }}
                disabled={aiRename.isPending}
                onClick={() => {
                  setAiOverlayOpen(true)
                  setAiSuggestions(null)
                  aiRename.mutate(tankId, {
                    onSuccess: (data) => { setAiSuggestions(data) },
                    onError: (err) => {
                      setAiOverlayOpen(false)
                      toast({ heading: t("tankoubonEdit.aiRenameFailed") ?? undefined, text: String(err), icon: "error" })
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
                      <Button
                        onClick={() => navigate(routes.edit(archiveId))}
                        style={{ minWidth: 32, margin: 0 }}
                      >
                        {t("tankoubonEdit.edit")}
                      </Button>
                      <IconButton
                        icon={<FaXmark size={16} />}
                        onClick={() => removeArchive(archiveId)}
                        title={t("tankoubonEdit.removeFromTankoubon") ?? undefined}
                        // `.stdbtn` is `box-sizing: border-box`, so its own `height: 21px` already
                        // includes its `border: 2px` — confirmed via `getBoundingClientRect` (21px
                        // rendered, not 25px) rather than assumed. Matched here so the two buttons
                        // share one row height instead of the button defaulting to its own
                        // unrelated 26px square.
                        size={21}
                      />
                    </div>

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
                            placeholder={t("tankoubonEdit.chapterName") ?? undefined}
                            value={chapterNames[archiveId] ?? ""}
                            onChange={(e) => setChapterNames((prev) => ({ ...prev, [archiveId]: e.target.value }))}
                            // `.stdinput`'s own `margin: 4px 1px 0` pushes this input's visual box
                            // 4px lower than the icon button beside it under this row's
                            // `alignItems: center` — both are centered by their own margin-box, so
                            // a margin only one of them has desyncs the two centerlines. Zeroed
                            // here rather than compensating on the button, since the margin has no
                            // purpose in this flex row to begin with.
                            style={{ flex: 1, maxWidth: "none", height: 18, fontSize: "9.33px", margin: 0 }}
                          />
                        )}
                        <IconButtonWithTooltip
                          icon={<FaRobot size={12} />}
                          title={t("tankoubonEdit.aiChapterName")}
                          description={t("tankoubonEdit.suggestAChapterTitleBased")}
                          size={18}
                          style={{ fontSize: "9.33px" }}
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
                                    toast({ heading: t("tankoubonEdit.chapterNameUnchanged"), text: data.name, icon: "info" })
                                  } else {
                                    toast({ heading: t("tankoubonEdit.chapterNameSuggested"), text: data.name, icon: "success" })
                                  }
                                },
                                onError: (err) => {
                                  setChapterAiLoading(null)
                                  toast({ heading: t("tankoubonEdit.aiRenameFailed") ?? undefined, text: String(err), icon: "error" })
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
                      const mem = aiSuggestions.original_member_names.find((m) => m.index === ch.original_index)
                      if (mem) {
                        if (ch.name) next[mem.id] = ch.name
                        reordered.push(mem.id)
                      }
                    }
                    setChapterNames((prev) => ({ ...prev, ...next }))
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

          <style>{`
            @keyframes ai-shimmer {
              0%   { background-position: -200% 0; }
              100% { background-position: 200% 0; }
            }
          `}</style>

          <div style={{ display: "grid", gridTemplateColumns: "120px 1fr", alignItems: "center", gap: 6 }}>
            <span>{t("tankoubonEdit.addArchiveToTankoubon")}</span>
            <div style={{ display: "flex", gap: 6 }}>
              <span style={{ position: "relative", width: "100%" }}>
                <input
                  className="stdinput"
                  type="text"
                  autoComplete="off"
                  style={{ width: "100%", maxWidth: "none", height: 21, boxSizing: "border-box" }}
                  value={newArchiveId}
                  onChange={(e) => {
                    setNewArchiveId(e.target.value)
                    setArchiveSearchOpen(true)
                  }}
                  onFocus={() => setArchiveSearchOpen(true)}
                  onBlur={() => setTimeout(() => setArchiveSearchOpen(false), 150)}
                  onKeyDown={(e) => {
                    if (e.key === "Escape") setArchiveSearchOpen(false)
                  }}
                  placeholder={t("tankoubonEdit.archiveId40characterLong") ?? undefined}
                />
                {archiveSearchOpen && archiveSearchResults.length > 0 && (
                  <PopupMenu
                    portal={false}
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
                value={t("tankoubonEdit.add") ?? undefined}
                onClick={() => void handleAddArchive()}
              />
            </div>
          </div>

          <div style={{ display: "flex", flexWrap: "wrap", justifyContent: "center", gap: 8, marginTop: 10 }}>
            <input
              className="stdbtn"
              type="button"
              value={t("tankoubonEdit.update") ?? undefined}
              onClick={() => void handleSave()}
            />
            <input
              className="stdbtn"
              type="button"
              value={t("common.deleteTankoubon") ?? undefined}
              onClick={() => void handleDelete()}
            />
            <input
              className="stdbtn"
              type="button"
              value={t("tankoubonEdit.readTankoubon") ?? undefined}
              onClick={() => navigate(routes.reader(tankId))}
            />
            <input
              className="stdbtn"
              type="button"
              value={t("common.returnToLibrary") ?? undefined}
              onClick={() => navigate(routes.library())}
            />
          </div>
        </div>
      </form>
    </div>
  )
}
