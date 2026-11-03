/** The settings page's left-column entry point for the diagnostics dialog (specs/018). */

import { useState } from "react"
import { useTranslation } from "react-i18next"

import { Button } from "@/components/common-ui/Form"

import { DiagnosticsModal } from "./DiagnosticsModal"

export function DiagnosticsButton() {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)

  return (
    <>
      <Button id="diagnostics" onClick={() => setOpen(true)}>
        {t("diagnostics.open")}
      </Button>
      {open && <DiagnosticsModal onClose={() => setOpen(false)} />}
    </>
  )
}
