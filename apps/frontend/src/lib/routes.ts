/** Single source of truth for every in-app route path — every `navigate(...)` call and every
 * fallback `<a href>` (for real middle-click/right-click/hover-preview behavior, since this app
 * is `BrowserRouter`-based, not `HashRouter`) should build its target through one of these
 * functions rather than a raw template string. A raw `` `#/reader/${id}` `` string (a same-page
 * hash fragment, not a real route) shipped in six different files before this module existed —
 * `BrowserRouter` ignores hash fragments entirely, so every one of those was a broken link for
 * anything that doesn't go through a React `onClick` handler (middle-click "open in new tab",
 * right-click "copy link", or a "Copy Link" button building a shareable URL by hand) — silently
 * broken since nothing but a real click ever exercised them. One typo like that is easy to miss
 * during review; six independent copies of the same typo is a real footgun from not having a
 * single place this had to be gotten right.
 *
 * Mirrors the route table in `App.tsx` — if a route is added/renamed there, add/rename its
 * builder here too. */
export const routes = {
  library: () => "/",
  login: () => "/login",
  reader: (archiveId: string) => `/reader/${archiveId}`,
  // `?overview=1` opens straight into the archive overview overlay (`Reader.tsx`'s own
  // `startWithOverview` reads it) — used by the Activity page's own `archive.metadata_update`/
  // `archive.rating_update` operation-content links, whose real subject is "this archive's tags/
  // rating/summary changed", which the overview overlay shows directly rather than the plain
  // reader view or the Edit page (neither of which surfaces a rating at all).
  readerOverview: (archiveId: string) => `/reader/${archiveId}?overview=1`,
  edit: (archiveId: string) => `/edit/${archiveId}`,
  tankoubonEdit: (tankId: string) => `/tankoubon/${tankId}/edit`,
  upload: () => "/upload",
  duplicates: () => "/duplicates",
  stats: () => "/stats",
  backup: () => "/backup",
  logs: () => "/logs",
  jobs: () => "/jobs",
  activity: () => "/activity",
  bookmarks: () => "/bookmarks",
  batch: () => "/batch",
  // `section` deep-links to one of Settings' own accordions (`CollapsibleSection`'s own `id`
  // prop — "global"/"theme"/"security"/"api-tokens"/"archive-files"/"tags-thumbnails"/
  // "background-workers") — `useSectionDeepLink` reads `?section=<id>`, opens the matching
  // section, and scrolls it into view. The Activity page's own operation-content links use this to
  // jump straight to "which settings section changed" for a `settings.update`/`token.*`/etc.
  // entry instead of just the bare settings landing page.
  settings: (section?: string) => (section ? `/config?section=${encodeURIComponent(section)}` : "/config"),
  categories: () => "/config/categories",
  // `focus` deep-links to a specific plugin's download/rate-limit settings section (issue #2):
  // Plugins.tsx reads `?focus=<namespace>` and scrolls that section into view + briefly highlights
  // it. Omit for the plain plugin-list landing.
  pluginSettings: (focus?: string) =>
    focus ? `/config/plugins?focus=${encodeURIComponent(focus)}` : "/config/plugins",
  // `section` deep-links to one of Plugins' own accordions (`CollapsibleSection`'s own `id` prop —
  // a plugin `type` value "login"/"download"/"script"/"metadata", or "maintenance-scripts") — same
  // `useSectionDeepLink` mechanism as `settings` above, for `plugin.*` activity entries.
  pluginSection: (section: string) => `/config/plugins?section=${encodeURIComponent(section)}`,
  pluginWizard: () => "/config/plugins/wizard",
} as const
