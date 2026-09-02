import { QueryClient } from "@tanstack/react-query"

import { ApiError } from "./client"

/** A 4xx is a deterministic failure — don't retry it. 5xx/network errors still retry normally. */
export const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      retry: (failureCount, error) => {
        if (error instanceof ApiError && error.status >= 400 && error.status < 500) return false
        return failureCount < 3
      },
    },
  },
})
