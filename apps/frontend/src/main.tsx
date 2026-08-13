import "./index.css"
import "./i18n"

import { QueryClient, QueryClientProvider } from "@tanstack/react-query"
import { StrictMode } from "react"
import { createRoot } from "react-dom/client"

import { ApiError } from "./api/client"
import { App } from "./App"

const queryClient = new QueryClient({
  defaultOptions: {
    queries: {
      // A 4xx (e.g. opening the reader on an archive id that no longer exists) is a deterministic
      // failure — retrying it just re-plays the same 404 three more times with exponential
      // backoff, turning an instant "not found" into several seconds of spinner for no benefit.
      // 5xx/network errors (the default's actual target) still retry normally.
      retry: (failureCount, error) => {
        if (error instanceof ApiError && error.status >= 400 && error.status < 500) return false
        return failureCount < 3
      },
    },
  },
})

const root = document.getElementById("root")
if (!root) throw new Error("missing #root element")
createRoot(root).render(
  <StrictMode>
    <QueryClientProvider client={queryClient}>
      <App />
    </QueryClientProvider>
  </StrictMode>,
)
