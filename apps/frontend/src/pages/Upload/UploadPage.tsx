import { useQueryClient } from "@tanstack/react-query"
import { useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { fetchJson } from "@/api/client"
import { useAddToQueue, useCategories, useCreateCategory, useDownloadQueue, usePlugins } from "@/api/hooks"
import type { PluginInfo } from "@/api/types"
import { STATE_COLOR } from "@/components/Display"
import { Tooltip } from "@/components/Display"
import { newCategoryDialog } from "@/dialog"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { routes } from "@/lib/routes"
import { FONT_SIZE_XS, useApplyTheme } from "@/theme"
import { toast } from "@/toast"

import { DownloadQueuePanel } from "./DownloadQueuePanel"
import { findMatchingPlugin } from "./shared"

// "Add from URL" stages matched URLs into a persistent, server-side queue (`useDownloadQueue`),
// grouped by which download plugin's `url_pattern` matched, so the queue survives a page refresh
// or a different browser tab. Manual file upload (left column) now goes through that exact same
// persistent queue too (`crates/lanrurugi-api/src/upload.rs` writes a `local_upload`-origin queue
// item before/around its own synchronous ingest) — `DownloadQueuePanel` renders both kinds from
// one `useDownloadQueue()` poll, so this page itself no longer tracks any upload state of its own.
export function Upload() {
  const { t } = useTranslation()
  const navigate = useNavigate()
  const queryClient = useQueryClient()
  const categories = useCategories()
  const createCategory = useCreateCategory()
  const downloadPlugins = usePlugins("download")
  const metadataPlugins = usePlugins("metadata")
  const downloadQueue = useDownloadQueue()
  const fileInputRef = useRef<HTMLInputElement>(null)
  const [category, setCategory] = useState("")
  const [urls, setUrls] = useState("")
  const [unmatchedUrls, setUnmatchedUrls] = useState<string[]>([])
  const [uploadingCount, setUploadingCount] = useState(0)
  useApplyTheme()
  useDocumentTitle(t("upload.uploadCenter") ?? undefined)

  async function handleUpload(toUpload: File) {
    setUploadingCount((n) => n + 1)
    try {
      const formData = new FormData()
      formData.append("file", toUpload)
      if (category) formData.append("catid", category)

      // The handler's own JSON response is no longer this page's source of truth for what to show
      // — the queue item it created (and its outcome, including a filename conflict) is. Refetch
      // is what actually surfaces the new row; `archives`/`stats` are invalidated too so the
      // Library/Stats pages don't need their own separate refresh to see a successful upload.
      await fetch("/api/archives/upload", { method: "PUT", body: formData }).catch(() => null)
      await Promise.all([
        downloadQueue.refetch(),
        queryClient.invalidateQueries({ queryKey: ["archives"] }),
        queryClient.invalidateQueries({ queryKey: ["stats"] }),
      ])
    } finally {
      setUploadingCount((n) => n - 1)
      if (fileInputRef.current) fileInputRef.current.value = ""
    }
  }

  const addToQueue = useAddToQueue()

  async function handleAddToQueue() {
    const list = Array.from(new Set(urls.split("\n").map((u) => u.trim()).filter(Boolean)))
    if (list.length === 0) return

    const matched: { url: string; plugin: PluginInfo }[] = []
    const unmatched: string[] = []
    for (const url of list) {
      const plugin = findMatchingPlugin(downloadPlugins.data, url)
      if (plugin) matched.push({ url, plugin })
      else unmatched.push(url)
    }
    setUnmatchedUrls(unmatched)
    if (matched.length === 0) return

    const items = matched.map(({ url, plugin }) => ({
      url,
      plugin_namespace: plugin.namespace,
      category: category || undefined,
      metadataNamespace: findMatchingPlugin(metadataPlugins.data, url)?.namespace,
    }))
    await resolveDefaultsAndAdd(items)
    setUrls("")
  }

  // Resolving each URL's checkbox defaults needs a per-plugin settings/options fetch, which can't
  // happen inside a plain callback (hooks can't be called conditionally/in a loop) — so defaults
  // are resolved via direct one-off fetches instead of hooks.
  async function resolveDefaultsAndAdd(
    items: Array<{
      url: string
      plugin_namespace: string
      category?: string
      metadataNamespace?: string
    }>,
  ) {
    const resolved = await Promise.all(
      items.map(async (item) => {
        let autoFetch = false
        if (item.metadataNamespace) {
          const settings = await fetchJson<{ enabled: boolean }>(
            `/plugins/settings?namespace=${encodeURIComponent(item.metadataNamespace)}`,
          ).catch(() => null)
          autoFetch = settings?.enabled ?? false
        }
        const options = await fetchJson<{ overwrite_on_duplicate?: { value: boolean } }>(
          `/plugins/options?namespace=${encodeURIComponent(item.plugin_namespace)}`,
        ).catch(() => null)
        let overwrite: boolean
        if (options?.overwrite_on_duplicate) {
          overwrite = options.overwrite_on_duplicate.value
        } else {
          const settings = await fetchJson<{ replacedupe: boolean }>("/settings").catch(() => null)
          overwrite = settings?.replacedupe ?? false
        }
        return {
          url: item.url,
          plugin_namespace: item.plugin_namespace,
          category: item.category,
          auto_fetch_metadata: autoFetch,
          overwrite_on_duplicate: overwrite,
        }
      }),
    )
    await addToQueue.mutateAsync(resolved)
  }

  async function handleNewCategory() {
    const result = await newCategoryDialog()
    if (result === null) return
    try {
      const data = await createCategory.mutateAsync(result)
      setCategory(data.category_id)
    } catch {
      toast({ heading: t("common.errorModifyingCategory") ?? undefined, icon: "error" })
    }
  }

  return (
    <div className="ido" style={{ textAlign: "center", fontSize: FONT_SIZE_XS }}>
      <h1 className="ih" style={{ textAlign: "center" }}>
        {t("upload.addingArchivesToTheLibrary")}
      </h1>

      {t("upload.addFilesToYourLanrurugi")}
      <br />
      <br />

      <div style={{ marginLeft: "auto", marginRight: "auto" }}>
        <div className="left-column">
          {t("upload.addUploadedFilesToCategory")}
          <select id="category" className="favtag-btn" value={category} onChange={(e) => setCategory(e.target.value)}>
            <option value="">{t("common.NoCategory")}</option>
            {categories.data?.map((c) => (
              <option key={c.id} value={c.id}>
                {c.name}
              </option>
            ))}
          </select>
          <Tooltip label={t("common.newCategory") ?? undefined}>
            <a
              href="#"
              style={{ marginLeft: 6 }}
              onClick={(e) => {
                e.preventDefault()
                void handleNewCategory()
              }}
            >
              <i className="fas fa-plus" />
            </a>
          </Tooltip>
          <br />
          <br />

          <h1 className="ih">{t("upload.fromYourComputer")}</h1>

          {t("upload.youCanDragAndDrop")}
          <br />
          <br />

          <span className="stdbtn fileinput-button" style={{ minHeight: 50, padding: "8px 12px" }}>
            <i className="fas fa-download fa-2x" style={{ paddingTop: 6, paddingBottom: 5 }}></i>
            <br />
            <span>{t("upload.addFromYourComputer")}</span>
            <input
              ref={fileInputRef}
              type="file"
              name="file"
              accept=".zip,.cbz,.rar,.cbr,.7z,.cb7,.pdf,.epub"
              disabled={uploadingCount > 0}
              onChange={(e) => {
                const chosen = e.target.files?.[0] ?? null
                if (chosen) void handleUpload(chosen)
              }}
            />
          </span>

          <br />
          <br />
          <h1 className="ih">{t("upload.fromTheInternet")}</h1>

          {t("upload.youCanDownloadFilesFrom")}
          <br />
          {t("upload.downloadJobsWillKeepGoing")}
          <br />
          <br />

          {t("upload.typeInYourUrlsSeparated")}
          <br />
          {t("upload.ifADownloaderPluginIs")}
          <br />
          <br />

          <label htmlFor="urlForm">{t("upload.urlSToDownload")}</label>
          <br />
          <textarea
            id="urlForm"
            value={urls}
            onChange={(e) => setUrls(e.target.value)}
            onPaste={(e) => {
              const raw = e.clipboardData.getData("text")
              if (!raw.trim()) return
              // 无换行 → 识别分隔符转成换行；有换行 → 原样保留；单条 URL 原样保留
              let insert: string
              if (raw.includes("\n")) {
                insert = raw
              } else {
                const sep = raw.includes(",") ? /,\s*/ : /\s+/
                const lines = raw.trim().split(sep).filter(Boolean)
                insert = lines.length <= 1 ? raw.trim() : lines.join("\n")
              }
              // 末尾无空行时追加一个
              if (!insert.endsWith("\n\n")) insert = insert.replace(/\n?$/, "\n")
              e.preventDefault()
              const ta = e.target as HTMLTextAreaElement
              setUrls(urls.slice(0, ta.selectionStart) + insert + urls.slice(ta.selectionEnd))
            }}
            style={{ width: 400, height: 100, whiteSpace: "pre" }}
          />
          <div style={{ fontSize: FONT_SIZE_XS, opacity: 0.5, textAlign: "left" }}>
            {t("upload.validUrlLinesCount", { count: urls.split("\n").map((u) => u.trim()).filter(Boolean).length })}
          </div>
          <br />

          <span
            id="add-to-queue"
            className="stdbtn fileinput-button"
            style={{ minHeight: 50, padding: "8px 12px" }}
            onClick={() => !addToQueue.isPending && urls.trim() && void handleAddToQueue()}
          >
            <i className="fas fa-list fa-2x" style={{ paddingTop: 6, paddingBottom: 5 }}></i>
            <br />
            <span>{t("upload.addToQueue")}</span>
          </span>

          {unmatchedUrls.length > 0 && (
            <div style={{ marginTop: 12, textAlign: "left", color: STATE_COLOR.failed }}>
              <i className="fa fa-exclamation-circle"></i>{" "}
              {t("upload.noInstalledDownloadPluginRecognizes", { n: unmatchedUrls.length })}
              <ul style={{ margin: "4px 0 0", paddingLeft: 20 }}>
                {unmatchedUrls.map((u) => (
                  <li key={u} style={{ wordBreak: "break-all" }}>
                    {u}
                  </li>
                ))}
              </ul>
            </div>
          )}
        </div>

        <div className="right-column" style={{ paddingLeft: 24, boxSizing: "border-box" }}>
          <DownloadQueuePanel downloadPlugins={downloadPlugins.data} metadataPlugins={metadataPlugins.data} />
        </div>
      </div>

      <br />
      <br />
      <input type="button" id="return" className="stdbtn" value={t("common.returnToLibrary") ?? undefined} onClick={() => navigate(routes.library())} />
    </div>
  )
}
