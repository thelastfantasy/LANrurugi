import { useTranslation } from "react-i18next"
import { Link } from "react-router-dom"

import type { ActivityEntry } from "@/api/types"
import { StarRatingDisplay } from "@/components/common-ui/Form"
import { parseRating } from "@/lib/utils/rating"

import { actionTypeLabel, hasNoTarget, isDeletionActionType, PLUGIN_TYPE_LABEL_KEYS, targetLink } from "./activityTarget"

/** `action_type` values whose `before`/`after` both carry a unified `{ name: string }` shape (see
 * `archives.rs::rename_archive`'s own docs on why every rename-type write site now agrees on this
 * one field name) — these get the "将 A 重命名为 B" two-value treatment instead of the plain
 * single-title-link every other non-deletion action type uses. */
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

/** `download_queue.add`/`download_queue.start`'s own `after.url` — the item's real source link,
 * set unconditionally alongside `target.label` (which prefers an already-fetched title when one
 * exists, per `download_queue.rs::start_queue_item`'s own docs) so both can be shown together:
 * a title alone doesn't tell two different downloads apart, and a bare URL alone loses the
 * human-readable name once one's actually known. */
function readUrl(value: unknown): string | undefined {
  if (value && typeof value === "object" && "url" in value) {
    const url = (value as { url: unknown }).url
    return typeof url === "string" ? url : undefined
  }
  return undefined
}

const DOWNLOAD_QUEUE_URL_ACTION_TYPES = new Set(["download_queue.add", "download_queue.start"])

/** `bookmark.add`/`bookmark.remove`'s own `after.page` — the one piece of information a bare
 * archive title/link doesn't carry: which specific page the bookmark was on
 * (`bookmarks.rs::add_bookmark`/`remove_bookmark` both record it, per that module's own
 * `record_manual` calls). */
function readPage(value: unknown): number | undefined {
  if (value && typeof value === "object" && "page" in value) {
    const page = (value as { page: unknown }).page
    return typeof page === "number" ? page : undefined
  }
  return undefined
}

const BOOKMARK_ACTION_TYPES = new Set(["bookmark.add", "bookmark.remove"])

/** `archive.rating_update`'s own `before.rating`/`after.rating` — each is either a bare
 * `"rating:X"` tag string or `null` (rating cleared/absent), per
 * `archives.rs::update_archive_metadata`'s rating-only-change branch. */
function readRatingTag(value: unknown): string | null | undefined {
  if (value && typeof value === "object" && "rating" in value) {
    const rating = (value as { rating: unknown }).rating
    return typeof rating === "string" ? rating : null
  }
  return undefined
}

/** Short, single-line summary of an `archive.metadata_update`/`tankoubon.metadata_update`'s own
 * `after` (the precomputed `tags_added`/`tags_removed` plus whether the title/name field actually
 * changed — `archives.rs` calls its field `title`, `tankoubons.rs` calls its own `name`, both
 * checked here) — for the "操作内容" column/field, which only has room for a line of text, not the
 * full `MetadataDiff` word-level diff the detail modal renders below it. `undefined` when nothing
 * about `after` looks like a real change summary (a malformed/unexpected shape), letting the
 * caller fall back to just the title. */
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

/** A resource title, linked to its real page when `activityTarget.ts::targetLink` has one for
 * this `target.kind`/`action_type` combination, plain text otherwise (e.g. a `database.*` entry
 * with no single-resource page at all, or a deletion-type entry — that resource no longer exists,
 * so a link would only ever 404). Shared by the rename case's own "B" half below and the plain
 * non-rename case. */
