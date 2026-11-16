import { QueryClient } from "@tanstack/react-query"

import { ApiError } from "./client"

function isDeterministic(error: unknown): boolean {
  return error instanceof ApiError && error.status >= 400 && error.status < 500
}

/** A 4xx is a deterministic failure — don't retry it, and don't ask again just because the tab
 * regained focus (#290). 5xx/network errors still retry and refetch normally. */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: (failureCount, error) => {
        if (isDeterministic(error)) return false
        return failureCount < 3
      },
      refetchOnWindowFocus: (query) =>
        !(query.state.status === "error" && isDeterministic(query.state.error)),
    },
  },
})
