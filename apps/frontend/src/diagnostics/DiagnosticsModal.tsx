/** The diagnostics dialog (specs/018): one modal, three tabs — overview, issues, bundle. */

import { useState } from "react"
import { useTranslation } from "react-i18next"

import { Modal } from "@/components/common-ui/Display"

import { BundleView } from "./BundleModal"
import { DiagnosticsPanel } from "./DiagnosticsPanel"
import { FeedbackView } from "./FeedbackView"
import { IssuesView } from "./IssuesModal"
import { useIssueSelection } from "./selection"

// Short, parallel nouns rather than "直接反馈"-style descriptions: the tab bar reads as one set.
const TABS = [
  { id: "overview", labelKey: "diagnostics.tab.overview" },
  { id: "issues", labelKey: "diagnostics.tab.issues" },
  { id: "feedback", labelKey: "diagnostics.tab.feedback" },
  { id: "bundle", labelKey: "diagnostics.tab.bundle" },
] as const

type Tab = (typeof TABS)[number]["id"]

export function DiagnosticsModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()
  const [tab, setTab] = useState<Tab>("overview")
  const clearSelection = useIssueSelection((state) => state.clear)
  const close = () => {
    clearSelection()
    onClose()
  }

  return (
    <Modal onClose={close} width={620} textAlign="left">
      <h2 className="share-modal-title">{t("diagnostics.title")}</h2>
      <div className="diag-tabs" role="tablist">
        {TABS.map((entry) => (
          <button
            key={entry.id}
            type="button"
            role="tab"
            aria-selected={tab === entry.id}
            className={`stdbtn diag-tab${tab === entry.id ? " is-on" : ""}`}
            onClick={() => setTab(entry.id)}
          >
            {t(entry.labelKey)}
          </button>
        ))}
      </div>
      <div className="diag-tab-body" role="tabpanel">
        {tab === "overview" && <DiagnosticsPanel />}
        {tab === "issues" && <IssuesView />}
        {tab === "feedback" && <FeedbackView />}
        {tab === "bundle" && <BundleView />}
      </div>
    </Modal>
  )
}
