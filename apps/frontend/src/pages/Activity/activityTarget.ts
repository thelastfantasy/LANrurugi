import { routes } from "@/lib/routes"

/** Turns `"archive.delete"` into a `("archive", "delete")` pair — action types are namespaced
 * strings (`lanrurugi_storage::activity::action_types`), grouped/labeled here by their namespace
 * prefix the same way legacy-derived tag namespaces already group elsewhere in this app. Shared
 * by every place that needs either half (`ActivityFilterCombobox.tsx`'s own namespace grouping,
 * `actionTypeLabel` below's fallback) rather than each keeping its own copy — two independent
 * copies of this exact split is what let `ActivityFilterCombobox.tsx`'s dropdown-item labels and
 * `ActivityRow.tsx`/`ActivityDetailPanel.tsx`'s row/detail labels drift to two different fallback
 * strings (`"start"` vs. `"download_queue.start"`) for the same missing translation, confirmed
 * live before this was unified. */
export function splitActionTypeNamespace(actionType: string): { namespace: string; leaf: string } {
  const dot = actionType.indexOf(".")
  if (dot === -1) return { namespace: actionType, leaf: actionType }
  return { namespace: actionType.slice(0, dot), leaf: actionType.slice(dot + 1) }
}

/** Fixed namespace display order — matches `lanrurugi_storage::activity::action_types`'s own
 * declaration order (archive, settings, category, token, download_queue, ...), a resource-kind
 * grouping a human already reasons about ("all the category stuff", "all the download stuff")
 * rather than alphabetical-by-translated-label, which scatters related actions across the list in
 * whatever order their Chinese/English label text happens to sort to — confirmed live as reading
 * as genuinely random ("撤销API令牌"/"创建分类"/"创建API令牌" sharing one row with no visible
 * relationship). Any namespace not in this list sorts after all of these, alphabetically among
 * themselves — new namespaces don't need an edit here to appear, just won't be front-and-center. */
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

/** The one real label function for an `action_type` — every chip/row/detail-panel/dropdown-item
 * that shows an action type's name calls this, not its own inline `t(...)` call, specifically so
 * a translation gap resolves to the *same* fallback text everywhere instead of each call site's
 * own independently-chosen fallback (`entry.action_type` verbatim in one place,
 * `splitActionTypeNamespace(...).leaf` in another) making the same untranslated action type look
 * like two different things depending on which component happened to render it. */
export function actionTypeLabel(t: (key: string, opts?: Record<string, unknown>) => string | null, actionType: string): string {
  return t(`activity.actionType.${actionType}`, { defaultValue: splitActionTypeNamespace(actionType).leaf }) ?? actionType
}

/** `action_type` values whose whole point is "this resource no longer exists" — a link to
 * `target` for one of these would resolve to a 404/blank page essentially every time (the
 * resource it names was, by definition, just deleted), so these render as plain "已删除 {title}"
 * text instead of a clickable link (`ActivityRow`/`ActivityDetailPanel`'s own logic). An explicit
 * set, not a `.endsWith("delete")` string-suffix guess — `token.revoke`/`database.drop` are
 * semantically identical (the named resource stops existing) but don't share that literal
 * spelling, and a suffix guess would also need to special-case `archive.patch_delete` (still
 * matches `delete`, but happens to work) while offering no protection against a future action
 * type that contains "delete" as a substring without meaning it in this sense. */
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

/** `action_type` values whose `target.label`/`target.id` are both always `null` — a handful of
 * whole-database operations (`database.rs`'s own write sites confirm every one of these) that
 * have no single resource to name at all, not "a resource whose title happens to be missing this
 * time." These get a fixed, no-placeholder description string (`activity.operationFixed.*`)
 * instead of the templated `activity.operationDescription.*` (`{{title}}`) every other action
 * type uses — a template with nothing to fill would otherwise render literally as "{{title}}" or
 * need a redundant per-call empty-string fallback at every call site. */
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

/** `plugin.*` action types' `type`/`kind` value (`login`/`download`/`script`/`metadata`) maps
 * 1:1 onto `PluginsPage.tsx`'s own `CollapsibleSection` `id`s for those same four groups — see
 * `LEFT_GROUPS`/`RIGHT_GROUPS` there. */
const PLUGIN_TYPE_TO_SECTION = new Set(["login", "download", "script", "metadata"])

/** Same four `type` values, mapped to the exact (untranslated, `t()`-ready) i18n key
 * `PluginsPage.tsx`'s own `LEFT_GROUPS`/`RIGHT_GROUPS` already use as each flyout's title —
 * `plugin.priority_update`'s `target.label` is the raw `type` string itself (`body.kind`, per
 * `plugins.rs::put_plugin_priority`), which reads as an untranslated technical value ("metadata")
 * rather than a real label if shown as-is; this turns it into the same "元数据插件" text the
 * Plugins page's own accordion header already shows for that group. */
export const PLUGIN_TYPE_LABEL_KEYS: Record<string, string> = {
  login: "Login Plugins",
  download: "Downloaders",
  script: "Scripts",
  metadata: "Metadata Plugins",
}

/** `archive`/`tankoubon`-kind action types whose real subject is the resource's own tags/rating/
 * summary, not its structural identity (filename/member list) the Edit page is actually for — a
 * rating or metadata change deep-links straight into the reader's own archive overview overlay
 * (`routes.readerOverview`) instead, since that's the one surface that actually shows a rating at
 * all (the Edit page doesn't render one), matching what these two entries are actually about far
 * better than the generic structural editor. Every other `archive`/`tankoubon` action type
 * (rename, upload, member add/remove, ...) keeps linking to the real structural Edit page. */
