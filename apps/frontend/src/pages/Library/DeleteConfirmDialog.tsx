import { useTranslation } from "react-i18next"

import { Confirm } from "@/components/Display"

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
    <Confirm
      danger
      message={
        isTank
          ? t("This will delete this Tankoubon grouping (archives inside it are not deleted).")
          : t("This will delete both metadata and matching files from your system! Please use with caution.")
      }
      confirmLabel={t("Yes, delete it") ?? undefined}
      onConfirm={onConfirm}
      onCancel={onCancel}
    />
  )
}
