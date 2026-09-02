/** Single source of truth for every in-app route path — build targets through these functions,
 * not a raw template string. Mirrors the route table in `App.tsx`. */
export const routes = {
  library: () => "/",
  login: () => "/login",
  reader: (archiveId: string) => `/reader/${archiveId}`,
  // `?overview=1` opens straight into the archive overview overlay (`Reader.tsx`'s `startWithOverview`).
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
  // `section` deep-links to one of Settings' accordions; `useSectionDeepLink` opens+scrolls to it.
  settings: (section?: string) => (section ? `/config?section=${encodeURIComponent(section)}` : "/config"),
  categories: (categoryId?: string) =>
    categoryId ? `/config/categories/${categoryId}` : "/config/categories",
  // `focus` scrolls to a specific plugin's settings section and briefly highlights it.
  pluginSettings: (focus?: string) =>
    focus ? `/config/plugins?focus=${encodeURIComponent(focus)}` : "/config/plugins",
  pluginSection: (section: string) => `/config/plugins?section=${encodeURIComponent(section)}`,
  pluginWizard: () => "/config/plugins/wizard",
} as const
