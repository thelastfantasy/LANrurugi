import type { CSSProperties } from "react"
import { useTranslation } from "react-i18next"

import { useUpdateSettings } from "@/api/hooks"
import { Select } from "@/components/common-ui/Form/Select"
import { SUPPORTED_LANGUAGES } from "@/i18n"

/** Switches i18next immediately and persists to `settings.language`, keeping this in sync with
 * the Global section's own language dropdown, which reads the same field. */
export function LanguageSelector({ variant, style }: { variant?: "stdbtn" | "favtag-btn"; style?: CSSProperties }) {
  const { i18n } = useTranslation()
  const updateSettings = useUpdateSettings()

  return (
    <Select
      ariaLabel="Language"
      value={i18n.resolvedLanguage ?? i18n.language}
      onValueChange={(lang) => {
        void i18n.changeLanguage(lang)
        void updateSettings.mutateAsync({ language: lang })
      }}
      items={SUPPORTED_LANGUAGES.map(({ code, nativeName }) => ({ value: code, label: nativeName }))}
      variant={variant}
      style={style}
    />
  )
}
