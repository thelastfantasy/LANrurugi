import { useEffect, useState } from "react"
import { useTranslation } from "react-i18next"

import {
  deleteGlossaryEntry,
  fetchGlossary,
  type GlossaryEntry,
  updateGlossaryEntry,
} from "../translation/api"

/**
 * View, edit, and delete individual terminology-glossary entries (T032b, FR-007d).
 *
 * Per-entry operations only — there is intentionally no "clear all" button. Entries are captured
 * automatically and are independent of one another, so a bulk wipe would discard correct (often
 * hand-corrected) entries to fix one wrong one. An edit takes effect on the next translation
 * request; already-rendered pages keep what they were translated with until re-translated.
 *
 * A term usually has exactly one source candidate; a same-named term captured from unrelated
 * archives/chapters of this volume (issue #105 — e.g. a Tankoubon bundling unrelated series, or an
 * anthology archive with several short stories) can have more than one, each independently
 * translated. The edit/delete endpoints still only identify an entry by (volume, term) — editing an
 * ambiguous term is refused by the backend (409), so this view surfaces every candidate's own
 * source but only offers the edit control when there's exactly one.
 */
export function TerminologyGlossaryControl({ volumeId }: { volumeId: string }) {
  const { t } = useTranslation()
  const [entries, setEntries] = useState<Record<string, GlossaryEntry[]> | null>(null)
  const [drafts, setDrafts] = useState<Record<string, string>>({})

  useEffect(() => {
    let cancelled = false
    void fetchGlossary(volumeId)
      .then((glossary) => {
        if (!cancelled) setEntries(glossary.entries)
      })
      .catch(() => {
        // Informational only.
      })
    return () => {
      cancelled = true
    }
  }, [volumeId])

  if (!entries) return null

  const terms = Object.keys(entries)

  const onSave = async (term: string, current: string) => {
    const value = (drafts[term] ?? current).trim()
    if (!value) return
    await updateGlossaryEntry(volumeId, term, value)
    setEntries((prev) => {
      if (!prev) return prev
      const existing = prev[term]
      if (!existing || existing.length !== 1) return prev
      return { ...prev, [term]: [{ ...existing[0], translation: value }] }
    })
    setDrafts((prev) => {
      const next = { ...prev }
      delete next[term]
      return next
    })
  }

  const onDelete = async (term: string) => {
    if (!window.confirm(t("translation.glossary.deleteConfirm"))) return
    await deleteGlossaryEntry(volumeId, term)
    setEntries((prev) => {
      if (!prev) return prev
      const next = { ...prev }
      delete next[term]
      return next
    })
  }

  /** "档案名 — 章节名", falling back to just the archive id when there's no chapter. */
  const sourceLabel = (candidate: GlossaryEntry): string =>
    candidate.chapter_name ? `${candidate.archive_id} — ${candidate.chapter_name}` : candidate.archive_id

  return (
    <div className="terminology-glossary">
      <h3>{t("translation.glossary.title")}</h3>
      <p className="helptext">{t("translation.glossary.help")}</p>

      {terms.length === 0 ? (
        <p className="helptext">{t("translation.glossary.empty")}</p>
      ) : (
        <table className="terminology-glossary__table">
          <thead>
            <tr>
              <th>{t("translation.glossary.sourceTerm")}</th>
              <th>{t("translation.glossary.translation")}</th>
              <th />
            </tr>
          </thead>
          <tbody>
            {terms.map((term) => {
              const candidates = entries[term]
              if (candidates.length === 1) {
                const [only] = candidates
                return (
                  <tr key={term}>
                    <td>{term}</td>
                    <td>
                      <input
                        type="text"
                        value={drafts[term] ?? only.translation}
                        onChange={(e) => setDrafts({ ...drafts, [term]: e.target.value })}
                      />
                    </td>
                    <td>
                      <button
                        type="button"
                        className="stdbtn"
                        onClick={() => void onSave(term, only.translation)}
                      >
                        {t("translation.glossary.save")}
                      </button>
                      <button type="button" className="stdbtn" onClick={() => void onDelete(term)}>
                        {t("translation.glossary.delete")}
                      </button>
                    </td>
                  </tr>
                )
              }
              // More than one source: not editable through this per-(volume, term) endpoint (the
              // backend refuses with 409) — shown read-only, each candidate labeled by its own
              // source so it's clear these are deliberately different translations, not a bug.
              return (
                <tr key={term}>
                  <td>{term}</td>
                  <td colSpan={2}>
                    <ul className="terminology-glossary__ambiguous">
                      {candidates.map((candidate, i) => (
                        <li key={i}>
                          <span className="terminology-glossary__source">{sourceLabel(candidate)}</span>
                          {": "}
                          {candidate.translation}
                        </li>
                      ))}
                    </ul>
                  </td>
                </tr>
              )
            })}
          </tbody>
        </table>
      )}
    </div>
  )
}

export default TerminologyGlossaryControl
