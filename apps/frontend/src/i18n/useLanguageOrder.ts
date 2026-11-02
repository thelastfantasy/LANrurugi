import { useTranslation } from "react-i18next"

import { useSettings } from "@/api/hooks"

import { languageOrder } from "./index"

/** The viewer's ordered language preference, for choosing among several spellings of one thing.
 *
 * Reads the same setting the interface language comes from, so a title and the surrounding interface
 * never disagree about which language the viewer wanted. Falls back to the language actually in use,
 * which is what `"auto"` resolves to once the browser has been consulted. */
export function useLanguageOrder(): string[] {
  const { i18n } = useTranslation()
  const settings = useSettings()
  const stored = languageOrder(settings.data?.language)
  // The active language still goes last: with `"auto"` there is no stored list at all, and even with
  // one it is the language the viewer is demonstrably reading.
  return stored.length > 0 ? [...stored, i18n.language] : [i18n.language]
}
