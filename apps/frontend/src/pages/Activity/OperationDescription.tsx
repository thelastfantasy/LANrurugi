import { useTranslation } from "react-i18next"
import { Link } from "react-router-dom"

import type { ActivityEntry } from "@/api/types"
import { StarRatingDisplay } from "@/components/common-ui/Form"
import { parseRating } from "@/lib/utils/rating"

import { actionTypeLabel, hasNoTarget, isDeletionActionType, PLUGIN_TYPE_LABEL_KEYS, targetLink } from "./activityTarget"

/** `action_type` values whose `before`/`after` share a unified `{ name: string }` shape — these
 * get the "将 A 重命名为 B" two-value treatment instead of a plain single-title-link. */
const RENAME_ACTION_TYPES = new Set([
  "archive.rename",
  "tankoubon.rename",
  "token.rename",
  "download_queue.rename",
])

function readName(value: unknown): string | undefined {
  if (value && typeof value === "object" && "name" in value) {
    const name = (value as { name: unknown }).name
    return typeof name === "string" ? name : undefined
  }
  return undefined
}

/** `download_queue.add`/`download_queue.start`'s own `after.url` — shown alongside `target.label`
 * so a bare URL doesn't lose the human-readable name once one's known. */
function readUrl(value: unknown): string | undefined {
  if (value && typeof value === "object" && "url" in value) {
    const url = (value as { url: unknown }).url
    return typeof url === "string" ? url : undefined
  }
  return undefined
}

const DOWNLOAD_QUEUE_URL_ACTION_TYPES = new Set(["download_queue.add", "download_queue.start"])

/** `bookmark.add`/`bookmark.remove`'s own `after.page` — which specific page the bookmark was on. */
function readPage(value: unknown): number | undefined {
  if (value && typeof value === "object" && "page" in value) {
    const page = (value as { page: unknown }).page
    return typeof page === "number" ? page : undefined
  }
  return undefined
}

const BOOKMARK_ACTION_TYPES = new Set(["bookmark.add", "bookmark.remove"])

/** `archive.rating_update`'s own `before.rating`/`after.rating` — a bare `"rating:X"` tag string
 * or `null` (cleared/absent). */
function readRatingTag(value: unknown): string | null | undefined {
  if (value && typeof value === "object" && "rating" in value) {
    const rating = (value as { rating: unknown }).rating
    return typeof rating === "string" ? rating : null
  }
  return undefined
}

/** Short single-line summary of a metadata-update's `after` (title change + tag counts) for the
 * "操作内容" column. `undefined` lets the caller fall back to just the title. */
function metadataChangeSummary(
  t: (key: string, opts?: Record<string, unknown>) => string | null,
  before: unknown,
  after: unknown,
): string | undefined {
  if (!after || typeof after !== "object") return undefined
  const a = after as { title?: unknown; name?: unknown; tags_added?: unknown; tags_removed?: unknown }
  const b = before && typeof before === "object" ? (before as { title?: unknown; name?: unknown }) : {}
  const parts: string[] = []
  const titleChanged =
    (typeof a.title === "string" && typeof b.title === "string" && a.title !== b.title) ||
    (typeof a.name === "string" && typeof b.name === "string" && a.name !== b.name)
  if (titleChanged) parts.push(t("activity.metadataChangeTitle") ?? "标题")
  const addedCount = Array.isArray(a.tags_added) ? a.tags_added.length : 0
  const removedCount = Array.isArray(a.tags_removed) ? a.tags_removed.length : 0
  if (addedCount > 0) parts.push(t("activity.metadataTagsAdded", { count: addedCount }) ?? `添加了 ${addedCount} 个标签`)
  if (removedCount > 0) parts.push(t("activity.metadataTagsRemoved", { count: removedCount }) ?? `移除了 ${removedCount} 个标签`)
  return parts.length > 0 ? parts.join(", ") : undefined
}

/** A resource title, linked to its real page when `targetLink` has one, plain text otherwise. */
function TargetTitle({ entry, title }: { entry: ActivityEntry; title: string }) {
  if (isDeletionActionType(entry.action_type)) return <>{title}</>
  const href = targetLink(entry.target.kind, entry.target.id, entry.target.label, entry.after, entry.action_type)
  if (!href) return <>{title}</>
  // `target.exists === false`: keep the real `<Link>`/`href` (hover/copy-link still work), just
  // suppress the click's navigation — a deleted target would only ever 404.
  return (
    <Link
      to={href}
      style={{ textDecoration: entry.target.exists === false ? "line-through" : undefined }}
      onClick={(e) => {
        e.stopPropagation()
        if (entry.target.exists === false) e.preventDefault()
      }}
    >
      {title}
    </Link>
  )
}

