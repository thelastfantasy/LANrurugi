import { routes } from "@/lib/routes"

/** Turns `"archive.delete"` into a `("archive", "delete")` pair — shared by every place that needs
 * either half, so a missing translation resolves to the same fallback everywhere. */
export function splitActionTypeNamespace(actionType: string): { namespace: string; leaf: string } {
  const dot = actionType.indexOf(".")
  if (dot === -1) return { namespace: actionType, leaf: actionType }
  return { namespace: actionType.slice(0, dot), leaf: actionType.slice(dot + 1) }
}

/** Fixed namespace display order (resource-kind grouping, not alphabetical-by-label). Any
 * namespace not listed sorts after these, alphabetically among themselves. */
const ACTION_TYPE_NAMESPACE_ORDER = [
  "archive",
  "bookmark",
  "settings",
  "category",
  "token",
  "download_queue",
  "tankoubon",
  "database",
  "plugin",
  "scanner",
  "metadata_plugin",
  "auto_download",
]

/** Sort comparator for a list of `action_type` strings — namespace order first
 * (`ACTION_TYPE_NAMESPACE_ORDER`), then alphabetically by translated label within a namespace. */
export function compareActionTypesForDisplay(
  t: (key: string, opts?: Record<string, unknown>) => string | null,
  a: string,
  b: string,
): number {
  const nsA = splitActionTypeNamespace(a).namespace
  const nsB = splitActionTypeNamespace(b).namespace
  const orderA = ACTION_TYPE_NAMESPACE_ORDER.indexOf(nsA)
  const orderB = ACTION_TYPE_NAMESPACE_ORDER.indexOf(nsB)
  const rankA = orderA === -1 ? ACTION_TYPE_NAMESPACE_ORDER.length : orderA
  const rankB = orderB === -1 ? ACTION_TYPE_NAMESPACE_ORDER.length : orderB
  if (rankA !== rankB) return rankA - rankB
  if (rankA === ACTION_TYPE_NAMESPACE_ORDER.length && nsA !== nsB) return nsA.localeCompare(nsB)
  return actionTypeLabel(t, a).localeCompare(actionTypeLabel(t, b))
}

/** The one real label function for an `action_type` — every caller uses this instead of its own
 * inline `t(...)`, so a translation gap resolves to the same fallback everywhere. */
export function actionTypeLabel(t: (key: string, opts?: Record<string, unknown>) => string | null, actionType: string): string {
  return t(`activity.actionType.${actionType}`, { defaultValue: splitActionTypeNamespace(actionType).leaf }) ?? actionType
}

/** `action_type` values meaning "this resource no longer exists" — rendered as plain text instead
 * of a link, since the linked resource is gone by definition. An explicit set, not a suffix guess. */
const DELETION_ACTION_TYPES = new Set([
  "archive.delete",
  "archive.patch_delete",
  "category.delete",
  "tankoubon.delete",
  "download_queue.delete",
  "token.revoke",
  "database.drop",
])

export function isDeletionActionType(actionType: string): boolean {
  return DELETION_ACTION_TYPES.has(actionType)
}

/** `"success"` | `"failure"` → its i18n display label — same "one shared lookup" reasoning as
 * `actionTypeLabel` above. */
export function outcomeLabel(t: (key: string) => string | null, status: string): string {
  return (status === "failure" ? t("activity.outcomeFailure") : t("activity.outcomeSuccess")) ?? status
}

/** `action_type` values with no single resource to name at all (whole-database operations) — use
 * a fixed no-placeholder description instead of the `{{title}}`-templated one every other type uses. */
const NO_TARGET_ACTION_TYPES = new Set([
  "database.clean",
  "database.clear_new_flags",
  "database.drop",
  "database.rebuild_index",
  "database.restore",
])

export function hasNoTarget(actionType: string): boolean {
  return NO_TARGET_ACTION_TYPES.has(actionType)
}

/** `plugin.*` action types' `type`/`kind` value maps 1:1 onto `PluginsPage.tsx`'s own
 * `CollapsibleSection` `id`s for those four groups. */
const PLUGIN_TYPE_TO_SECTION = new Set(["login", "download", "script", "metadata"])

/** Same four `type` values, mapped to `PluginsPage.tsx`'s own group title i18n keys, so a raw
 * `type` string (e.g. from `plugin.priority_update`'s `target.label`) reads as a real label. */
