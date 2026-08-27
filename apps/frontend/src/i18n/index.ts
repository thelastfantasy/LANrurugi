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

// The 14 languages the legacy LANraragi shipped translation templates for
// (verified by listing `locales/template/*.po` in the legacy source), each
// keyed by its English source string per `research.md` #10.
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
    // FR-019: any string missing from the selected language's resource falls back to English
    // rather than rendering blank — `as` in particular ships with zero translated strings (its
    // legacy .po template has no filled-in msgstr entries at all) so this path is exercised for
    // real, not just hypothetically.
    fallbackLng: "en",
    supportedLngs: SUPPORTED_LANGUAGES.map((l) => l.code),
    interpolation: { escapeValue: false },
    // Translation keys here are literal English source sentences (legacy's own `c.lh("...")`
    // text, carried over verbatim — see `research.md` #10), not a nested namespace/key hierarchy,
    // so i18next's own default separators actively break real keys instead of organizing them:
    // `nsSeparator: ':'` in particular turns any key containing a colon (e.g. `"Lines:"`,
    // `"Category:"`, `"Currently Viewing:"` — 65 of them across `en.json`) into "look up an empty
    // key in a namespace named everything before the colon", which always fails and silently
    // renders nothing. `keySeparator: '.'` has the same failure mode for the (far more common)
    // keys ending in a period, though a real corpus check found it only actually breaks a key
    // when what follows the first `.` happens to itself resolve as a nested path — disabled here
    // too since these resource files are already flat and were never meant to nest.
    nsSeparator: false,
    keySeparator: false,
    detection: {
      order: ["localStorage", "navigator"],
      caches: ["localStorage"],
      lookupLocalStorage: "lanrurugi_language",
    },
  })

/** Applies the server-side `language` setting (Settings page's "Language" dropdown,
 * `LRR_CONFIG.language`) on top of i18next's own `localStorage`/browser-navigator detection —
 * that detector has no idea the server-side setting exists at all, so picking a language there
 * previously did nothing (issue #85). `"auto"` (the default) intentionally leaves the detector's
 * own choice alone. Mounted once in `Layout.tsx` alongside `useApplyTheme`, the same "sync a
 * Settings-page value into a global, non-React API on every settings change" pattern — and, per
 * issue #92, the same auth-gated-`/settings`-401s-while-logged-out fix `useApplyTheme` itself
 * needed: `useSettings()` is disabled outright while logged out (rather than fetched and left to
 * 401, which `client.ts`'s own global 401 handler force-navigates away from regardless of this
 * hook's own readiness to just fall back), falling back to the public `/theme` endpoint's now
 * also-`language`-carrying response instead. */
export function useApplySettingsLanguage() {
  const loginStatus = useLoginStatus()
  // `=== true`, not `?? true` — see `theme.ts::useApplyTheme`'s own identical fix for why treating
  // "login status still loading" as "assume logged in" here would still let this fire (and 401)
  // during that brief window on every logged-out page load.
  const settings = useSettings({ enabled: loginStatus.data?.logged_in === true })
  const publicSettings = usePublicSettings({ enabled: settings.data === undefined })
  const language = settings.data?.language ?? publicSettings.data?.language
  useEffect(() => {
    if (!language) return
    if (language === "auto") {
      // Switching back to "auto" after a specific language was picked previously must actually
      // undo that pick, not just stop re-applying it — `i18next-browser-languagedetector`'s own
      // `caches: ["localStorage"]` (below) wrote the previous explicit selection into
      // `lanrurugi_language`, which its own `detection.order` (`["localStorage", "navigator"]`)
      // then keeps preferring over real browser-language detection forever, since nothing was
      // ever clearing it — a real bug (confirmed live, 2026-08-27): setting the dropdown back to
      // "Automatic" left the UI stuck on whatever language had been explicitly chosen before,
      // with no way back to following the browser's own language short of manually clearing
      // localStorage. `i18n.changeLanguage()` with no argument re-runs the detector's own
      // `order` from scratch and re-caches whatever it lands on.
      localStorage.removeItem("lanrurugi_language")
      void i18n.changeLanguage()
      return
    }
    if (i18n.language === language) return
    void i18n.changeLanguage(language)
  }, [language])
}

export { i18n as default }
