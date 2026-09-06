import { Component, type ErrorInfo, type ReactNode } from "react"

/** Catch-all for a render crash (class-only API) — wraps `<App />` so an unhandled exception
 * renders this recovery UI instead of a blank page. Deliberately outside Layout/Router context. */
export class ErrorBoundary extends Component<{ children: ReactNode }, { hasError: boolean }> {
  constructor(props: { children: ReactNode }) {
    super(props)
    this.state = { hasError: false }
  }

  static getDerivedStateFromError(): { hasError: boolean } {
    return { hasError: true }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
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
