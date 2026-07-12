import { useTranslation } from 'react-i18next'

import { SUPPORTED_LANGUAGES } from '../i18n'

export default function LanguageSelector() {
  const { i18n } = useTranslation()

  return (
    <select
      aria-label="Language"
      value={i18n.resolvedLanguage ?? i18n.language}
      onChange={(e) => void i18n.changeLanguage(e.target.value)}
      className="favtag-btn"
    >
      {SUPPORTED_LANGUAGES.map(({ code, nativeName }) => (
        <option key={code} value={code}>
          {nativeName}
        </option>
      ))}
    </select>
  )
}