/** `settings.update`/`plugin.*` entries have no per-resource title — these get the action's own
 * translated name linked to the specific accordion `targetLink` resolves for it. */
const SECTION_LINK_ONLY_KINDS = new Set(["settings", "plugin"])

function SectionLink({ entry }: { entry: ActivityEntry }) {
  const { t } = useTranslation()
  const label = actionTypeLabel(t, entry.action_type)
  const href = targetLink(entry.target.kind, entry.target.id, entry.target.label, entry.after)
  if (!href) return <>{label}</>
  return (
    <Link to={href} onClick={(e) => e.stopPropagation()}>
      {label}
    </Link>
  )
}

/** What the "操作内容" column shows for one entry: a fixed description for whole-database actions,
 * "A 重命名为 B" for renames, "已删除 X" for deletions, else the target's title, linked if possible. */
export function OperationDescription({ entry }: { entry: ActivityEntry }) {
  const { t } = useTranslation()

  if (hasNoTarget(entry.action_type)) {
    return <>{t(`activity.operationFixed.${entry.action_type}`, { defaultValue: entry.action_type }) ?? entry.action_type}</>
  }

  const rawTitle = entry.target.label ?? entry.target.id
  const title =
    entry.target.kind === "plugin" && rawTitle && rawTitle in PLUGIN_TYPE_LABEL_KEYS
      ? (t(PLUGIN_TYPE_LABEL_KEYS[rawTitle]) ?? rawTitle)
      : rawTitle
  if (!title) {
    if (entry.target.kind && SECTION_LINK_ONLY_KINDS.has(entry.target.kind)) return <SectionLink entry={entry} />
    return <>—</>
  }

  if (RENAME_ACTION_TYPES.has(entry.action_type)) {
    const oldName = readName(entry.before)
    const newName = readName(entry.after) ?? title
    if (oldName && oldName !== newName) {
      return (
        <>
          {t("activity.renamePrefix", { from: oldName })}
          <TargetTitle entry={entry} title={newName} />
        </>
      )
    }
    return <TargetTitle entry={entry} title={newName} />
  }

  if (isDeletionActionType(entry.action_type)) {
    return <>{t("activity.deletedTarget", { title }) ?? `已删除 ${title}`}</>
  }

  if (DOWNLOAD_QUEUE_URL_ACTION_TYPES.has(entry.action_type)) {
    const url = readUrl(entry.after)
    if (url && url !== title) {
      return (
        <>
          <TargetTitle entry={entry} title={title} />{" "}
          <a href={url} target="_blank" rel="noreferrer" onClick={(e) => e.stopPropagation()} style={{ opacity: 0.65 }}>
            ({url})
          </a>
        </>
      )
    }
  }

  if (entry.action_type === "archive.rating_update" || entry.action_type === "tankoubon.rating_update") {
    const newRatingTag = readRatingTag(entry.after)
    const oldRatingTag = readRatingTag(entry.before)
    const newRating = newRatingTag ? parseRating(newRatingTag.slice("rating:".length)) : null
    const oldRating = oldRatingTag ? parseRating(oldRatingTag.slice("rating:".length)) : null
    if (newRating != null || oldRating != null) {
      return (
        <span style={{ display: "inline-flex", alignItems: "center", gap: 6 }}>
          <TargetTitle entry={entry} title={title} />
          {": "}
          {oldRating != null && (
            <>
              <StarRatingDisplay rating={oldRating} size={14} />
              <i className="fa fa-arrow-right" aria-hidden="true" style={{ fontSize: "0.75em", opacity: 0.65 }}></i>
            </>
          )}
          {newRating != null ? (
            <StarRatingDisplay rating={newRating} size={14} />
          ) : (
            t("activity.ratingRemoved") ?? "移除了评分"
          )}
        </span>
      )
    }
  }

  if (entry.action_type === "archive.metadata_update" || entry.action_type === "tankoubon.metadata_update") {
    const summary = metadataChangeSummary(t, entry.before, entry.after)
    if (summary) {
      return (
        <>
          <TargetTitle entry={entry} title={title} />
          {" — "}
          {summary}
        </>
      )
    }
  }

  if (BOOKMARK_ACTION_TYPES.has(entry.action_type)) {
    const page = readPage(entry.after)
    if (page != null) {
      return (
        <>
          <TargetTitle entry={entry} title={title} />
          {" — "}
          {t("bookmarks.pageLabel", { page })}
        </>
      )
    }
  }

  return <TargetTitle entry={entry} title={title} />
}
