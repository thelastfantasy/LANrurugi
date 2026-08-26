import { useTranslation } from "react-i18next"

/** The one domain-level shared link input the whole wizard run works from (`WizardSession.
 * sharedLinks`'s own doc comment has the full rationale) — target page links for metadata/
 * download's generate+trial-run, the default reference URL login analysis inspects, and any
 * purely auxiliary/API-doc links, all in one box. Rendered once, above the per-type panels, not
 * once per type. */
export function SharedLinksForm({ links, onChange }: { links: string[]; onChange: (links: string[]) => void }) {
  const { t } = useTranslation()

  return (
    <label style={{ display: "block", marginTop: 8 }}>
      {t("pluginWizard.sharedLinksHint")}
      <textarea
        className="stdinput"
        value={links.join("\n")}
        // Deliberately does NOT trim/filter-empty here — doing so on every keystroke fed a
        // stale-looking value back into this controlled textarea's own `value` prop (a newly
        // typed blank line, or trailing whitespace mid-edit, was immediately stripped out of the
        // stored `links` array, so the very next render re-synced the DOM to a shorter string
        // than what the user just typed — from the user's perspective, pressing Enter to start a
        // new line appeared to do nothing). Store the raw split lines as-is; trimming/dropping
        // blank entries happens once, downstream, wherever `links` is actually sent to the
        // backend (`GenerationStep.tsx`/`TrialRunResult.tsx`), not on every render here.
        onChange={(e) => onChange(e.target.value.split("\n"))}
        placeholder={t("pluginWizard.sharedLinksPlaceholder") ?? undefined}
        rows={5}
        // `.stdinput`'s own theme CSS caps `max-width: 450px` (meant for a normal single-line
        // text input) — without overriding it here too, `width: 100%` alone still gets capped
        // well short of the wizard's own 720px step container, leaving a large empty gap on the
        // right (real user feedback, 2026-08-26).
        style={{ width: "100%", maxWidth: "none", display: "block" }}
      />
    </label>
  )
}
