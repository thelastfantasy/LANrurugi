import { Component, type ErrorInfo, type ReactNode } from "react"

import { reportError } from "@/diagnostics/sdk"
import i18n from "@/i18n"

/** Contains a render crash to one panel of a multi-panel page, so the rest of the page and the way
 * out of the broken feature stay usable (#283). Reported with the panel id as a tag. */
export class SectionErrorBoundary extends Component<
  {
    section: string
    /** `li` when the boundary stands in for a whole settings `<li>` section. */
    as?: "li" | "div"
    children: ReactNode
  },
  { error: Error | null }
> {
  state: { error: Error | null } = { error: null }

  static getDerivedStateFromError(error: Error): { error: Error } {
    return { error }
  }

  componentDidCatch(error: Error, info: ErrorInfo) {
    console.error(`Render error in section "${this.props.section}"`, error, info)
    reportError(error, { section: this.props.section })
  }

  render() {
    const { error } = this.state
    if (!error) return this.props.children
    const t = i18n.t.bind(i18n)
    const body = (
      <div className="section-error" role="alert" style={{ padding: "8px 12px" }}>
        <p style={{ margin: "0 0 4px" }}>
          <i className="fas fa-triangle-exclamation" aria-hidden="true"></i> {t("sectionError.title")}
        </p>
        <p style={{ margin: "0 0 8px", opacity: 0.8, fontSize: "0.9em", overflowWrap: "anywhere" }}>
          {error.message}
        </p>
        <p style={{ margin: "0 0 8px", opacity: 0.8, fontSize: "0.9em" }}>{t("sectionError.hint")}</p>
        <input
          type="button"
          className="stdbtn"
          value={t("common.retry")}
          onClick={() => this.setState({ error: null })}
        />
      </div>
    )
    if (this.props.as !== "li") return body
    return (
      <li className="option-flyout" data-section-id={this.props.section}>
        <div className="collapsible-title active" style={{ userSelect: "none" }}>
          <i className="fa fa-triangle-exclamation" aria-hidden="true"></i> {this.props.section}
        </div>
        <div className="collapsible-body">{body}</div>
      </li>
    )
  }
}
