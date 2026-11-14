import { useQueryClient } from "@tanstack/react-query"
import { lazy, Suspense, useRef, useState } from "react"
import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import { fetchJson } from "@/api/client"
import { useAddToQueue, useCategories, useCreateCategory, useDownloadQueue, usePlugins } from "@/api/hooks"
import type { PluginInfo } from "@/api/types"
import { uploadResumable } from "@/api/uploadSession"
import { Drawer, Tooltip } from "@/components/common-ui/Display"
import { formatBytes, STATE_COLOR } from "@/components/Display"
import { reportWarning } from "@/diagnostics/sdk"
import { newCategoryDialog } from "@/dialog"
import { useDocumentTitle } from "@/hooks/useDocumentTitle"
import { useIsNarrowViewport } from "@/hooks/useIsNarrowViewport"
import { routes } from "@/lib/routes"
import { UPLOAD_ACCEPT } from "@/lib/utils/bookFormats"
import { sortCategories } from "@/lib/utils/sortCategories"
import { FONT_SIZE_XS, useApplyTheme, Z_OVERLAY_ABOVE_DRAWER } from "@/theme"
import { toast } from "@/toast"

import { DownloadQueuePanel } from "./DownloadQueuePanel"
import { ReservationGroup } from "./ReservationGroup"
import { findMatchingPlugin, findPluginByDomain } from "./shared"
import { SubscriptionStatus } from "./SubscriptionStatus"

/** Splits pasted text into one URL per line (comma- or whitespace-separated if it has no newlines)
 * and ends it with a newline, so the next paste starts on its own line. */
function urlLinesForInsert(raw: string): string {
  let insert: string
  if (raw.includes("\n")) {
    insert = raw
  } else {
    const sep = raw.includes(",") ? /,\s*/ : /\s+/
    const lines = raw.trim().split(sep).filter(Boolean)
    insert = lines.length <= 1 ? raw.trim() : lines.join("\n")
  }
  return insert.endsWith("\n\n") ? insert : insert.replace(/\n?$/, "\n")
}

// Loaded only when opened: it carries the QR decoder.
const ImportShareModal = lazy(() =>
  import("@/share/ImportShareModal").then((m) => ({ default: m.ImportShareModal })),
)