function TargetTitle({ entry, title }: { entry: ActivityEntry; title: string }) {
  if (isDeletionActionType(entry.action_type)) return <>{title}</>
  const href = targetLink(entry.target.kind, entry.target.id, entry.target.label, entry.after, entry.action_type)
  if (!href) return <>{title}</>
  // `target.exists === false` — the resource this entry is *about* (not this entry's own action)
  // was deleted by some later, unrelated action (e.g. a rating change on an archive that's since
  // been removed). The link would only ever 404 now, so navigation is blocked — but the `<Link>`
  // itself (and its real `href`) stays intact rather than degrading to a plain `<span>`, per
  // explicit feedback: hovering must still show the target URL in the browser's status bar, and
  // right-click "copy link address" must still work. Only the click's default navigation is
  // suppressed (`e.preventDefault()`), same struck-through styling as before.
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

/** `settings.update`/`plugin.priority_update` (and other `plugin.*`) entries have no per-resource
 * title at all (`target.label`/`target.id` both `null` — there's no single "thing" a settings
 * change or a priority reorder is about, just a section of the config surface). Rather than
 * falling into the "no title, render em dash" case below, these get the action's own translated
 * name (`actionTypeLabel`, already shown as this entry's chip) linked to the specific accordion
 * `activityTarget.ts::targetLink` resolves for it — e.g. "更新设置" linking straight to
 * `/config?section=tags-thumbnails` when the changed fields were all thumbnail-related. */
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

/** What the "操作内容" (operation content) column/field shows for one entry:
 * - a whole-database action with no single target at all (`hasNoTarget`) → a fixed, no-
 *   placeholder description string (`activity.operationFixed.*`).
 * - a rename-type action (`RENAME_ACTION_TYPES`) → "将 {old name} 重命名为 {new name}", the new
 *   name linked to the resource's real page (old name is plain text — it's no longer this
 *   resource's own name, linking it would be misleading).
 * - a deletion-type action (`isDeletionActionType`) → "已删除 {title}", plain text (the resource
 *   named no longer exists, so a link would only ever 404).
 * - everything else → the target's own title, linked to its real page when one exists for this
 *   `target.kind`, plain text otherwise.
 *
 * `target.label` (a human title, snapshotted at write time) is preferred over the raw `target.id`
 * wherever available — see `ActivityTarget::label`'s own backend docs. */
export function OperationDescription({ entry }: { entry: ActivityEntry }) {
  const { t } = useTranslation()

  if (hasNoTarget(entry.action_type)) {
    return <>{t(`activity.operationFixed.${entry.action_type}`, { defaultValue: entry.action_type }) ?? entry.action_type}</>
  }

  // `plugin.priority_update`'s own `target.label` is the raw plugin `type` value itself
  // (`body.kind`, e.g. `"metadata"`) — translated here via the same i18n key
  // `PluginsPage.tsx`'s own accordion header uses for that group, rather than shown as-is.
  const rawTitle = entry.target.label ?? entry.target.id
  const title =
    entry.target.kind === "plugin" && rawTitle && rawTitle in PLUGIN_TYPE_LABEL_KEYS
      ? (t(PLUGIN_TYPE_LABEL_KEYS[rawTitle]) ?? rawTitle)
      : rawTitle
  if (!title) {
    // `settings.update`/most `plugin.*` entries have no title at all (see `SectionLink`'s own
    // docs) — still worth a real link to the section that actually changed, not just an em dash.
    if (entry.target.kind && SECTION_LINK_ONLY_KINDS.has(entry.target.kind)) return <SectionLink entry={entry} />
    return <>—</>
  }

  if (RENAME_ACTION_TYPES.has(entry.action_type)) {
    const oldName = readName(entry.before)
    const newName = readName(entry.after) ?? title
    if (oldName && oldName !== newName) {
      // `activity.renamePrefix` is deliberately just the "将 {{from}} 重命名为 " lead-in, not the
      // whole sentence — the new name is a live `<Link>`, not plain text, so it can't be baked
      // into a single interpolated i18n string the way the rest of this component's descriptions
      // are. This is the one place in the Activity page's copy that isn't a single self-contained
      // translated sentence for that reason.
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

  // A rating change's own real star-row widgets (`StarRatingDisplay`, the same read-only rendering
  // `ArchiveOverviewOverlay.tsx`'s `TagsTable` already uses for a rating tag, not plain text) —
  // shown right in the "操作内容" column/field itself rather than only inside the detail modal,
  // since it's compact enough to always fit on one line unlike a full metadata diff. Shows *both*
  // the old and new rating (old → new, an arrow between them, GitHub-diff-style before/after —
  // matching how every other change on this page shows both sides, not just the result), omitting
  // the "before" side only when there genuinely was none (a fresh first-time rating, not a
  // changed one). Covers both an archive's own rating and a Tankoubon's
  // (`tankoubons.rs::update_tankoubon`'s own rating-only branch) — same `{rating: ...}`
  // before/after shape either way.
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

  // Metadata edits get a short "what changed" suffix right in this column/field too (see
  // `metadataChangeSummary`'s own docs) — the full word-level diff still only renders in the
  // detail modal below "变更内容", this is just enough to tell at a glance whether the title
  // changed and how many tags were added/removed without opening it. Covers both an archive's own
  // edit and a Tankoubon's (`tankoubons.rs::update_tankoubon`'s non-rating branch).
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

  // `bookmark.add`/`bookmark.remove` get a "第 N 页" suffix — `TargetTitle` alone only ever shows
  // the archive itself, which a page-level bookmark event needs one more piece of context beyond
  // (an archive can carry any number of independent page bookmarks at once, so "已添加书签" on its
  // own doesn't say which one this entry was actually about).
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
