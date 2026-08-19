import "./index.css"
import "./i18n"

import { QueryClientProvider } from "@tanstack/react-query"
import { StrictMode } from "react"
import { createRoot } from "react-dom/client"

import { queryClient } from "./api/queryClient"
import { App } from "./App"
import { ErrorBoundary } from "./components/Display/ErrorBoundary"

const root = document.getElementById("root")
if (!root) throw new Error("missing #root element")
createRoot(root).render(
  <StrictMode>
    <ErrorBoundary>
      <QueryClientProvider client={queryClient}>
        <App />
      </QueryClientProvider>
    </ErrorBoundary>
  </StrictMode>,
)