/** Both "Add from URL" and manual file upload stage into the same persistent server-side queue
 * (`useDownloadQueue`); `DownloadQueuePanel` renders both kinds from one poll. */
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
  const [importOpen, setImportOpen] = useState(false)
  // Narrow viewports put the whole "add archives" column behind a drawer button: it pushed the
  // queue itself below the fold, and the queue is what this page is opened for on a phone.
  const [formOpen, setFormOpen] = useState(false)
  // Matches the stylesheet breakpoint that stacks these two columns (`@media (max-width: 768px)`).
  const narrow = useIsNarrowViewport("(max-width: 768px)")
  const [urls, setUrls] = useState("")
  // Value before an IME composition began; the URL split waits until the composition ends.
  const composingFrom = useRef<string | null>(null)
  // Keyboard clipboard managers (Gboard) commit text through the IME, not a `paste` event, so a
  // multi-character insertion carrying a URL gets the same one-per-line treatment as a paste.
  function commitUrls(ta: HTMLTextAreaElement, before: string) {
    const next = ta.value
    let start = 0
    while (start < before.length && start < next.length && before[start] === next[start]) start++
    let end = 0
    while (end < before.length - start && end < next.length - start && before[before.length - 1 - end] === next[next.length - 1 - end]) end++
    const inserted = next.slice(start, next.length - end)
    if (inserted.length > 1 && !inserted.includes("\n") && /https?:\/\//.test(inserted)) {
      const insert = urlLinesForInsert(inserted)
      setUrls(next.slice(0, start) + insert + next.slice(next.length - end))
      requestAnimationFrame(() => ta.setSelectionRange(start + insert.length, start + insert.length))
      return
    }
    setUrls(next)
  }
  const [unmatchedUrls, setUnmatchedUrls] = useState<string[]>([])
  const [uploadingCount, setUploadingCount] = useState(0)
  const [uploadProgress, setUploadProgress] = useState<{ loaded: number; total: number } | null>(null)
  // Bytes the server already held when this upload started, when it was a resume of an
  // interrupted one — surfaced so the transfer does not look stuck at 60% for no reason.
  const [resumedFrom, setResumedFrom] = useState<number | null>(null)
  useApplyTheme()
  useDocumentTitle(t("upload.uploadCenter") ?? undefined)

  async function handleUpload(toUpload: File) {
    // The picker lives in the narrow-screen drawer: once a file is chosen the queue is what the
    // user is waiting to see, so the drawer gets out of the way. Its own progress bar is not lost —
    // the queue row for this upload reports the same transfer.
    setFormOpen(false)
    setUploadingCount((n) => n + 1)
    setUploadProgress({ loaded: 0, total: toUpload.size })
    setResumedFrom(null)
    try {
      // Chunked and resumable: a dropped connection costs one chunk, and re-picking the same file
      // continues where the server left off rather than starting over. The ingest that runs at
      // `complete` is the same one the old one-shot multipart call ran, so the queue item, its
      // overwrite/rename prompts, and the activity records are all unchanged.
      await uploadResumable(
        toUpload,
        {
          catid: category || undefined,
          onResume: (offset) => setResumedFrom(offset > 0 ? offset : null),
          onProgress: (loaded, total) => setUploadProgress({ loaded, total }),
        },
      ).catch((err: unknown) => {
        // The queue item's own outcome is the source of truth when the transfer worked; a failure
        // *before* that (network, expired session, ingest refusal) has no queue row to show, so it
        // is reported here instead of vanishing. The session's staged bytes stay on the server, so
        // picking the same file again resumes it.
        toast({
          heading: t("upload.uploadFailed"),
          text: err instanceof Error ? err.message : String(err),
          icon: "error",
        })
        return null
      })
      await Promise.all([
        downloadQueue.refetch(),
        queryClient.invalidateQueries({ queryKey: ["archives"] }),
        queryClient.invalidateQueries({ queryKey: ["stats"] }),
      ])
    } finally {
      setUploadingCount((n) => n - 1)
      setUploadProgress(null)
      setResumedFrom(null)
      if (fileInputRef.current) fileInputRef.current.value = ""
    }
  }

  const addToQueue = useAddToQueue()

  function hostnameOfUrl(url: string): string {
    try {
      return new URL(url).hostname
    } catch {
      return "(unparseable)"
    }
  }

  async function handleAddToQueue() {
    const list = Array.from(new Set(urls.split("\n").map((u) => u.trim()).filter(Boolean)))
    if (list.length === 0) return
    // Same reasoning as the file picker: reveal the queue as soon as the URLs are being added.
    setFormOpen(false)

    const matched: { url: string; plugin: PluginInfo }[] = []
    const unmatched: string[] = []
    for (const url of list) {
      const plugin = findMatchingPlugin(downloadPlugins.data, url)
      if (plugin) matched.push({ url, plugin })
      else unmatched.push(url)
    }
    setUnmatchedUrls(unmatched)
    if (unmatched.length > 0) {
      // The UI already tells the user; this makes the same fact visible in the report window, where
      // "every URL says no plugin matched" plus the installed namespaces answers *why* in one look.
      // Hosts only, never the paths: an unmatched URL is exactly the kind of thing whose path can
      // be a private gallery link.
      reportWarning("upload: no installed download plugin matched the URL", {
        unmatched: unmatched.length,
        hosts: [...new Set(unmatched.map((u) => hostnameOfUrl(u)))],
        installed: (downloadPlugins.data ?? []).map((p) => p.namespace),
      })
    }
    if (matched.length === 0) return

    const items = matched.map(({ url, plugin }) => ({
      url,
      plugin_namespace: plugin.namespace,
      category: category || undefined,
      metadataNamespace: findPluginByDomain(metadataPlugins.data, url)?.namespace,
    }))
    await resolveDefaultsAndAdd(items)
    setUrls("")
  }

  // Hooks can't be called conditionally/in a loop, so the per-plugin defaults are fetched here —
  // but once per *namespace*, not once per URL: a 310-URL paste used to issue ~3 requests per line
  // before the single queue POST, which is why a ten-line paste took seconds to render.
  async function resolveDefaultsAndAdd(
    items: Array<{
      url: string
      plugin_namespace: string
      category?: string
      metadataNamespace?: string
    }>,
  ) {
    const metadataNamespaces = [
      ...new Set(items.map((item) => item.metadataNamespace).filter((ns): ns is string => !!ns)),
    ]

    // `overwrite_on_duplicate` is deliberately absent: since 020 an archive is addressed by its
    // content, so a download never collides with an existing name — the flag (and its global
    // `replacedupe` default) could not change any outcome.
    const metadataSettings = await Promise.all(
      metadataNamespaces.map((ns) =>
        fetchJson<{ enabled: boolean }>(
          `/plugins/settings?namespace=${encodeURIComponent(ns)}`,
        ).catch(() => null),
      ),
    )
    const metadataEnabled = new Map(metadataNamespaces.map((ns, i) => [ns, metadataSettings[i]?.enabled ?? false]))

    const resolved = items.map((item) => ({
      url: item.url,
      plugin_namespace: item.plugin_namespace,
      category: item.category,
      auto_fetch_metadata: item.metadataNamespace
        ? (metadataEnabled.get(item.metadataNamespace) ?? false)
        : false,
    }))
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

  const leftColumn = (
    <div className="left-column">
      {t("upload.addUploadedFilesToCategory")}
      <select id="category" className="favtag-btn" value={category} onChange={(e) => setCategory(e.target.value)}>
        <option value="">{t("common.NoCategory")}</option>
        {sortCategories(categories.data ?? []).map((c) => (
          <option key={c.id} value={c.id}>
            {c.name}
          </option>
        ))}
      </select>
      <Tooltip
        label={t("common.newCategory") ?? undefined}
        // Middle of the select's row, not its text baseline, and above the drawer it may sit in.
        wrapperStyle={{ verticalAlign: "middle" }}
        zIndex={narrow ? Z_OVERLAY_ABOVE_DRAWER : undefined}
      >
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
          accept={UPLOAD_ACCEPT}
          disabled={uploadingCount > 0}
          onChange={(e) => {
            const chosen = e.target.files?.[0] ?? null
            if (chosen) void handleUpload(chosen)
          }}
        />
      </span>

      {uploadProgress && uploadProgress.total > 0 && (
        <div style={{ display: "flex", alignItems: "center", gap: 6, marginTop: 8, maxWidth: 320, marginLeft: "auto", marginRight: "auto" }}>
          <div style={{ flex: 1, height: 8, background: "rgba(128,128,128,0.25)", borderRadius: 4, overflow: "hidden" }}>
            <div
              style={{
                width: `${Math.min(100, (uploadProgress.loaded / uploadProgress.total) * 100)}%`,
                height: "100%",
                background: STATE_COLOR.active,
              }}
            />
          </div>
          <span style={{ fontSize: FONT_SIZE_XS, whiteSpace: "nowrap" }}>
            {formatBytes(uploadProgress.loaded)} / {formatBytes(uploadProgress.total)} (
            {Math.round((uploadProgress.loaded / uploadProgress.total) * 100)}%)
          </span>
        </div>
      )}

      {resumedFrom !== null && (
        <p style={{ fontSize: FONT_SIZE_XS, opacity: 0.72, marginTop: 4, textAlign: "center" }}>
          {t("upload.resumingFrom", { size: formatBytes(resumedFrom) })}
        </p>
      )}

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
        onChange={(e) => {
          if (composingFrom.current !== null) setUrls(e.target.value)
          else commitUrls(e.target, urls)
        }}
        onCompositionStart={() => {
          composingFrom.current = urls
        }}
        onCompositionEnd={(e) => {
          const before = composingFrom.current ?? urls
          composingFrom.current = null
          commitUrls(e.currentTarget, before)
        }}
        onPaste={(e) => {
          const raw = e.clipboardData.getData("text")
          if (!raw.trim()) return
          const insert = urlLinesForInsert(raw)
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
  )

  return (
    <div className="ido" style={{ textAlign: "center", fontSize: FONT_SIZE_XS }}>
      <h1 className="ih" style={{ textAlign: "center" }}>
        {t("upload.addingArchivesToTheLibrary")}
      </h1>

      {t("upload.addFilesToYourLanrurugi")}
      <br />
      <button type="button" className="stdbtn" style={{ margin: "8px 0" }} onClick={() => setImportOpen(true)}>
        <i className="fa fa-share-nodes" aria-hidden="true"></i> {t("share.import.button")}
      </button>
      {importOpen && (
        <Suspense fallback={null}>
          <ImportShareModal onClose={() => setImportOpen(false)} />
        </Suspense>
      )}
      <br />

      <div style={{ marginLeft: "auto", marginRight: "auto" }}>
        {narrow ? (
          <Drawer
            open={formOpen}
            onClose={() => setFormOpen(false)}
            title={t("upload.addingArchivesToTheLibrary") ?? undefined}
          >
            {/* `Drawer` lays its body out left-aligned (Tailwind `text-left`); this column is the
                page's centred form, so restore the centring it has when rendered inline. */}
            <div style={{ textAlign: "center" }}>{leftColumn}</div>
          </Drawer>
        ) : (
          leftColumn
        )}

        <div
          className="right-column"
          style={{ paddingLeft: narrow ? 0 : 24, boxSizing: "border-box" }}
        >
          <SubscriptionStatus
            leading={
              narrow ? (
                // Same round icon pill as the subscription buttons beside it: on a phone the add
                // form lives in a drawer, and this is the control that opens it.
                <button
                  type="button"
                  className="stdbtn pill-btn modal-close-btn"
                  style={{ height: 26 }}
                  onClick={() => setFormOpen(true)}
                >
                  <i className="fa fa-plus" aria-hidden="true"></i>
                  <span>{t("upload.add")}</span>
                </button>
              ) : undefined
            }
          />
          <DownloadQueuePanel downloadPlugins={downloadPlugins.data} metadataPlugins={metadataPlugins.data} />
          <ReservationGroup />
        </div>
      </div>

      <br />
      <br />
      <input type="button" id="return" className="stdbtn" value={t("common.returnToLibrary") ?? undefined} onClick={() => navigate(routes.library())} />
    </div>
  )
}
