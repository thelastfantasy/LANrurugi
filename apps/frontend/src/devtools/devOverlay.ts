/**
 * Development-only problem overlay (#287). Loaded from `main.tsx` behind `import.meta.env.DEV`, so
 * a production build contains none of it.
 *
 * Makes visible what otherwise only reaches the console: runtime and render errors (including the
 * ones an error boundary caught, which leave no white screen behind), `console.error`/`warn`
 * calls, and a request repeated far more often than any page needs (a self-tracing loop, a query
 * that refetches forever). Plain DOM rather than React, so it still works when React has crashed.
 */

import type { ErrorInfo } from "react"

type Kind = "error" | "console" | "request"

interface Entry {
  kind: Kind
  title: string
  detail: string
  count: number
  last: number
}

/** Same method + path more often than this within {@link RATE_WINDOW_MS} is reported. */
const RATE_LIMIT = 20
const RATE_WINDOW_MS = 10_000
const MAX_ENTRIES = 100

const entries = new Map<string, Entry>()
let badge: HTMLButtonElement | null = null
let panel: HTMLDivElement | null = null
let open = false

function add(kind: Kind, title: string, detail: string, reveal: boolean) {
  const key = `${kind}\u0000${title}`
  const existing = entries.get(key)
  if (existing) {
    existing.count += 1
    existing.last = Date.now()
    existing.detail = detail || existing.detail
  } else {
    if (entries.size >= MAX_ENTRIES) entries.delete(entries.keys().next().value as string)
    entries.set(key, { kind, title, detail, count: 1, last: Date.now() })
  }
  if (reveal) open = true
  // Never render synchronously inside a console call or a React commit.
  queueMicrotask(render)
}

function describe(error: unknown): { title: string; detail: string } {
  if (error instanceof Error) return { title: `${error.name}: ${error.message}`, detail: error.stack ?? "" }
  return { title: String(error), detail: "" }
}

/** Reports an error from one of React's root callbacks or a global handler. */
export function reportDevError(source: string, error: unknown, info?: Partial<ErrorInfo>) {
  const { title, detail } = describe(error)
  const stack = info?.componentStack ? `\n\nComponent stack:${info.componentStack}` : ""
  add("error", `[${source}] ${title}`, detail + stack, true)
}

/** The callbacks `createRoot` takes; the defaults also log, so these log too. */
export const devRootOptions = {
  onUncaughtError(error: unknown, info: ErrorInfo) {
    console.error(error)
    reportDevError("uncaught render error", error, info)
  },
  onCaughtError(error: unknown, info: ErrorInfo) {
    console.error(error)
    reportDevError("render error caught by a boundary", error, info)
  },
  onRecoverableError(error: unknown, info: ErrorInfo) {
    console.warn(error)
    reportDevError("recoverable render error", error, info)
  },
}

function interceptConsole() {
  for (const kind of ["error", "warn"] as const) {
    // Only `error`/`warn` are wrapped; the rule cannot see that through the computed key.
    // eslint-disable-next-line no-console
    const original = console[kind].bind(console)
    // eslint-disable-next-line no-console
    console[kind] = (...args: unknown[]) => {
      original(...args)
      const [first, ...rest] = args
      const title = (first instanceof Error ? `${first.name}: ${first.message}` : String(first)).slice(0, 300)
      const detail = [first instanceof Error ? first.stack ?? "" : "", ...rest.map(String)].join("\n")
      // Counted, not shown: a warning should not open the panel over the page.
      add("console", `console.${kind}: ${title}`, detail, false)
    }
  }
}

function watchRequestRate() {
  const original = window.fetch.bind(window)
  const seen = new Map<string, number[]>()
  window.fetch = (...args: Parameters<typeof fetch>) => {
    const [input, init] = args
    const url = typeof input === "string" ? input : input instanceof URL ? input.href : input.url
    const method = (init?.method ?? (input instanceof Request ? input.method : "GET")).toUpperCase()
    const path = url.replace(/^https?:\/\/[^/]+/, "").split("?")[0]
    const key = `${method} ${path}`
    const now = Date.now()
    const times = (seen.get(key) ?? []).filter((at) => now - at < RATE_WINDOW_MS)
    times.push(now)
    seen.set(key, times)
    if (times.length === RATE_LIMIT) {
      add(
        "request",
        `${key}: ${RATE_LIMIT} requests in ${RATE_WINDOW_MS / 1000} s`,
        "The same request this often usually means a loop: a request that triggers itself, a query refetching on every render, or a poll with a wrong interval.",
        true,
      )
    }
    return original(...args)
  }
}

