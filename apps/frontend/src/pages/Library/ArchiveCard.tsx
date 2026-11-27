import type { TFunction } from "i18next"
import type { MouseEvent } from "react"
import { useRef, useState } from "react"
import { useTranslation } from "react-i18next"

import type { ArchiveMetadata } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"
import { isPlainLinkClick,Link } from "@/components/common-ui/Link"
import { FormatBadge, StatusIcon } from "@/components/Display"
import { useLongPress } from "@/hooks/useLongPress"
import { routes } from "@/lib/routes"
import { isBookExtension } from "@/lib/utils/bookFormats"
import { coverUrl } from "@/lib/utils/coverUrl"
import { highlightText } from "@/lib/utils/highlightText"
import { isTankoubonId } from "@/lib/utils/isTankoubonId"

import { type MatchReasonMember,memberLabel, memberParts } from "./matchReason"
import { TagLine } from "./TagLine"
import type { ContextMenuTrigger } from "./types"

const gap = { marginLeft: 2 }

/** File format (e-books only) and status badges — new and read are mutually exclusive (a
 * Tankoubon can show both plus its own mark); read threshold is >85%. */
function StatusIcons({ archive }: { archive: ArchiveMetadata }) {
  const { t } = useTranslation()
  const isTank = isTankoubonId(archive.arcid)
  const isRead = archive.pagecount > 0 && archive.progress / archive.pagecount > 0.85
  const showNew = archive.isnew
  const showCrown = isRead && (isTank || !showNew)
  // No legacy equivalent, so no mutual-exclusion with the other badges.
  const showPatch = archive.has_patch === true
  const damaged = archive.damaged === true

  const isBook = isBookExtension(archive.extension)
  if (!showNew && !showCrown && !isTank && !showPatch && !isBook && !damaged) return null
  return (
    // Inline flow rather than flex: the row aligns this box by its first baseline, and the marks
    // centre on the text's middle like any inline icon.
    <div className="status-icons" style={{ flexShrink: 0, whiteSpace: "nowrap" }}>
      {damaged && <StatusIcon kind="damaged" title={t("library.damaged") ?? undefined} style={gap} />}
      {isBook && <FormatBadge extension={archive.extension} style={gap} />}
      {showNew && <StatusIcon kind="new" title={t("library.new") ?? undefined} style={gap} />}
      {showCrown && <StatusIcon kind="read" title={t("common.read") ?? undefined} style={gap} />}
      {isTank && <StatusIcon kind="tankoubon" title={t("library.tankoubon") ?? undefined} style={gap} />}
      {showPatch && <StatusIcon kind="patch" title={t("library.hasAPagePatch") ?? undefined} style={gap} />}
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

/** Why a Tankoubon is in this result set: its own record did not match, these members did.
 *  Naming them beats a generic "matched via members" — the whole point is which volumes. */
function matchReasonText(
  members: MatchReasonMember[],
  t: TFunction,
): string {
  const names = members.map(memberLabel).join("、")
  return t("library.tankMatchedThroughMembers", { members: names })
}

/** The same as a tooltip: the explanation, then one volume per line — a run-on list of long
 *  titles joined by 、 was unreadable. */
function MatchReason({ members, t }: { members: MatchReasonMember[]; t: TFunction }) {
  return (
    <div style={{ textAlign: "left" }}>
      <div>{t("library.tankMatchedThroughMembersHeading")}</div>
      <ul style={{ margin: "4px 0 0", paddingLeft: "1.2em" }}>
        {members.map((m, i) => {
          const parts = memberParts(m)
          return (
          <li key={i} style={{ listStyle: "disc" }}>
            {/* The archive's own name is bold — it is the one thing a reader is looking for;
                the chapter name the tankoubon gave it follows in normal weight. */}
            <strong>{parts.lead}</strong>
            {parts.rest ? ` — ${parts.rest}` : null}
          </li>
          )
        })}
      </ul>
    </div>
  )
}

/** Thumbnail URLs that have loaded at least once this session. A remounted card (the carousel
 * collapsed and reopened, a page revisited) starts from these instead of flashing the "wait warmly"
 * placeholder over an image the browser already has. */
const loadedThumbs = new Set<string>()

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
  onContextMenu: (e: ContextMenuTrigger, archive: ArchiveMetadata) => void
  onOpen: (id: string) => void
  onSearchTag: (namespacedTag: string) => void
  /** Space-separated keywords to `<mark>` inside the title, e.g. from a search box. */
  highlightQuery?: string
}) {
  const { t } = useTranslation()
  const id = archive.arcid
  const thumbSrc = coverUrl(archive)
  const [thumbLoaded, setThumbLoaded] = useState(() => loadedThumbs.has(thumbSrc))
  const [thumbFailed, setThumbFailed] = useState(false)
  // Touch opens the menu on our own long-press timing; right-click stays on `onContextMenu`.
  const [cardEl, setCardEl] = useState<HTMLDivElement | null>(null)
  useLongPress(cardEl, (_, e) => onContextMenu(e, archive), { blockContextMenu: () => true })
  // A title cut off at two lines is shown whole, laid over the cover, while the card is hovered
  // (mouse) or pressed (touch) anywhere but its tag line. `null` = folded; else the card's own
  // background, so the overlay matches every theme and the selected-card highlight.
  const titleRef = useRef<HTMLAnchorElement | null>(null)
  const [fullTitleBg, setFullTitleBg] = useState<string | null>(null)
  function showFullTitle(e: React.PointerEvent) {
    const title = titleRef.current
    if (!cardEl || !title || (e.target as Element).closest(".id4")) {
      setFullTitleBg(null)
      return
    }
    if (fullTitleBg === null && title.scrollHeight > title.clientHeight + 1) {
      setFullTitleBg(getComputedStyle(cardEl).backgroundColor)
    }
  }
  const hideFullTitle = () => setFullTitleBg(null)

  function handleOpen(e: MouseEvent) {
    // Let the browser keep its own link gestures — Ctrl/Cmd/Shift for a new tab/window, Alt for
    // Firefox's save-link — and ignore a click that only ended a text selection, which is what
    // Alt-dragging part of the title produces.
    if (!isPlainLinkClick(e)) return
    e.preventDefault()
    if (multiSelect) {
      onToggleSelect(id)
    } else {
      onOpen(id)
    }
  }


  return (
    <div
      ref={setCardEl}
      className={`id1${selected ? " msm-selected" : ""}`}
      id={id}
      onContextMenu={(e) => onContextMenu(e, archive)}
      onPointerMove={(e) => e.pointerType === "mouse" && showFullTitle(e)}
      onPointerLeave={(e) => e.pointerType === "mouse" && hideFullTitle()}
      onPointerDown={(e) => e.pointerType !== "mouse" && showFullTitle(e)}
      onPointerUp={(e) => e.pointerType !== "mouse" && hideFullTitle()}
      onPointerCancel={hideFullTitle}
    >
      <div
        className="id2"
        style={{ display: "flex", alignItems: "baseline", justifyContent: "center", gap: 2, position: "relative" }}
      >
        {fullTitleBg !== null && (
          // An `a` so the theme's `div.id2 a` colour and weight apply; the clamp is undone inline.
          <a
            aria-hidden="true"
            style={{
              position: "absolute",
              top: 0,
              left: 0,
              right: 0,
              zIndex: 2,
              display: "block",
              overflow: "visible",
              WebkitLineClamp: "unset",
              background: fullTitleBg,
              pointerEvents: "none",
            }}
          >
            {highlightText(archive.title, highlightQuery)}
          </a>
        )}
        <Link
          ref={titleRef}
          to={routes.reader(id)}
          title={archive.title}
          onClick={handleOpen}
          style={{ flex: "0 1 auto", minWidth: 0 }}
        >
          {highlightText(archive.title, highlightQuery)}
        </Link>
        <StatusIcons archive={archive} />
        {/* `openOnTap`: the icon sits inside the card link, whose pointer cursor it inherits, but is
            decorative itself — a tap can only mean "show me the reason". */}
        {archive.match_reason?.kind === "members" && (
          <Tooltip openOnTap label={<MatchReason members={archive.match_reason.members} t={t} />}>
            <span
              className="match-reason-icon"
              // Focusable so the tooltip is reachable without a mouse; the icon itself is
              // decorative and the text lives in the tooltip.
              tabIndex={0}
              role="img"
              aria-label={matchReasonText(archive.match_reason.members, t)}
            >
              <i className="fa fa-circle-info" aria-hidden="true"></i>
            </span>
          </Tooltip>
        )}
      </div>
      {/* A damaged file is greyed out: it is in the library but cannot be read (#291). */}
      <div
        className={cropThumbs ? "id3" : "id3 nocrop"}
        style={{ position: "relative", opacity: archive.damaged ? 0.5 : undefined }}
      >
        {multiSelect && (
          <input
            type="checkbox"
            checked={selected}
            onChange={() => onToggleSelect(id)}
            style={{ position: "absolute", top: 6, left: 6, zIndex: 1, width: 20, height: 20 }}
          />
        )}
        <Link to={routes.reader(id)} title={archive.title} onClick={handleOpen}>
          {!thumbLoaded && !thumbFailed && (
            <>
              <img style={{ position: "relative" }} src="/legacy/img/wait_warmly.jpg" alt="" />
              <i className="fa fa-4x fa-cog fa-spin ttspinner" aria-hidden="true"></i>
            </>
          )}
          <img
            src={thumbFailed ? "/legacy/img/noThumb.png" : thumbSrc}
            alt={archive.title}
            // `decoding="async"` only: decoding off the main thread keeps one large JPEG/WebP from
            // stalling the images behind it. Deliberately *not* `loading="lazy"` — this element is
            // `display: none` until it has loaded, and a hidden lazy image is exactly the case
            // browsers defer indefinitely, which left cards spinning instead of loading.
            decoding="async"
            style={thumbLoaded || thumbFailed ? undefined : { display: "none" }}
            onLoad={() => {
              loadedThumbs.add(thumbSrc)
              setThumbLoaded(true)
            }}
            onError={() => setThumbFailed(true)}
          />
        </Link>
      </div>
      <div className="id4">
        <PageCountBadge archive={archive} />
        <TagLine tags={archive.tags} onSearchTag={onSearchTag} />
      </div>
    </div>
  )
}
