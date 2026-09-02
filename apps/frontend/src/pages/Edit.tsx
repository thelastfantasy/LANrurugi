import { useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate, useParams } from "react-router-dom"

import { ApiError } from "@/api/client"
import {
  useArchiveMetadata,
  useDeleteArchive,
  usePlugins,
  useRenameArchive,
  useSettings,
  useStats,
  useUpdateArchiveMetadata,
} from "@/api/hooks"
import type { ArchiveMetadata } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { TagInput } from "@/components/Form"
import { confirmDialog, renameArchiveDialog } from "@/dialog"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { dismissToast, toast } from "@/toast"

export function Edit() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const { archiveId = "" } = useParams<{ archiveId: string }>()
  const metadata = useArchiveMetadata(archiveId)

  if (metadata.isLoading) {
    return (
      <div className="ido" style={{ textAlign: "center", maxWidth: 800, margin: "10px auto", color: "var(--theme-muted)" }}>
        {t("common.loadingLibrary")}
      </div>
    )
  }

  if (metadata.isError || !metadata.data) {
    return (
      <div className="ido" style={{ textAlign: "center", maxWidth: 800, margin: "10px auto", display: "flex", flexDirection: "column", gap: 12, alignItems: "center" }}>
        <p className="text-red-500">
          {t("common.failedToLoadArchivesError", { error: String(metadata.error) })}
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

  // Keyed by archiveId so switching between archives' edit pages remounts with fresh state.
  return <EditForm key={archiveId} archiveId={archiveId} archive={metadata.data} />
}

function EditForm({ archiveId, archive }: { archiveId: string; archive: ArchiveMetadata }) {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const plugins = usePlugins("metadata")
  const settings = useSettings()
  const stats = useStats(2)
  const updateMetadata = useUpdateArchiveMetadata(archiveId)
  const deleteArchive = useDeleteArchive()
  const renameArchive = useRenameArchive(archiveId)

  useDocumentTitle(t("edit.editing1").replace("%1", archive.title))
  const [title, setTitle] = useState(archive.title)
  const [summary, setSummary] = useState(archive.summary ?? "")
  const [tags, setTags] = useState(archive.tags)
  const [selectedPlugin, setSelectedPlugin] = useState("")
  const [pluginArg, setPluginArg] = useState("")
  const [pluginRunning, setPluginRunning] = useState(false)

  const tagSuggestions = (stats.data ?? []).map((s) => (s.namespace ? `${s.namespace}:${s.text}` : s.text))

  async function handleSave() {
    await updateMetadata.mutateAsync({ title, summary, tags })
    toast({ heading: t("edit.metadataSaved") ?? undefined, icon: "success" })
  }

  async function handleDelete() {
    if (!(await confirmDialog(t("edit.areYouSureYouWant") ?? "", true))) return
    await deleteArchive.mutateAsync(archiveId)
    navigate("/")
  }

  /** Renames the on-disk file — distinct from `title`, a display label. Only the stem round-trips;
   * `archive.extension` is a separate, already-dotless field. */
  async function handleRename() {
    const nextStem = await renameArchiveDialog(archive.filename, archive.extension)
    if (nextStem === null || nextStem.trim() === "" || nextStem.trim() === archive.filename) return
    const pendingToastId = toast({
      heading: t("edit.renaming") ?? undefined,
      text: t("edit.thisMayTakeAMoment") ?? undefined,
      icon: "info",
      hideAfter: false,
      closeOnClick: false,
    })
    try {
      const result = await renameArchive.mutateAsync(nextStem.trim())
      dismissToast(pendingToastId)
      toast({
        heading: t("edit.archiveRenamedTo") ?? undefined,
        text: result.filename,
        icon: "success",
      })
    } catch (err) {
      dismissToast(pendingToastId)
      toast({
        heading: t("edit.renameFailed") ?? undefined,
        text: err instanceof ApiError ? err.message : String(err),
        icon: "error",
      })
    }
  }

  /** Saves current form state first, then fetches+merges plugin tags — not a preview, it
   * unconditionally persists whatever's in the fields right now. */
  async function runPlugin() {
    if (!selectedPlugin) return
    setPluginRunning(true)
    try {
      await updateMetadata.mutateAsync({ title, summary, tags })
      const response = await fetch(
        `/api/plugins/use?plugin=${encodeURIComponent(selectedPlugin)}&id=${encodeURIComponent(archiveId)}&arg=${encodeURIComponent(pluginArg)}`,
        { method: "POST" },
      )
      const data = (await response.json()) as {
        success: number
        data?: { tags?: string; title?: string; summary?: string }
        error?: string
      }
      if (data.success && data.data) {
        const result = data.data
        let nextTitle = title
        let nextSummary = summary
        let nextTags = tags
        if (result.title && (settings.data?.replacetitles ?? true)) {
          nextTitle = result.title
          setTitle(nextTitle)
          toast({ heading: t("edit.archiveTitleChangedTo") ?? undefined, text: result.title, icon: "info" })
        }
        if (result.summary) {
          nextSummary = result.summary
          setSummary(nextSummary)
          toast({ heading: t("edit.archiveSummaryUpdated") ?? undefined, icon: "info" })
        }
        if (result.tags) {
          const existing = tags
            .split(/,\s?/)
            .map((t) => t.trim())
            .filter(Boolean)
          const actuallyNew: string[] = []
          const merged = [...existing]
          for (const tag of result.tags.split(/,\s?/)) {
            const trimmed = tag.trim()
            if (trimmed && !merged.includes(trimmed)) {
              merged.push(trimmed)
              actuallyNew.push(trimmed)
            }
          }
          nextTags = merged.join(", ")
          setTags(nextTags)
          if (actuallyNew.length > 0) {
            toast({
              heading: t("edit.addedTheFollowingTags") ?? undefined,
              text: actuallyNew.join(", "),
              icon: "info",
              hideAfter: 7000,
            })
          } else {
            toast({ heading: t("edit.noNewTagsAdded") ?? undefined, icon: "info" })
          }
        } else {
          toast({ heading: t("edit.noNewTagsAdded") ?? undefined, icon: "info" })
        }
        if (nextTitle !== title || nextSummary !== summary || nextTags !== tags) {
          await updateMetadata.mutateAsync({ title: nextTitle, summary: nextSummary, tags: nextTags })
        }
      } else {
        toast({ text: data.error ?? t("edit.unknownError") ?? undefined, icon: "error" })
      }
    } finally {
      setPluginRunning(false)
    }
  }

  const selectedPluginData = plugins.data?.find((p) => p.namespace === selectedPlugin)

  return (
    <div className="ido" style={{ textAlign: "center", maxWidth: 800, margin: "10px auto" }}>
      <h2 className="ih" style={{ textAlign: "center" }}>
        {t("edit.editing1").replace("%1", archive.title)}
      </h2>

      <form
        autoComplete="off"
        style={{ width: "98%", maxWidth: 700, margin: "0 auto", fontSize: "8pt" }}
        onSubmit={(e) => e.preventDefault()}
      >
        <div style={{ display: "flex", flexDirection: "column", gap: 10, textAlign: "left" }}>
          {/* maxWidth: 'none' overrides .stdinput's theme cap of 450px so fields fill the
              grid column on this page's wider card. */}
          <div style={{ display: "grid", gridTemplateColumns: "120px 1fr auto", alignItems: "center", gap: 6 }}>
            <span>{t("edit.currentFileName")}</span>
            <input readOnly className="stdinput" type="text" style={{ width: "100%", maxWidth: "none" }} value={archive.filename} />
            <input
              className="stdbtn"
              type="button"
              value={t("edit.rename") ?? undefined}
              onClick={() => void handleRename()}
              disabled={renameArchive.isPending}
              style={{ minWidth: 70, height: 18, boxSizing: "border-box" }}
            />
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "120px 1fr", alignItems: "center", gap: 6 }}>
            <span>{t("edit.id")}</span>
            <input readOnly className="stdinput" type="text" style={{ width: "100%", maxWidth: "none" }} maxLength={255} value={archiveId} />
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "120px 1fr", alignItems: "center", gap: 6 }}>
            <span>{t("edit.title")}</span>
            <input
              id="title"
              className="stdinput"
              type="text"
              style={{ width: "100%", maxWidth: "none" }}
              maxLength={255}
              value={title}
              onChange={(e) => setTitle(e.target.value)}
            />
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "120px 1fr", alignItems: "start", gap: 6 }}>
            <span>{t("edit.summary")}</span>
            <textarea
              id="summary"
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
            <span>{t("edit.importTagsFromPlugin")}</span>
            <div style={{ display: "flex", flexDirection: "column", gap: 6, alignItems: "flex-start", textAlign: "left" }}>
              <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
                <select
                  className="favtag-btn"
                  style={{ height: 25, boxSizing: "border-box" }}
                  value={selectedPlugin}
                  onChange={(e) => setSelectedPlugin(e.target.value)}
                >
                  <option value="">{t("common.NoCategory")}</option>
                  {plugins.data?.map((p) => (
                    <option key={p.namespace} value={p.namespace}>
                      {p.name}
                    </option>
                  ))}
                </select>

                <input
                  className="stdbtn"
                  type="button"
                  style={{ minWidth: 90, height: 25, boxSizing: "border-box" }}
                  disabled={!selectedPlugin || pluginRunning}
                  onClick={() => void runPlugin()}
                  value={t("edit.go") ?? undefined}
                />

                <Tooltip
                  label={
                    <>
                      <strong>{t("edit.aboutPlugins")}</strong>
                      <br />
                      <span
                        dangerouslySetInnerHTML={{
                          __html: t(
                            "edit.youCanUsePluginsTo",
                          ),
                        }}
                      />
                    </>
                  }
                >
                  <i className="fas fa-question-circle" style={{ fontSize: 20, cursor: "help" }} aria-hidden="true"></i>
                </Tooltip>
              </div>

              {selectedPluginData?.oneshot_arg && (
                <div style={{ display: "flex", flexDirection: "column", gap: 4, width: "100%" }}>
                  <span>{selectedPluginData.oneshot_arg} :</span>
                  <input
                    className="stdinput"
                    type="text"
                    style={{ width: "100%", boxSizing: "border-box" }}
                    value={pluginArg}
                    onChange={(e) => setPluginArg(e.target.value)}
                  />
                </div>
              )}

              <div style={{ display: "flex", alignItems: "center", gap: 6 }}>
                <i className="fa fa-2x fa-exclamation-circle"></i>
                {t("edit.usingAPluginWillSave")}
              </div>
            </div>
          </div>

          <div style={{ display: "flex", flexWrap: "wrap", justifyContent: "center", gap: 8, marginTop: 10 }}>
            <input
              className="stdbtn"
              type="button"
              value={t("edit.saveMetadata") ?? undefined}
              onClick={() => void handleSave()}
            />
            <input
              className="stdbtn"
              type="button"
              value={t("common.deleteArchive") ?? undefined}
              onClick={() => void handleDelete()}
            />
            <input
              className="stdbtn"
              type="button"
              value={t("edit.readArchive") ?? undefined}
              onClick={() => navigate(routes.reader(archiveId))}
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
