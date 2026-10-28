/** The diagnostics dialog (specs/018): one modal, three tabs — overview, issues, bundle. */

import { useState } from "react"
import { useTranslation } from "react-i18next"

import { Modal } from "@/components/common-ui/Display"

import { BundleView } from "./BundleModal"
import { DiagnosticsPanel } from "./DiagnosticsPanel"
import { IssuesView } from "./IssuesModal"

const TABS = [
  { id: "overview", labelKey: "diagnostics.title" },
  { id: "issues", labelKey: "diagnostics.issues.title" },
  { id: "bundle", labelKey: "diagnostics.bundle.title" },
] as const

type Tab = (typeof TABS)[number]["id"]

export function DiagnosticsModal({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()
  const [tab, setTab] = useState<Tab>("overview")

  return (
    <Modal onClose={onClose} width={620} textAlign="left">
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
        {tab === "bundle" && <BundleView />}
      </div>
    </Modal>
  )
}
