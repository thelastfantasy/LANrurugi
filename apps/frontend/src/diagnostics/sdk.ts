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

/** An envelope as the protocol writes it: a header line, then one header + payload line per item. */
/** What the SDK hands a transport; derived from the SDK's own option type so it stays in sync. */
type BrowserTransportFactory = NonNullable<NonNullable<Parameters<typeof Sentry.init>[0]>["transport"]>
type BrowserTransport = ReturnType<BrowserTransportFactory>
type TransportRequest = Parameters<BrowserTransport["send"]>[0]

function serializeEnvelope(request: TransportRequest): string {
  // Newer SDKs hand the transport a prepared body (`{ body }`); older ones hand the tuple itself.
  const prepared = request as unknown as { body?: string | Uint8Array }
  if (prepared.body !== undefined) {
    return typeof prepared.body === "string"
      ? prepared.body
      : new TextDecoder().decode(prepared.body)
  }
  const [header, items] = request as unknown as [
    Record<string, unknown>,
    [Record<string, unknown>, unknown][],
  ]
  let out = `${JSON.stringify(header)}\n`
  for (const [itemHeader, payload] of items ?? []) {
    out += `${JSON.stringify(itemHeader)}\n`
    if (typeof payload === "string") out += `${payload}\n`
    else if (payload instanceof Uint8Array) out += `${new TextDecoder().decode(payload)}\n`
    // An event item's payload arrives as a plain object, not pre-serialised text.
    else out += `${JSON.stringify(payload)}\n`
  }
  return out
}

/**
 * Posts to this instance's own ingest. A custom transport is required rather than the SDK's default:
 * the default derives `<dsn>/api/<project>/envelope/` and Sentry's DSN parser only accepts a
 * *numeric* project id, so pointing it at a path like `/diagnostics` silently disables the client.
 */
function localTransport(): BrowserTransport {
  return {
    send: async (envelope) => {
      try {
        const response = await fetch("/api/diagnostics/envelope", {
          method: "POST",
          headers: { "Content-Type": "application/x-sentry-envelope" },
          body: serializeEnvelope(envelope),
        })
        return { statusCode: response.status }
      } catch {
        // Offline or the instance is restarting: the SDK retries, and nothing is lost for long.
        return { statusCode: 503 }
      }
    },
    flush: async () => true,
  }
}

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

  // The DSN exists only to satisfy the SDK (it needs one to enable a client); every envelope goes
  // through `localTransport`, so the numeric project id here is never used to build a URL.
  const dsn = `http://lanrurugi@${window.location.host}/1`
  Sentry.init({
    dsn,
    transport: () => localTransport(),
    release: config.release,
    environment: "production",
    maxBreadcrumbs: 50,
    // Navigation + request spans, at the same 1% the server uses: enough for a p95, not a firehose.
    tracesSampleRate: 0.01,
    integrations: (defaults) => [
      ...defaults,
      Sentry.browserTracingIntegration({ instrumentNavigation: true }),
    ],
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

/** Reports a handled failure — one the UI recovered from but still wants to know about. */
export function reportError(error: unknown, context?: Record<string, string>): void {
  Sentry.withScope((scope) => {
    for (const [key, value] of Object.entries(context ?? {})) scope.setTag(key, value)
    Sentry.captureException(error)
  })
}

/** A breadcrumb for something worth knowing before an error, without an error of its own. */
export function note(message: string, data?: Record<string, unknown>): void {
  Sentry.addBreadcrumb({ category: "app", level: "info", message, data })
}

/** Wraps a user action so a failure inside it is reported with the action's name. */
export async function traced<T>(name: string, run: () => Promise<T>): Promise<T> {
  note(`start ${name}`)
  try {
    const result = await run()
    note(`done ${name}`)
    return result
  } catch (error) {
    reportError(error, { action: name })
    throw error
  }
}
