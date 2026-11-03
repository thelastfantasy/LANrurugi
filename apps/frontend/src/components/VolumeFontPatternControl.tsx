import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import { fetchFontPattern, type FontPatternResponse,resetFontPattern } from "../translation/api"

/**
 * Shows a volume's established lettering styles and lets the user reset them (T037, FR-010).
 *
 * The reset exists for one specific failure: a volume whose pattern locked early on
 * unrepresentative pages stays wrong for the whole volume otherwise. Unlike the glossary — where
 * entries are independent and only the bad one should go — the pattern is one derived thing, so
 * clearing it wholesale is the right granularity here.
 */
export function VolumeFontPatternControl({ volumeId }: { volumeId: string }) {
  const { t } = useTranslation()
  const [pattern, setPattern] = useState<FontPatternResponse | null>(null)
  const [status, setStatus] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    void fetchFontPattern(volumeId)
      .then((loaded) => {
        if (!cancelled) setPattern(loaded)
      })
      .catch(() => {
        // Informational only — a load failure shouldn't disturb the surrounding page.
      })
    return () => {
      cancelled = true
    }
  }, [volumeId])

  if (!pattern) return null

  const onReset = async () => {
    if (!window.confirm(t("translation.fontPattern.resetConfirm"))) return
    const reset = await resetFontPattern(volumeId)
    setPattern(reset)
    setStatus(t("translation.fontPattern.resetDone"))
  }

  return (
    <div className="volume-font-pattern">
      <h3>{t("translation.fontPattern.title")}</h3>

      <p className="helptext">
        {pattern.isLocked
          ? t("translation.fontPattern.locked", { count: pattern.voteTotal ?? 0 })
          : t("translation.fontPattern.learning")}
      </p>

      {pattern.goldenSet.length > 0 && (
        <ul className="volume-font-pattern__fonts">
          {pattern.goldenSet.map((font) => (
            <li key={font}>{font}</li>
          ))}
        </ul>
      )}

      <button type="button" className="stdbtn" onClick={() => void onReset()}>
        {t("translation.fontPattern.reset")}
      </button>
      {status && <span className="volume-font-pattern__status">{status}</span>}
    </div>
  )
}

export default VolumeFontPatternControl
