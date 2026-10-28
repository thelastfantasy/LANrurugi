/**
 * The client half of the diagnostics capture (specs/018, research.md §1).
 *
 * The SDK's DSN points at *this* instance: its own transport posts the standard envelope to
 * `/api/diagnostics/envelope` on the same origin, so capture works with no outbound network and
 * without a custom transport. Nothing is initialised at all when the instance disabled capture.
 */

import * as Sentry from "@sentry/react"

import type { ForwardWindow } from "./api"

interface DiagnosticsConfig {
  enabled: boolean
  sources: { web: boolean; server: boolean; plugins: boolean }
  breadcrumbs: boolean
  release: string
  version: string
  window: ForwardWindow
}

let started = false

/** One breadcrumb source for the actions that explain "it broke after I clicked X". */
function listenForClicks() {
  document.addEventListener(
    "click",
    (event) => {
      const target = event.target
      if (!(target instanceof Element)) return
      const element = target.closest("button, a, [role='button']")
      if (!element) return
      const label =
        element.getAttribute("aria-label") ??
        element.textContent?.trim().slice(0, 80) ??
        element.tagName.toLowerCase()
      // No form values, no file names: the label is the whole breadcrumb.
      Sentry.addBreadcrumb({
        category: "interaction",
        level: "info",
        message: `click ${label}`,
      })
    },
    { capture: true, passive: true },
  )
}

function listenForNavigation() {
  let last = window.location.pathname + window.location.search
  const report = () => {
    const next = window.location.pathname + window.location.search
    if (next === last) return
    Sentry.addBreadcrumb({ category: "navigation", level: "info", message: `to ${next}` })
    last = next
  }
  window.addEventListener("popstate", report)
  // The SPA pushes state rather than loading pages; polling catches it without patching history.
  window.setInterval(report, 1_000)
}

function listenForRequests() {
  const original = window.fetch
  window.fetch = async (...args: Parameters<typeof fetch>) => {
    const started = performance.now()
    const response = await original(...args)
    const request = args[0]
    const path =
      typeof request === "string"
        ? request
        : request instanceof URL
          ? request.pathname
          : request.url
    // Paths only: a query string can carry a share code.
    const clean = path.replace(/^https?:\/\/[^/]+/, "").split("?")[0]
    const duration = Math.round(performance.now() - started)
    if (!response.ok && response.status >= 400) {
      Sentry.addBreadcrumb({
        category: "request",
        level: response.status >= 500 ? "error" : "warning",
        message: `${args[1]?.method ?? "GET"} ${clean} → ${response.status} (${duration} ms)`,
        data: { status: response.status, duration },
      })
    } else if (duration > 1_000) {
      Sentry.addBreadcrumb({
        category: "request",
        level: "info",
        message: `slow ${clean} (${duration} ms)`,
        data: { duration },
      })
    }
    return response
  }
}

/** Initialises capture if the instance allows it. Safe to call once at startup. */
export async function startDiagnostics(): Promise<void> {
  if (started) return
  started = true
  let config: DiagnosticsConfig
  try {
    const response = await fetch("/api/diagnostics/config")
    if (!response.ok) return
    config = (await response.json()) as DiagnosticsConfig
  } catch {
    // An unreachable config endpoint (logged out, server restarting) simply means no capture.
    return
  }
  if (!config.enabled) return

  // The SDK derives `<dsn>/api/<project>/envelope/` from the DSN, so naming the project
  // `diagnostics` makes it post straight at our own `/api/diagnostics/envelope` — no custom
  // transport, no CORS, and the session cookie rides along because it is same-origin.
  const dsn = `http://lanrurugi@${window.location.host}/diagnostics`
  Sentry.init({
    dsn,
    release: config.release,
    environment: "production",
    maxBreadcrumbs: 50,
    // No performance tracing: the spec deliberately scopes this to errors.
    tracesSampleRate: 0,
    beforeSend(event) {
      // The server caps a stored event at 64 KB; drop anything absurd before it gets there.
      if (JSON.stringify(event).length > 60_000) return null
      return event
    },
  })
  Sentry.getCurrentScope().setTag("app", "lanrurugi")
  listenForClicks()
  listenForNavigation()
  listenForRequests()
}

/** Reports a render failure caught by the React error boundary. */
export function captureRenderError(error: unknown): void {
  Sentry.captureException(error)
}

/** The ingest URL the SDK derives from {@link diagnosticsDsn}, exported for tests. */
export function ingestUrl(host: string): string {
  return `http://${host}/api/diagnostics/envelope/`
}
