import { useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate, useParams } from "react-router-dom"

import { sendJson } from "@/api/client"
import {
  useArchiveMetadata,
  useDeleteArchive,
  usePlugins,
  useSettings,
  useStats,
  useUpdateArchiveMetadata,
} from "@/api/hooks"
import type { ArchiveMetadata } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { CollapsibleSection } from "@/components/Display"
import { TagInput } from "@/components/Form"
import { confirmDialog } from "@/dialog"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { clearTagsExcept, PROTECTED_TAG_NAMESPACES, sourceTagValues } from "@/lib/tagFormat"
import { findMatchingPlugin } from "@/pages/Upload/shared"
import { toast } from "@/toast"

/** A `source:` tag's URL rewritten into the form the plugin owning that site stores. */
async function formatSourceTag(tag: string): Promise<string> {
  if (!tag.startsWith("source:")) return tag
  const result = await sendJson<{ source: string }>("POST", "/plugins/canonicalize-source", {
    url: tag.slice("source:".length),
  })
  return result.source ? `source:${result.source}` : tag
}

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
    // Recoverable: the archive is in the recycle bin for the configured window, so offer the undo
    // right here instead of making the user find the bin page.
    toast({
      heading: t("recycleBin.movedToBin"),
      icon: "success",
      action: {
        label: t("recycleBin.undo"),
        onClick: () => {
          void (async () => {
            try {
              await sendJson("POST", `/recycle-bin/${encodeURIComponent(archiveId)}/restore`)
              toast({ heading: t("recycleBin.restored"), icon: "success" })
              navigate(`/edit/${archiveId}`)
            } catch {
              toast({ heading: t("recycleBin.restoreFailed"), icon: "error" })
            }
          })()
        },
      },
    })
  }

  function clearTags() {
    const kept = clearTagsExcept(tags, PROTECTED_TAG_NAMESPACES)
    if (kept === tags) return
    const previous = tags
    setTags(kept)
    toast({
      heading: t("edit.tagsCleared"),
      icon: "info",
      action: { label: t("recycleBin.undo"), onClick: () => setTags(previous) },
    })
  }

  /** Runs the metadata plugin a `source:` tag belongs to, or else the enabled chain ingest runs. */
  async function autoScrape() {
    const source = sourceTagValues(tags)
      .map((value) => (/^https?:\/\//i.test(value) ? value : `https://${value}`))
      .map((url) => ({ url, plugin: findMatchingPlugin(plugins.data, url) }))
      .find((match) => match.plugin)
    if (source?.plugin) {
      await runPlugin(source.plugin.namespace, source.plugin.oneshot_arg ? source.url : "")
      return
    }
    setPluginRunning(true)
    try {
      await updateMetadata.mutateAsync({ title, summary, tags })
      const result = await sendJson<{
        successes: number
        failures: number
        declined: number
        added_tags: number
        title: string
        tags: string
        summary: string
      }>("POST", `/plugins/auto?id=${encodeURIComponent(archiveId)}`)
      setTitle(result.title)
      setSummary(result.summary)
      setTags(result.tags)
      if (result.successes === 0 && result.failures === 0 && result.declined === 0) {
        toast({ heading: t("edit.autoScrapeNoPlugin"), icon: "info" })
      } else if (result.successes === 0 && result.failures === 0) {
        toast({ heading: t("edit.autoScrapeAllDeclined"), icon: "info" })
      } else {
        toast({
          heading: t("edit.autoScrapeDone", {
            successes: result.successes,
            failures: result.failures,
            added: result.added_tags,
          }),
          icon: result.failures > 0 ? "warning" : "success",
        })
      }
    } catch (e) {
      toast({ text: String(e), icon: "error" })
    } finally {
      setPluginRunning(false)
    }
  }

  /** Saves current form state first, then fetches+merges plugin tags — not a preview, it
   * unconditionally persists whatever's in the fields right now. */
  async function runPlugin(plugin: string, arg: string) {
    if (!plugin) return
    setPluginRunning(true)
    try {
      await updateMetadata.mutateAsync({ title, summary, tags })
      const response = await fetch(
        `/api/plugins/use?plugin=${encodeURIComponent(plugin)}&id=${encodeURIComponent(archiveId)}&arg=${encodeURIComponent(arg)}`,
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
            <div style={{ display: "flex", flexDirection: "column", alignItems: "flex-start", gap: 6 }}>
              <span>
                {t("common.tags")} <span style={{ fontSize: "6pt" }}>{t("edit.separatedByHyphensIE")}</span> :
              </span>
              <input
                className="stdbtn"
                type="button"
                title={t("edit.clearTagsHint")}
                value={t("edit.clearTags")}
                onClick={clearTags}
              />
            </div>
            <TagInput value={tags} onChange={setTags} suggestions={tagSuggestions} formatTag={formatSourceTag} />
          </div>

          <div style={{ display: "grid", gridTemplateColumns: "120px 1fr", alignItems: "start", gap: 6 }}>
            <span>{t("edit.importTagsFromPlugin")}</span>
            <div style={{ display: "flex", flexDirection: "column", gap: 6, alignItems: "stretch", textAlign: "left" }}>
              <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
                <input
                  className="stdbtn"
                  type="button"
                  style={{ minWidth: 90, height: 25, boxSizing: "border-box" }}
                  disabled={pluginRunning}
                  onClick={() => void autoScrape()}
                  value={t("edit.autoScrape")}
                />

                <Tooltip
                  label={
                    <>
                      <strong>{t("edit.aboutPlugins")}</strong>
                      <br />
                      {t("edit.autoScrapeHelp")}
                    </>
                  }
                >
                  <i className="fas fa-question-circle" style={{ fontSize: 20, cursor: "help" }} aria-hidden="true"></i>
                </Tooltip>
              </div>

              {/* Picking one plugin by hand stays available for the ones no URL can select —
                  filename parsing, copying another archive's tags. */}
              <CollapsibleSection as="div" icon="fa-sliders-h" title={t("edit.choosePluginManually")}>
                <div style={{ display: "flex", flexDirection: "column", gap: 6, padding: 6 }}>
                  <div style={{ display: "flex", gap: 6, alignItems: "center" }}>
                    <select
                      className="favtag-btn"
                      style={{ height: 25, boxSizing: "border-box" }}
                      value={selectedPlugin}
                      onChange={(e) => setSelectedPlugin(e.target.value)}
                    >
                      <option value="">{t("edit.choosePlugin")}</option>
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
                      onClick={() => void runPlugin(selectedPlugin, pluginArg)}
                      value={t("edit.go") ?? undefined}
                    />
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
                </div>
              </CollapsibleSection>

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
