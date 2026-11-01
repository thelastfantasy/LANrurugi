import { useState } from "react"
import { useTranslation } from "react-i18next"

import { Confirm } from "@/components/Display"

export type TankDeleteMode = "dissolve" | "recycle"

/** Two-choice confirmation for deleting a Tankoubon: dissolve the grouping (archives stay) or move
 * the whole thing into the recycle bin. The Library grid's own dialog carries the same choice
 * inline; this is for the editor and reader entry points. */
export function TankDeleteChoiceDialog({
  title,
  onConfirm,
  onCancel,
}: {
  title?: string
  onConfirm: (mode: TankDeleteMode) => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  const [mode, setMode] = useState<TankDeleteMode>("dissolve")

  return (
    <Confirm
      danger
      message={
        <>
          <span style={{ display: "block", marginBottom: 8 }}>
            {title ?? t("library.thisWillDeleteThisTankoubon")}
          </span>
          <span style={{ display: "block", textAlign: "left", fontWeight: "normal", fontSize: "0.95em" }}>
            <label style={{ display: "flex", gap: 6, alignItems: "flex-start", marginBottom: 6 }}>
              <input
                type="radio"
                name="tank-delete-mode"
                checked={mode === "recycle"}
                onChange={() => setMode("recycle")}
              />
              <span>{t("library.recycleWholeTankoubon")}</span>
            </label>
            <label style={{ display: "flex", gap: 6, alignItems: "flex-start" }}>
              <input
                type="radio"
                name="tank-delete-mode"
                checked={mode === "dissolve"}
                onChange={() => setMode("dissolve")}
              />
              <span>{t("library.dissolveTankoubonOnly")}</span>
            </label>
          </span>
        </>
      }
      confirmLabel={t("library.yesDeleteIt") ?? undefined}
      onConfirm={() => onConfirm(mode)}
      onCancel={onCancel}
    />
  )
}