export const PLUGIN_TYPE_LABEL_KEYS: Record<string, string> = {
  login: "Login Plugins",
  download: "Downloaders",
  script: "Scripts",
  metadata: "Metadata Plugins",
}

/** `archive`/`tankoubon` action types about tags/rating/summary, not structural identity — these
 * deep-link to the reader's overview overlay (which shows rating) instead of the Edit page. */
const OVERVIEW_LINK_ACTION_TYPES = new Set([
  "archive.metadata_update",
  "archive.rating_update",
  "tankoubon.metadata_update",
  "tankoubon.rating_update",
])

/** Maps an `ActivityTarget` (+ `after`, for kinds whose link target lives in the write-side
 * payload) to the in-app route showing that resource. Only called for non-deletion action types. */
export function targetLink(
  kind: string | null,
  id: string | null,
  label?: string | null,
  after?: unknown,
  actionType?: string,
): string | undefined {
  switch (kind) {
    case "archive":
      if (!id) return undefined
      // `bookmark.add`/`bookmark.remove` link straight into the reader at the bookmarked page.
      if (actionType === "bookmark.add" || actionType === "bookmark.remove") {
        const page = after && typeof after === "object" && "page" in after ? (after as { page: unknown }).page : undefined
        return typeof page === "number" ? `${routes.reader(id)}?p=${page}` : routes.reader(id)
      }
      return actionType && OVERVIEW_LINK_ACTION_TYPES.has(actionType) ? routes.readerOverview(id) : routes.edit(id)
    case "tankoubon":
      if (!id) return undefined
      return routes.tankoubonEdit(id)
    case "category":
      return id ? routes.categories() : undefined
    case "token":
      return routes.settings("api-tokens")
    case "download_queue_item":
      return routes.upload()
    case "download_url":
      return routes.upload()
    case "settings": {
      const changedFields =
        after && typeof after === "object" && "changed_fields" in after
          ? (after as { changed_fields: unknown }).changed_fields
          : undefined
      const section = settingsSectionForChangedFields(changedFields)
      return routes.settings(section)
    }
    case "plugin": {
      const afterType =
        after && typeof after === "object" && "type" in after ? (after as { type: unknown }).type : undefined
      const type = typeof afterType === "string" && PLUGIN_TYPE_TO_SECTION.has(afterType) ? afterType : label
      if (typeof type === "string" && PLUGIN_TYPE_TO_SECTION.has(type)) return routes.pluginSection(type)
      return routes.pluginSettings()
    }
    default:
      return undefined
  }
}

/** Which `Settings` accordion a raw settings field name belongs to, for deep-linking
 * `after.changed_fields`. `theme` saves via a separate path, so is never actually looked up here. */
const SETTINGS_FIELD_SECTIONS: Record<string, string> = {
  htmltitle: "global",
  motd: "global",
  language: "global",
  pagesize: "global",
  enableresize: "global",
  sizethreshold: "global",
  readerquality: "global",
  localprogress: "global",
  authprogress: "global",
  guestmode: "global",
  newbadgemode: "global",
  recommendprecision: "global",
  llm_api_key: "global",
  theme: "theme",
  access_token_lifetime_secs: "security",
  refresh_token_lifetime_secs: "security",
  enablecors: "security",
  tempmaxsize: "archive-files",
  replacedupe: "archive-files",
  hqthumbpages: "tags-thumbnails",
  enablewebp: "tags-thumbnails",
  webpquality: "tags-thumbnails",
  excludednamespaces: "tags-thumbnails",
  tagruleson: "tags-thumbnails",
  tagrules: "tags-thumbnails",
  usedateadded: "tags-thumbnails",
  usedatemodified: "tags-thumbnails",
  timezone: "tags-thumbnails",
}

/** First `changed_fields` entry that resolves to a known Settings section; `undefined` falls back
 * to the bare `/config` link. */
export function settingsSectionForChangedFields(changedFields: unknown): string | undefined {
  if (!Array.isArray(changedFields)) return undefined
  for (const field of changedFields) {
    if (typeof field === "string" && field in SETTINGS_FIELD_SECTIONS) return SETTINGS_FIELD_SECTIONS[field]
  }
  return undefined
}
