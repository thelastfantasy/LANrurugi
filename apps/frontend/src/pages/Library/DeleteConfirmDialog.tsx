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
          ? t("library.thisWillDeleteThisTankoubon")
          : t("common.thisWillDeleteBothMetadata")
      }
      confirmLabel={t("library.yesDeleteIt") ?? undefined}
      onConfirm={onConfirm}
      onCancel={onCancel}
    />
  )
}
