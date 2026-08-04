import type { MouseEvent } from "react"
import { useState } from "react"
import { useTranslation } from "react-i18next"

import type { ArchiveMetadata } from "@/api/types"
import { routes } from "@/lib/routes"
import { isTankoubonId } from "@/lib/utils/isTankoubonId"

import { BookmarkIcon } from "./BookmarkIcon"
import { TagLine } from "./TagLine"

/** Read-crown/new/tankoubon status badges — ports `buildStatusDiv` exactly, including its
 * mutual-exclusion rule (an archive shows 🆕 XOR 👑, never both; a Tankoubon can show both plus
 * 📚) and its >85%-read threshold. */
function StatusIcons({ archive }: { archive: ArchiveMetadata }) {
  const { t } = useTranslation()
  const isTank = isTankoubonId(archive.arcid)
  const isRead = archive.pagecount > 0 && archive.progress / archive.pagecount > 0.85
  const showNew = archive.isnew
  const showCrown = isRead && (isTank || !showNew)

  if (!showNew && !showCrown && !isTank) return null
  return (
    <div className="isnew status-icons">
      {showNew && <span title={t("New!") ?? undefined}>🆕</span>}
      {showCrown && <span title={t("Read") ?? undefined}>👑</span>}
      {isTank && <span title={t("Tankoubon") ?? undefined}>📚</span>}
    </div>
  )
}

/** Ports `buildPageCountDiv` — a Tankoubon with pages shows the 3-part `progress/pagecount/
 * archive_count` form (via its own `archive_count` field, populated server-side only for
 * synthetic Tankoubon search-result entries — see `search.rs`'s `resolve_search_entry`); a plain
 * archive shows the 2-part form; nothing renders when `pagecount` is 0. */
function PageCountBadge({ archive }: { archive: ArchiveMetadata }) {
  const { t } = useTranslation()
  if (archive.pagecount <= 0) return null
  const isTank = isTankoubonId(archive.arcid) && archive.archive_count != null
  return (
    <div className="isnew">
      <sup title={(isTank ? t("Tankoubon Page Count") : t("Page Count")) ?? undefined}>
        {isTank
          ? `${archive.progress}/${archive.pagecount}/${archive.archive_count}`
          : `${archive.progress}/${archive.pagecount}`}
      </sup>
    </div>
  )
}

/** Mirrors legacy's exact thumbnail card markup (`buildThumbnailDiv` in
 * `~/LANraragi/public/js/mod/common.js`) — `div.id1` > (`div.id2` status icons + title, `div.id3`
 * cover image + bookmark icon, `div.id4` page count + tags) — so the copied theme CSS
 * (`useApplyTheme`) styles it identically. Right-click opens `ArchiveContextMenu` (real functional
 * parity); multi-select mode overlays a checkbox instead of navigating on click. */
export function ArchiveCard({
  archive,
  multiSelect,
  selected,
  cropThumbs,
  onToggleSelect,
  onContextMenu,
  onOpen,
  onSearchTag,
}: {
  archive: ArchiveMetadata
  multiSelect: boolean
  selected: boolean
  cropThumbs: boolean
  onToggleSelect: (id: string) => void
  onContextMenu: (e: MouseEvent, archive: ArchiveMetadata) => void
  onOpen: (id: string) => void
  onSearchTag: (namespacedTag: string) => void
}) {
  const id = archive.arcid
  const isTank = isTankoubonId(id)
  const [thumbLoaded, setThumbLoaded] = useState(false)
  const [thumbFailed, setThumbFailed] = useState(false)

  function handleOpen(e: MouseEvent) {
    e.preventDefault()
    if (multiSelect) {
      onToggleSelect(id)
    } else {
      onOpen(id)
    }
  }

  const thumbSrc = isTank
    ? `/api/tankoubons/${id}/thumbnail?no_fallback=true`
    : `/api/archives/${id}/thumbnail?no_fallback=true`

  return (
    <div
      className={`id1${selected ? " msm-selected" : ""}`}
      id={id}
      onContextMenu={(e) => onContextMenu(e, archive)}
    >
      <div className="id2">
        <StatusIcons archive={archive} />
        <a href={routes.reader(id)} title={archive.title} onClick={handleOpen}>
          {archive.title}
        </a>
      </div>
      <div className={cropThumbs ? "id3" : "id3 nocrop"} style={{ position: "relative" }}>
        {multiSelect && (
          <input
            type="checkbox"
            checked={selected}
            onChange={() => onToggleSelect(id)}
            style={{ position: "absolute", top: 6, left: 6, zIndex: 1, width: 20, height: 20 }}
          />
        )}
        <a href={routes.reader(id)} title={archive.title} onClick={handleOpen}>
          {!thumbLoaded && !thumbFailed && (
            <>
              <img style={{ position: "relative" }} src="/legacy/img/wait_warmly.jpg" alt="" />
              <i className="fa fa-4x fa-cog fa-spin ttspinner" aria-hidden="true"></i>
            </>
          )}
          <img
            src={thumbFailed ? "/legacy/img/noThumb.png" : thumbSrc}
            alt={archive.title}
            style={thumbLoaded || thumbFailed ? undefined : { display: "none" }}
            onLoad={() => setThumbLoaded(true)}
            onError={() => setThumbFailed(true)}
          />
        </a>
        {!isTank && <BookmarkIcon archiveId={id} />}
      </div>
      <div className="id4">
        <PageCountBadge archive={archive} />
        <TagLine tags={archive.tags} onSearchTag={onSearchTag} />
      </div>
    </div>
  )
}
