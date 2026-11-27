import { type QueryClient, useQueryClient } from "@tanstack/react-query"

import { useDataChange } from "./lib/crossTabData"

/**
 * Everything another tab can invalidate by changing the library. Prefixes, not full keys: each one
 * matches its own family (`["archive"]` matches `["archive", id]`, `["archives"]` the listings).
 */
const AFFECTED_KEYS = [
  "archives",
  "archive",
  "archive-pages",
  "archive-split-suggestion",
  "archive-split-tree",
  "categories",
  "filter-counts",
  "search",
  "search-pages",
  "tankoubons",
  "tankoubon",
] as const

/** Refetches everything a library change can make stale: another tab's, or a batch run in this one. */
export function invalidateLibraryData(queryClient: QueryClient): void {
  for (const key of AFFECTED_KEYS) {
    void queryClient.invalidateQueries({ queryKey: [key] })
  }
}

/**
 * Refetches what another tab changed. Mounted once, at the shell: a page that owns its own cache
 * entries (the Library listing, a reader, the selection drawer) is otherwise only ever refreshed by
 * its own mutations, so a batch run in a second tab left every one of them showing state that no
 * longer exists.
 */
export function CrossTabSync() {
  const queryClient = useQueryClient()

  useDataChange(() => invalidateLibraryData(queryClient))

  return null
}
