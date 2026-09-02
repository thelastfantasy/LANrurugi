import { useOnlyMatchingBookmarks as useOnlyMatchingBookmarksQuery, useSetOnlyMatchingBookmarks } from "@/api/hooks"

/** Persisted server-side (Redis), same "follows the user across devices" reasoning
 * `useHoverGridPageOrder` already documents. `false` (show every bookmark on a matched archive,
 * the historical behavior) while loading or if never explicitly saved. */
export function useOnlyMatchingBookmarksPreference(): [boolean, (value: boolean) => void] {
  const query = useOnlyMatchingBookmarksQuery()
  const setMutation = useSetOnlyMatchingBookmarks()
  const onlyMatching = query.data?.only_matching ?? false
  function setOnlyMatching(next: boolean) {
    setMutation.mutate(next)
  }
  return [onlyMatching, setOnlyMatching]
}
