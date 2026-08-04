import { useTranslation } from "react-i18next"

import { Z_OVERLAY_BACKDROP, Z_OVERLAY_CONTENT } from "@/theme"

export function DeleteConfirmDialog({
  isTank,
  onConfirm,
  onCancel,
}: {
  isTank: boolean
  onConfirm: () => void
  onCancel: () => void
}) {
  const { t } = useTranslation()
  return (
    <>
      <div style={{ position: "fixed", inset: 0, zIndex: Z_OVERLAY_BACKDROP, background: "rgba(0,0,0,0.4)" }} onClick={onCancel} />
      <div
        style={{
          position: "fixed",
          top: "50%",
          left: "50%",
          transform: "translate(-50%, -50%)",
          zIndex: Z_OVERLAY_CONTENT,
          width: 360,
          padding: 20,
          textAlign: "center",
          background: "#fff",
          border: "1px solid #bebebe",
          borderRadius: ".2em",
          boxShadow: "0 2px 5px rgba(0,0,0,.5)",
        }}
      >
        <i className="fa fa-exclamation-triangle fa-2x" style={{ color: "#d33" }} aria-hidden="true"></i>
        <p>
          {isTank
            ? t("This will delete this Tankoubon grouping (archives inside it are not deleted).")
            : t("This will delete both metadata and matching files from your system! Please use with caution.")}
        </p>
        <div style={{ display: "flex", justifyContent: "center", gap: 12, marginTop: 12 }}>
          <input type="button" className="stdbtn" value={t("Cancel") ?? undefined} onClick={onCancel} />
          <input
            type="button"
            className="stdbtn"
            style={{ background: "#d33", color: "white" }}
            value={t("Yes, delete it") ?? undefined}
            onClick={onConfirm}
          />
        </div>
      </div>
    </>
  )
}
