import { Component, type ErrorInfo, type ReactNode } from "react"

/** issue #92's own catch-all for a render crash (React itself has no function-component API for
 * this — `componentDidCatch`/`getDerivedStateFromError` are still class-only even in React 19) —
 * wraps the whole `<App />` (`main.tsx`) so an unhandled exception anywhere in the tree renders
 * this generic recovery UI instead of leaving a blank white page with no indication anything went
 * wrong, which is what happens with no boundary at all. Deliberately outside `Layout`/`BrowserRouter`
 * context assumptions (no `useTranslation`/`useNavigate` — a crash could originate from the i18n or
 * router setup itself) — plain hardcoded English/`window.location`, not because localization
 * doesn't matter here but because a component that might itself be reacting to a broken provider
 * tree shouldn't lean on that same tree to render its own fallback. */
export class ErrorBoundary extends Component<{ children: ReactNode }, { hasError: boolean }> {
  constructor(props: { children: ReactNode }) {
    super(props)
    this.state = { hasError: false }
  }

  static getDerivedStateFromError(): { hasError: boolean } {
    return { hasError: true }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    // Last resort: this fires only when nothing else in the tree (including any page-level error
    // handling) caught the exception, so a real console.error trace is the only diagnostic trail
    // an admin has left.
    console.error("Unhandled render error", error, info)
  }

  render() {
    if (!this.state.hasError) return this.props.children
    return (
      <div className="ido" style={{ textAlign: "center", padding: 40 }}>
        <i className="fas fa-8x fa-triangle-exclamation" aria-hidden="true"></i>
        <h2 style={{ marginTop: 16 }}>Something went wrong</h2>
        <p>The page encountered an unexpected error.</p>
        <div style={{ marginTop: 16, display: "flex", gap: 8, justifyContent: "center" }}>
          <input type="button" className="stdbtn" value="Reload" onClick={() => window.location.reload()} />
          <input type="button" className="stdbtn" value="Return to Library" onClick={() => window.location.assign("/")} />
        </div>
      </div>
    )
  }
}