const OVERVIEW_LINK_ACTION_TYPES = new Set([
  "archive.metadata_update",
  "archive.rating_update",
  "tankoubon.metadata_update",
  "tankoubon.rating_update",
])

/** Maps an `ActivityTarget` (+ `after`, for the handful of kinds whose real link target lives in
 * the write-side payload rather than `target.id` itself — `settings`/`plugin`) to the in-app
 * route that actually shows that resource. `undefined` for a kind with no single-resource page at
 * all (`database`: a whole-database operation, never "this one specific thing"). Only ever called
 * for a *non*-deletion action type (`isDeletionActionType` above already routes deletions to a
 * plain-text render instead) — an archive/category/tankoubon/token/download-queue-item this
 * resolves a link for is expected to still exist, though nothing here re-verifies that at click
 * time (a link to a resource deleted by some *other*, unrelated action in between is the one
 * remaining edge case, same tradeoff `Jobs.tsx`'s own links to now-gone archives already accepts
 * elsewhere in this app). */
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
      // `bookmark.add`/`bookmark.remove` link straight into the reader at the specific page the
      // bookmark was on (`after.page`, per `bookmarks.rs`'s own `record_manual` calls) — a plain
      // Edit-page link (this branch's own default, below) would land somewhere that says nothing
      // about which of the archive's potentially many independent page bookmarks this entry was
      // actually about.
      if (actionType === "bookmark.add" || actionType === "bookmark.remove") {
        const page = after && typeof after === "object" && "page" in after ? (after as { page: unknown }).page : undefined
        return typeof page === "number" ? `${routes.reader(id)}?p=${page}` : routes.reader(id)
      }
      // Only a real archive (36-char hash id) has a reader page at all — a Tankoubon's own member
      // archives never reach this branch (they're `kind: "tankoubon"`), but `target.id` itself
      // gives no other signal to check against here, so this stays a straight passthrough.
      return actionType && OVERVIEW_LINK_ACTION_TYPES.has(actionType) ? routes.readerOverview(id) : routes.edit(id)
    case "tankoubon":
      if (!id) return undefined
      // A Tankoubon has no reader-overview-equivalent overlay of its own — its own Edit page is
      // already the closest thing to that (renders its tags/rating alongside its member list), so
      // rating/metadata changes still link there, just called out here so a reader coming from
      // `OVERVIEW_LINK_ACTION_TYPES`'s own archive-side docs above doesn't wonder why this branch
      // doesn't also redirect somewhere else.
      return routes.tankoubonEdit(id)
    case "category":
      return id ? routes.categories() : undefined
    case "token":
      return routes.settings("api-tokens")
    case "download_queue_item":
      // `id` is always present for this kind (the queue item's own id) — the URL-download trigger
      // case below is the only `download_url`-kind entry, which never has one at all.
      return routes.upload()
    case "download_url":
      return routes.upload()
    case "settings": {
      // `after.changed_fields` — see `settingsSectionForChangedFields`'s own docs.
      const changedFields =
        after && typeof after === "object" && "changed_fields" in after
          ? (after as { changed_fields: unknown }).changed_fields
          : undefined
      const section = settingsSectionForChangedFields(changedFields)
      return routes.settings(section)
    }
    case "plugin": {
      // The plugin `type` value lives in different places per action type: `plugin.upload`'s own
      // `after.type`, `plugin.priority_update`'s own `target.label` (`body.kind`, per
      // `plugins.rs::put_plugin_priority`). `plugin.execute`'s `after` is `None` and `label` is
      // the plugin *name* not its type, so it falls back to the bare plugin list rather than
      // guessing a section.
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

/** Which `Settings` accordion (`CollapsibleSection`'s own `id` prop) a given raw settings field
 * name belongs to — built directly from each section component's own field list
 * (`GlobalSection.tsx`/`SecuritySection.tsx`/`ArchiveFilesSection.tsx`/
 * `TagsThumbnailsSection.tsx`/the inline theme section in `SettingsPage.tsx` itself). `settings.rs`'s
 * own `put_settings` records `after.changed_fields` as this same raw field-name list (the JSON
 * body's own keys, e.g. `"motd"`/`"webpquality"`), so this map is what turns that into "which
 * section actually changed" for a deep link. `theme` isn't reachable through `put_settings`'s own
 * `changed_fields` (the theme radio buttons call `updateSettings.mutate({ theme })` directly, a
 * separate save path — see `SettingsPage.tsx`'s own theme-switch handler), so it's included here
 * for completeness but in practice never the field this map is asked to resolve. */
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
  devmode: "global",
  newbadgemode: "global",
  recommendprecision: "global",
  llm_api_key: "global",
  theme: "theme",
  enablepass: "security",
  nofunmode: "security",
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

/** The Settings deep-link section for a `settings.update` entry's `after.changed_fields` — the
 * first field (in list order) that resolves to a known section, since one save can touch fields
 * from several sections at once (they're all one bulk `PUT /settings` call) and a single link can
 * only point at one place. `undefined` when `changed_fields` is missing/empty/entirely unknown
 * fields, so the caller can fall back to the bare `/config` link. */
export function settingsSectionForChangedFields(changedFields: unknown): string | undefined {
  if (!Array.isArray(changedFields)) return undefined
  for (const field of changedFields) {
    if (typeof field === "string" && field in SETTINGS_FIELD_SECTIONS) return SETTINGS_FIELD_SECTIONS[field]
  }
  return undefined
}
