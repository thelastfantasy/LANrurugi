import "./index.css"
import "./i18n"

import { QueryClientProvider } from "@tanstack/react-query"
import { StrictMode } from "react"
import { createRoot } from "react-dom/client"

import { queryClient } from "./api/queryClient"
import { App } from "./App"
import { ErrorBoundary } from "./components/common-ui/Display/ErrorBoundary"
import { installImageFormatSupportCookie } from "./lib/utils/imageFormatSupport"
import { SessionProvider } from "./session/SessionProvider"

async function bootstrap() {
  // Feature-detect JXL/WebP once before the first image request is rendered. The result is
  // persisted in a cookie so plain `<img>` thumbnail/page requests carry the same capability
  // signal as the reader's own `fetch()` calls. A detection failure must never keep the app from
  // mounting; the backend's own default remains the existing WebP pipeline.
  try {
    await installImageFormatSupportCookie()
  } catch {
    // Fall through to `getImageFormatSupport()`'s own WebP default.
  }
  const root = document.getElementById("root")
  if (!root) throw new Error("missing #root element")
  createRoot(root).render(
    <StrictMode>
      <ErrorBoundary>
        <QueryClientProvider client={queryClient}>
          <SessionProvider>
            <App />
          </SessionProvider>
        </QueryClientProvider>
      </ErrorBoundary>
    </StrictMode>,
  )
}

void bootstrap()
