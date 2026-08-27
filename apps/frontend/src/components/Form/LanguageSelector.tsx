import type { CSSProperties } from "react"
import { useTranslation } from "react-i18next"

import { useUpdateSettings } from "@/api/hooks"
import { Select } from "@/components/common-ui/Form/Select"
import { SUPPORTED_LANGUAGES } from "@/i18n"

/** The only place in the app this renders is the Settings page's sidebar (always behind
 * `RequireAuth`) — picking a language here both switches i18next immediately (so the change is
 * visible without a reload) *and* persists it to the server-side `settings.language` field,
 * keeping it in sync with the Global section's own "Language" dropdown (`GlobalSection.tsx`),
 * which reads that same field. Before this, picking a language here only ever called
 * `i18n.changeLanguage()` — the Global section's dropdown stayed on whatever `settings.language`
 * was previously, and `useApplySettingsLanguage`'s own effect (`i18n/index.ts`) could re-apply
 * that stale server value over top of this selector's pick the next time it re-ran, silently
 * reverting the choice — confirmed live, 2026-08-27, as a real "language selector doesn't
 * actually save the setting" report. */
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
