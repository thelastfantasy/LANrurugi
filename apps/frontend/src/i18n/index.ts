import i18n from "i18next"
import LanguageDetector from "i18next-browser-languagedetector"
import { useEffect } from "react"
import { initReactI18next } from "react-i18next"

import { useLoginStatus, usePublicSettings, useSettings } from "@/api/hooks"

import asLocale from "./locales/as.json"
import de from "./locales/de.json"
import en from "./locales/en.json"
import es from "./locales/es.json"
import fr from "./locales/fr.json"
import id from "./locales/id.json"
import it from "./locales/it.json"
import ja from "./locales/ja.json"
import ko from "./locales/ko.json"
import nbNO from "./locales/nb_NO.json"
import pt from "./locales/pt.json"
import vi from "./locales/vi.json"
import zh from "./locales/zh.json"
import zhHant from "./locales/zh_Hant.json"

// The 14 languages legacy LANraragi shipped translation templates for, each keyed by its
// English source string.
export const SUPPORTED_LANGUAGES = [
  { code: "en", nativeName: "English" },
  { code: "ja", nativeName: "日本語" },
  { code: "zh", nativeName: "简体中文" },
  { code: "zh_Hant", nativeName: "繁體中文" },
  { code: "ko", nativeName: "한국어" },
  { code: "fr", nativeName: "Français" },
  { code: "de", nativeName: "Deutsch" },
  { code: "es", nativeName: "Español" },
  { code: "it", nativeName: "Italiano" },
  { code: "pt", nativeName: "Português" },
  { code: "vi", nativeName: "Tiếng Việt" },
  { code: "id", nativeName: "Bahasa Indonesia" },
  { code: "nb_NO", nativeName: "Norsk Bokmål" },
  { code: "as", nativeName: "অসমীয়া" },
] as const

void i18n
  .use(LanguageDetector)
  .use(initReactI18next)
  .init({
    resources: {
      en: { translation: en },
      ja: { translation: ja },
      zh: { translation: zh },
      zh_Hant: { translation: zhHant },
      ko: { translation: ko },
      fr: { translation: fr },
      de: { translation: de },
      es: { translation: es },
      it: { translation: it },
      pt: { translation: pt },
      vi: { translation: vi },
      id: { translation: id },
      nb_NO: { translation: nbNO },
      as: { translation: asLocale },
    },
    // Falls back to English rather than rendering blank — `as` ships with zero translated
    // strings, so this path is exercised for real.
    fallbackLng: "en",
    supportedLngs: SUPPORTED_LANGUAGES.map((l) => l.code),
    interpolation: { escapeValue: false },
    // Keys are literal English source sentences, not a namespace/key hierarchy — i18next's default
    // separators would break real keys containing a colon or period (e.g. "Lines:", "Category:").
    nsSeparator: false,
    keySeparator: false,
    detection: {
      order: ["localStorage", "navigator"],
      caches: ["localStorage"],
      lookupLocalStorage: "lanrurugi_language",
    },
  })

/** Applies the server-side `language` setting on top of i18next's own detection, which has no
 * idea it exists. Falls back to the public `/theme` endpoint's `language` field while logged out. */
export function useApplySettingsLanguage() {
  const loginStatus = useLoginStatus()
  const settings = useSettings({ enabled: loginStatus.data?.logged_in === true })
  const publicSettings = usePublicSettings({ enabled: settings.data === undefined })
  const language = settings.data?.language ?? publicSettings.data?.language
  useEffect(() => {
    if (!language) return
    if (language === "auto") {
      // Undoes a previous explicit pick, not just stops re-applying it — otherwise the detector's
      // own localStorage cache would keep preferring it forever over real navigator detection.
      localStorage.removeItem("lanrurugi_language")
      void i18n.changeLanguage()
      return
    }
    if (i18n.language === language) return
    void i18n.changeLanguage(language)
  }, [language])
}

export { i18n as default }
