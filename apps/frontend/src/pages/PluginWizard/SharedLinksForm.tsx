import { useTranslation } from "react-i18next"

/** The one domain-level shared link input the whole wizard run works from. Rendered once, above
 * the per-type panels, not once per type. */
export function SharedLinksForm({ links, onChange }: { links: string[]; onChange: (links: string[]) => void }) {
  const { t } = useTranslation()

  return (
    <label style={{ display: "block", marginTop: 8 }}>
      {t("pluginWizard.sharedLinksHint")}
      <textarea
        className="stdinput"
        value={links.join("\n")}
        // Deliberately does NOT trim/filter-empty here — doing so on every keystroke stripped
        // blank lines being typed, making Enter appear to do nothing. Trim downstream instead.
        onChange={(e) => onChange(e.target.value.split("\n"))}
        placeholder={t("pluginWizard.sharedLinksPlaceholder") ?? undefined}
        rows={5}
        style={{ width: "100%", maxWidth: "none", display: "block" }}
      />
    </label>
  )
}
