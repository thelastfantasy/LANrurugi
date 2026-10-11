import { useTranslation } from "react-i18next"

import type { PluginType } from "./useWizardSession"

/** The one domain-level shared link input the whole wizard run works from. Rendered once, above
 * the per-type panels, not once per type. */
export function SharedLinksForm({
  links,
  onChange,
  selectedTypes = [],
}: {
  links: string[]
  onChange: (links: string[]) => void
  /** Only used to pick the right hint text: a discovery plugin wants a listing URL or a creator
   * name, not a handful of individual work pages. */
  selectedTypes?: PluginType[]
}) {
  const { t } = useTranslation()
  const discoveryOnly =
    selectedTypes.length > 0 && selectedTypes.every((type) => type === "discovery")

  return (
    <label style={{ display: "block", marginTop: 8 }}>
      {t(discoveryOnly ? "pluginWizard.sharedLinksHintDiscovery" : "pluginWizard.sharedLinksHint")}
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
