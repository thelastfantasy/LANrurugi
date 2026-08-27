import type { CSSProperties } from "react"
import { useTranslation } from "react-i18next"

import { Select } from "@/components/common-ui/Form/Select"
import { SUPPORTED_LANGUAGES } from "@/i18n"

export function LanguageSelector({ variant, style }: { variant?: "stdbtn" | "favtag-btn"; style?: CSSProperties }) {
  const { i18n } = useTranslation()

  return (
    <Select
      ariaLabel="Language"
      value={i18n.resolvedLanguage ?? i18n.language}
      onValueChange={(lang) => void i18n.changeLanguage(lang)}
      items={SUPPORTED_LANGUAGES.map(({ code, nativeName }) => ({ value: code, label: nativeName }))}
      variant={variant}
      style={style}
    />
  )
}
