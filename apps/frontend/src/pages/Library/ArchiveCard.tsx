import type { MouseEvent } from "react"
import { useState } from "react"
import { useTranslation } from "react-i18next"

import type { ArchiveMetadata } from "@/api/types"
import { routes } from "@/lib/routes"
import { highlightText } from "@/lib/utils/highlightText"
import { isTankoubonId } from "@/lib/utils/isTankoubonId"

import { TagLine } from "./TagLine"

/** Status badges — 🆕 and 👑 are mutually exclusive (a Tankoubon can show both plus 📚); read
 * threshold is >85%. */
function StatusIcons({ archive }: { archive: ArchiveMetadata }) {
  const { t } = useTranslation()
  const isTank = isTankoubonId(archive.arcid)
  const isRead = archive.pagecount > 0 && archive.progress / archive.pagecount > 0.85
  const showNew = archive.isnew
  const showCrown = isRead && (isTank || !showNew)
  // No legacy equivalent, so no mutual-exclusion with the other badges.
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

/** A Tankoubon shows the 3-part `progress/pagecount/archive_count` form; a plain archive shows
 * the 2-part form. */
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

/** Mirrors legacy's thumbnail card markup so the copied theme CSS styles it identically.
 * Right-click opens `ArchiveContextMenu`; multi-select mode overlays a checkbox instead. */
export function ArchiveCard({
  archive,
  multiSelect,
  selected,
  cropThumbs,
  onToggleSelect,
  onContextMenu,
  onOpen,
  onSearchTag,
  highlightQuery,
}: {
  archive: ArchiveMetadata
  multiSelect: boolean
  selected: boolean
  cropThumbs: boolean
  onToggleSelect: (id: string) => void
  onContextMenu: (e: MouseEvent, archive: ArchiveMetadata) => void
  onOpen: (id: string) => void
  onSearchTag: (namespacedTag: string) => void
  /** Space-separated keywords to `<mark>` inside the title, e.g. from a search box. */
  highlightQuery?: string
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
          {highlightText(archive.title, highlightQuery)}
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
