import { QueryClient } from "@tanstack/react-query"

import { ApiError } from "./client"

// A 4xx (e.g. opening the reader on an archive id that no longer exists) is a deterministic
// failure — retrying it just re-plays the same 404 three more times with exponential backoff,
// turning an instant "not found" into several seconds of spinner for no benefit. 5xx/network
// errors (the default's actual target) still retry normally.
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
