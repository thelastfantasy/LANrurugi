import { type ReactNode, useCallback, useEffect, useMemo, useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import { Select } from "@/components/common-ui/Form"
import { useReaderSettings } from "@/hooks/useReaderSettings"
import { fetchContentLengthKb, fetchResizedPageInfo, type ResizedPageInfo } from "@/lib/utils/imageMeta"
import { displayTocName } from "@/lib/utils/tocValidation"
import { chapterForPage, tocChapterSpans } from "@/pages/Reader/ArchiveOverviewOverlay/shared"

import type { ShareTransport } from "./transport"
import type { ArchiveEntry } from "./types"

interface PageHead {
  resized: ResizedPageInfo | null
  sizeKb: number | null
}

const formatKb = (kb: number) => (kb >= 1024 ? `${(kb / 1024).toFixed(1)} MB` : `${Math.round(kb)} KB`)

/** Bytes the browser actually received for `url`, from Resource Timing; `null` when it came from
 * cache (an encoded size of 0) or the entry is gone. */
function servedKb(url: string): number | null {
  const entries = performance.getEntriesByName(new URL(url, window.location.href).href) as PerformanceResourceTiming[]
  const size = entries.at(-1)?.encodedBodySize ?? 0
  return size > 0 ? Math.round(size / 1024) : null
}

/** A read-only reader over a share transport: one page at a time, with no bookmarks, progress or
 * admin actions. Keys follow the library reader, and its right-to-left setting (kept in this
 * browser only) applies here too. */
export function ShareReader({
  transport,
  entry,
  onClose,
  initialPage = 0,
  onPageChange,
  volume,
}: {
  transport: ShareTransport
  entry: ArchiveEntry
  onClose: () => void
  /** 0-based page to open at, e.g. from the address. */
  initialPage?: number
  onPageChange?: (index: number) => void
  /** A tankoubon member: its 1-based place among `total` volumes, and how to open another. */
  volume?: { current: number; total: number; go: (n: number) => void }
}) {
  const { t } = useTranslation()
  const [settings, updateSettings] = useReaderSettings()
  const rtl = settings.mangaMode
  const [pages, setPages] = useState<string[] | null>(null)
  const [failed, setFailed] = useState(false)
  const [index, setIndex] = useState(initialPage)
  const [showKeys, setShowKeys] = useState(false)
  const [loaded, setLoaded] = useState<{ index: number; width: number; height: number; kb: number | null } | null>(null)
  const heads = useRef(new Map<string, Promise<PageHead>>())
  const [head, setHead] = useState<{ url: string; head: PageHead } | null>(null)

  useEffect(() => {
    let cancelled = false
    transport
      .pages(entry.archive_id)
      .then((urls) => {
        if (cancelled) return
        setPages(urls)
        // An address may name a page past the end (a stale link): stay on the last one.
        setIndex((i) => Math.min(Math.max(i, 0), Math.max(urls.length - 1, 0)))
      })
      .catch(() => !cancelled && setFailed(true))
    return () => {
      cancelled = true
    }
  }, [transport, entry.archive_id])

  const count = pages?.length ?? 0
  const goTo = useCallback((target: number) => setIndex(Math.min(Math.max(target, 0), Math.max(count - 1, 0))), [count])
  // The sharer's chapters (1-based first pages), and the one this page is in.
  const chapters = useMemo(() => tocChapterSpans(entry.toc ?? [], count || entry.pagecount), [entry.toc, count, entry.pagecount])
  const chapter = chapterForPage(chapters, index + 1)
  const go = useCallback((delta: number) => setIndex((i) => Math.min(Math.max(i + delta, 0), Math.max(count - 1, 0))), [count])
  /** A step towards the screen's left or right edge: backwards or forwards depending on direction. */
  const toward = useCallback((side: "left" | "right", jump: boolean) => {
    const forward = (side === "right") !== rtl
    if (jump) goTo(forward ? count - 1 : 0)
    else go(forward ? 1 : -1)
  }, [rtl, go, goTo, count])
  const toggleFullscreen = () => {
    if (document.fullscreenElement) void document.exitFullscreen()
    else void document.documentElement.requestFullscreen?.().catch(() => {})
  }

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const target = e.target as HTMLElement | null
      if (target?.closest("input, textarea, select, [contenteditable]")) return
      if (e.ctrlKey || e.metaKey || e.altKey) return
      switch (e.key) {
        case "ArrowLeft":
        case "a":
        case "A":
          toward("left", e.shiftKey)
          break
        case "ArrowRight":
        case "d":
        case "D":
          toward("right", e.shiftKey)
          break
        case " ":
        case "PageDown":
        case "j":
          go(1)
          break
        case "PageUp":
        case "k":
          go(-1)
          break
        case "Home":
          goTo(0)
          break
        case "End":
          goTo(count - 1)
          break
        case "m":
          updateSettings({ mangaMode: !rtl })
          break
        case "f":
          toggleFullscreen()
          break
        case "Escape":
          onClose()
          break
        default:
          return
      }
      e.preventDefault()
    }
    window.addEventListener("keydown", onKey)
    return () => window.removeEventListener("keydown", onKey)
  }, [toward, go, goTo, count, rtl, updateSettings, onClose])

  const url = pages?.[index]

  // The address follows the page, so a reload or a shared link opens here.
  const pageChanged = useRef(onPageChange)
  useEffect(() => {
    pageChanged.current = onPageChange
  })
  useEffect(() => {
    if (pages) pageChanged.current?.(index)
  }, [pages, index])

  // Preload the next page so turning forward feels instant.
  useEffect(() => {
    if (pages && index + 1 < pages.length) new Image().src = pages[index + 1]
  }, [pages, index])

  // What the server did with this page (format, original size), as the library reader shows it.
  useEffect(() => {
    if (!url) return
    let request = heads.current.get(url)
    if (!request) {
      const absolute = new URL(url, window.location.href).toString()
      request = Promise.all([fetchResizedPageInfo(absolute), fetchContentLengthKb(absolute)]).then(([resized, sizeKb]) => ({
        resized,
        sizeKb,
      }))
      heads.current.set(url, request)
    }
    let cancelled = false
    void request.then((h) => !cancelled && setHead({ url, head: h }))
    return () => {
      cancelled = true
    }
  }, [url])

  const name = url ? (new URL(url, window.location.href).searchParams.get("path") ?? "") : ""
  let info: ReactNode = name
  const page = loaded?.index === index ? loaded : null
  // `head` is null until the first answer, and so is `url` until the page list loads: compare only
  // when both exist, or `undefined === undefined` reads into a null `head`.
  const pageHead = head && url && head.url === url ? head.head : null
  if (page && pageHead?.resized) {
    const r = pageHead.resized
    const origKb = r.origSizeBytes / 1024
    info = (
      <>
        <span className="file-info-opt">
          {name} → {r.servedFormat.toUpperCase()} :: {page.width} x {page.height}
          {page.kb !== null && ` :: ${page.kb} KB`}
        </span>
        {" · "}
        <span className="file-info-orig">
          {t("reader.original")}: {name.split(".").pop()} {r.origWidth} x {r.origHeight} :: {formatKb(origKb)}
          {page.kb !== null &&
            ` · ${t("reader.savedPct", { pct: Math.max(0, Math.round((1 - (page.kb * 1024) / r.origSizeBytes) * 100)) })}`}
        </span>
      </>
    )
  } else if (page) {
    const kb = page.kb ?? pageHead?.sizeKb ?? null
    info = `${name} :: ${page.width} x ${page.height}${kb !== null ? ` :: ${kb} KB` : ""}`
  }

  // A webtoon strip is far taller than a page; fitting it to the screen would shrink it to
  // illegibility, so those scroll inside the stage while a normal page always fits (phones).
  const tall = !!page && page.height > page.width * 2.2

  return (
    <div className="ido share-reader">
      <div className="share-reader-toolbar">
        <button type="button" className="stdbtn" onClick={onClose}>
          {t("share.action.close")}
        </button>
        <button type="button" className="stdbtn" disabled={index === 0} onClick={() => go(-1)}>
          {t("share.action.prev")}
        </button>
        <span className="share-reader-counter">{count > 0 ? `${index + 1} / ${count}` : ""}</span>
        <button type="button" className="stdbtn" disabled={index + 1 >= count} onClick={() => go(1)}>
          {t("share.action.next")}
        </button>
      </div>
      <div className="share-reader-stage" data-tall={tall ? "1" : undefined}>
        {failed && <p>{t("share.page.readFailed")}</p>}
        {!failed && !pages && <i className="fa fa-3x fa-cog fa-spin" aria-hidden="true"></i>}
        {url && (
          <img
            className="share-reader-page"
            src={url}
            alt={`${index + 1}`}
            referrerPolicy="no-referrer"
            onLoad={(e) => {
              const img = e.currentTarget
              setLoaded({ index, width: img.naturalWidth, height: img.naturalHeight, kb: servedKb(url) })
            }}
            onClick={(e) => {
              const rect = e.currentTarget.getBoundingClientRect()
              toward(e.clientX - rect.left > rect.width / 2 ? "right" : "left", false)
            }}
          />
        )}
      </div>
      {showKeys && <p className="share-result-note">{t("share.reader.keys")}</p>}
      <div className="share-reader-below">
        <div className="share-reader-controls">
          {volume && volume.total > 1 && (
            <>
              <button
                type="button"
                className="stdbtn"
                disabled={volume.current <= 1}
                onClick={() => volume.go(volume.current - 1)}
              >
                {t("share.reader.prevVolume")}
              </button>
              <span className="share-reader-counter">
                {t("share.reader.volume", { current: volume.current, total: volume.total })}
              </span>
              <button
                type="button"
                className="stdbtn"
                disabled={volume.current >= volume.total}
                onClick={() => volume.go(volume.current + 1)}
              >
                {t("share.reader.nextVolume")}
              </button>
            </>
          )}
          {chapters.length > 0 && (
            <Select
              value={chapter ? String(chapter.page) : ""}
              onValueChange={(page) => page && goTo(Number(page) - 1)}
              ariaLabel={t("share.reader.chapters")}
              variant="stdbtn"
              align="start"
              items={[
                ...(chapter ? [] : [{ value: "", label: "—" }]),
                ...chapters.map((c) => ({ value: String(c.page), label: `${displayTocName(c.name, t)} · ${c.page}` })),
              ]}
            />
          )}
          <button
            type="button"
            className="stdbtn share-reader-toggle"
            aria-pressed={rtl}
            aria-label={t("reader.rightToLeft")}
            title={t("reader.mToggleMangaModeRighttoleft")}
            onClick={() => updateSettings({ mangaMode: !rtl })}
          >
            <i className="fa fa-right-left" aria-hidden="true"></i>
            {/* The label stays where there is room for it; on a phone the icon and its tooltip
                carry the same meaning (the accessible name is on the button either way). */}
            <span className="share-reader-toggle-label">{t("reader.rightToLeft")}</span>
          </button>
          <button
            type="button"
            className="stdbtn share-reader-toggle"
            aria-pressed={showKeys}
            aria-label={t("reader.help")}
            title={t("reader.help")}
            onClick={() => setShowKeys((v) => !v)}
          >
            <i className="fa fa-keyboard" aria-hidden="true"></i>
          </button>
        </div>
        <span className="share-reader-title" title={entry.title}>
          {entry.title}
        </span>
        {url && (
          <div className="file-info share-reader-info" title={typeof info === "string" ? info : undefined}>
            {info}
          </div>
        )}
      </div>
    </div>
  )
}
