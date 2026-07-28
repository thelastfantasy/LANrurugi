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
  library: () => '/',
  login: () => '/login',
  reader: (archiveId: string) => `/reader/${archiveId}`,
  edit: (archiveId: string) => `/edit/${archiveId}`,
  tankoubonEdit: (tankId: string) => `/tankoubon/${tankId}/edit`,
  upload: () => '/upload',
  duplicates: () => '/duplicates',
  stats: () => '/stats',
  backup: () => '/backup',
  logs: () => '/logs',
  jobs: () => '/jobs',
  batch: () => '/batch',
  settings: () => '/config',
  categories: () => '/config/categories',
  // `focus` deep-links to a specific plugin's download/rate-limit settings section (issue #2):
  // Plugins.tsx reads `?focus=<namespace>` and scrolls that section into view + briefly highlights
  // it. Omit for the plain plugin-list landing.
  pluginSettings: (focus?: string) =>
    focus ? `/config/plugins?focus=${encodeURIComponent(focus)}` : '/config/plugins',
} as const