function render() {
  if (!badge || !panel) return
  const list = [...entries.values()]
  const errors = list.filter((e) => e.kind === "error").reduce((n, e) => n + e.count, 0)
  const logged = list.filter((e) => e.kind === "console").reduce((n, e) => n + e.count, 0)
  const requests = list.filter((e) => e.kind === "request").length
  badge.hidden = list.length === 0
  badge.textContent = `⚠ ${errors} error${errors === 1 ? "" : "s"} · ${logged} console${requests ? ` · ${requests} loop` : ""}`
  panel.hidden = !open
  if (!open) return
  const body = panel.querySelector<HTMLDivElement>("[data-dev-list]")
  if (!body) return
  body.replaceChildren(
    ...list
      .sort((a, b) => b.last - a.last)
      .map((e) => {
        const item = document.createElement("details")
        item.style.cssText = "margin:4px 0;border-top:1px solid currentColor;padding-top:4px"
        item.open = e.kind !== "console"
        const summary = document.createElement("summary")
        summary.style.cssText = "cursor:pointer;font-weight:bold;overflow-wrap:anywhere"
        summary.textContent = `${e.kind.toUpperCase()}${e.count > 1 ? ` ×${e.count}` : ""} — ${e.title}`
        const pre = document.createElement("pre")
        pre.style.cssText = "white-space:pre-wrap;font-size:11px;margin:4px 0;max-height:240px;overflow:auto"
        pre.textContent = e.detail
        item.append(summary, pre)
        return item
      }),
  )
}

function button(label: string, onClick: () => void): HTMLButtonElement {
  const b = document.createElement("button")
  b.type = "button"
  // The theme's own button class, so the overlay follows the active theme.
  b.className = "stdbtn"
  // Theme `.stdbtn` rules fix a height and a lopsided padding, which sat the label low.
  b.style.cssText = "width:auto;height:auto;line-height:1.4;margin:0 0 0 6px;padding:2px 8px"
  b.textContent = label
  b.addEventListener("click", onClick)
  return b
}

function mount() {
  badge = document.createElement("button")
  badge.type = "button"
  badge.className = "stdbtn"
  badge.hidden = true
  badge.title = "Development problems (dev server only)"
  badge.style.cssText =
    "position:fixed;right:8px;bottom:8px;z-index:2147483646;width:auto;height:auto;line-height:1.4;margin:0;padding:3px 8px;font-size:12px;opacity:.9"
  badge.addEventListener("click", () => {
    open = !open
    render()
  })

  panel = document.createElement("div")
  // `.ido` carries the active theme's page colours.
  panel.className = "ido"
  panel.hidden = true
  panel.style.cssText =
    "position:fixed;right:8px;bottom:40px;z-index:2147483647;width:min(720px,calc(100vw - 16px));max-height:60vh;overflow:auto;padding:8px 12px;text-align:left;border:2px solid currentColor;box-shadow:0 4px 16px rgba(0,0,0,.4);font-size:12px"
  const header = document.createElement("div")
  header.style.cssText = "display:flex;align-items:center"
  const heading = document.createElement("strong")
  heading.style.flex = "1"
  heading.textContent = "Dev problems"
  header.append(
    heading,
    button("Clear", () => {
      entries.clear()
      open = false
      render()
    }),
    button("Close", () => {
      open = false
      render()
    }),
  )
  const list = document.createElement("div")
  list.dataset.devList = ""
  panel.append(header, list)
  document.body.append(panel, badge)
  render()
}

export function installDevOverlay() {
  interceptConsole()
  watchRequestRate()
  window.addEventListener("error", (event) => reportDevError("window error", event.error ?? event.message))
  window.addEventListener("unhandledrejection", (event) => reportDevError("unhandled rejection", event.reason))
  if (document.body) mount()
  else document.addEventListener("DOMContentLoaded", mount, { once: true })
}
