import type { CSSProperties } from "react"
import { useTranslation } from "react-i18next"

import { useSettings, useUpdateSettings } from "@/api/hooks"
import { Select } from "@/components/common-ui/Form/Select"
import { languageOrder, languageOrderAfterPick, preferredInterfaceLanguage, SUPPORTED_LANGUAGES } from "@/i18n"

/** The option standing for "follow the browser", distinct from every language code. */
const AUTO = "auto"

/** Quick switch for the interface language.
 *
 * There is only one `language` setting, and the Global section's list is its full editor: either
 * "auto" or an ordered list (`"zh,ja,en"`), whose first entry this build has translations for is the
 * interface language. So picking a language here *moves it to the front* of that list instead of
 * replacing the list with it — replacing, as this used to, also threw away the other entries, which
 * decide which spelling of a work's title is preferred. "自动" is offered here too; without it the
 * dropdown showed the browser-detected language as though it had been chosen. */
export function LanguageSelector({ variant, style }: { variant?: "stdbtn" | "favtag-btn"; style?: CSSProperties }) {
  const { i18n, t } = useTranslation()
  const settings = useSettings()
  const updateSettings = useUpdateSettings()

  const stored = settings.data?.language
  const isAuto = !stored || stored === AUTO
  // What the interface is actually showing, which is what this control claims to switch.
  const current = isAuto
    ? AUTO
    : (preferredInterfaceLanguage(stored) ?? languageOrder(stored)[0] ?? AUTO)

  return (
    <Select
      ariaLabel="Language"
      value={current}
      onValueChange={(picked) => {
        // Persist first: the settings hook then re-applies the list (and, for "auto", re-runs browser
        // detection after clearing i18next's own cached pick).
        void updateSettings.mutateAsync({ language: languageOrderAfterPick(stored, picked) })
        if (picked !== AUTO) void i18n.changeLanguage(picked)
      }}
      items={[
        { value: AUTO, label: t("settings.automaticBrowserDefault") },
        ...SUPPORTED_LANGUAGES.map(({ code, nativeName }) => ({ value: code, label: nativeName })),
      ]}
      variant={variant}
      style={style}
    />
  )
}
