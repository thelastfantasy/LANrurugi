import { Component, type ErrorInfo, type ReactNode } from "react"

import { captureRenderError } from "@/diagnostics/sdk"
import i18n from "@/i18n"

/** Catch-all for a render crash (class-only API) — wraps `<App />` so an unhandled exception
 * renders this recovery UI instead of a blank page. Deliberately outside Layout/Router context, so
 * it resets itself on browser back/forward: otherwise the error page outlived every navigation away
 * from the page that crashed (#293). */
export class ErrorBoundary extends Component<{ children: ReactNode }, { hasError: boolean }> {
  constructor(props: { children: ReactNode }) {
    super(props)
    this.state = { hasError: false }
  }

  static getDerivedStateFromError(): { hasError: boolean } {
    return { hasError: true }
  }

  componentDidMount() {
    window.addEventListener("popstate", this.reset)
  }

  componentWillUnmount() {
    window.removeEventListener("popstate", this.reset)
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error("Unhandled render error", error, info)
    captureRenderError(error)
  }

  // The router reads the new location itself once its tree is rendered again.
  private reset = () => {
    if (this.state.hasError) this.setState({ hasError: false })
  }

  render() {
    if (!this.state.hasError) return this.props.children
    const t = i18n.t.bind(i18n)
    return (
      <div className="ido" style={{ textAlign: "center", padding: 40 }}>
        <i className="fas fa-8x fa-triangle-exclamation" aria-hidden="true"></i>
        <h2 style={{ marginTop: 16 }}>{t("errorPage.title")}</h2>
        <p>{t("errorPage.description")}</p>
        <div style={{ marginTop: 16, display: "flex", gap: 8, justifyContent: "center", flexWrap: "wrap" }}>
          {window.history.length > 1 && (
            <input type="button" className="stdbtn" value={t("errorPage.back")} onClick={() => window.history.back()} />
          )}
          <input type="button" className="stdbtn" value={t("common.refresh")} onClick={() => window.location.reload()} />
          <input
            type="button"
            className="stdbtn"
            value={t("common.returnToLibrary")}
            onClick={() => window.location.assign("/")}
          />
        </div>
      </div>
    )
  }
}
