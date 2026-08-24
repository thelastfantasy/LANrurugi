import type { MouseEvent } from "react"
import { useState } from "react"
import { useTranslation } from "react-i18next"

import type { ArchiveMetadata } from "@/api/types"
import { routes } from "@/lib/routes"
import { isTankoubonId } from "@/lib/utils/isTankoubonId"

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
  // Additive, LANrurugi-only badge (issue #77's own follow-on design) — a sidecar `.patch.zip`
  // exists next to this archive, so what the reader shows for it differs from the raw file on
  // disk. No legacy equivalent, so no mutual-exclusion rule with the other three (unlike 🆕/👑,
  // which are deliberately XOR) — a patched archive can be new/read/a Tankoubon at the same time.
  const showPatch = archive.has_patch === true

  if (!showNew && !showCrown && !isTank && !showPatch) return null
  return (
    <div className="status-icons" style={{ display: "flex", gap: 1, flexShrink: 0, paddingTop: 2 }}>
      {showNew && <span title={t("library.new") ?? undefined} style={{ fontSize: "0.8em" }}>🆕</span>}
      {showCrown && <span title={t("common.read") ?? undefined} style={{ fontSize: "0.8em" }}>👑</span>}
      {isTank && <span title={t("library.tankoubon") ?? undefined} style={{ fontSize: "0.8em" }}>📚</span>}
      {showPatch && <span title={t("library.hasAPagePatch") ?? undefined} style={{ fontSize: "0.8em" }}>🩹</span>}
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
      <sup title={(isTank ? t("library.tankoubonPageCount") : t("library.pageCount")) ?? undefined}>
        {isTank
          ? `${archive.progress}/${archive.pagecount}/${archive.archive_count}`
          : `${archive.progress}/${archive.pagecount}`}
      </sup>
    </div>
  )
}

/** Mirrors legacy's exact thumbnail card markup (`buildThumbnailDiv` in
 * `~/LANraragi/public/js/mod/common.js`) — `div.id1` > (`div.id2` status icons + title, `div.id3`
 * cover image, `div.id4` page count + tags) — so the copied theme CSS (`useApplyTheme`) styles it
 * identically. Right-click opens `ArchiveContextMenu` (real functional parity); multi-select mode
 * overlays a checkbox instead of navigating on click. */
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
      <div className="id2" style={{ display: "flex", alignItems: "flex-start", justifyContent: "center", gap: 2 }}>
        <a
          href={routes.reader(id)}
          title={archive.title}
          onClick={handleOpen}
          style={{ flex: "0 1 auto", minWidth: 0 }}
        >
          {archive.title}
        </a>
        <StatusIcons archive={archive} />
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
      </div>
      <div className="id4">
        <PageCountBadge archive={archive} />
        <TagLine tags={archive.tags} onSearchTag={onSearchTag} />
      </div>
    </div>
  )
}
