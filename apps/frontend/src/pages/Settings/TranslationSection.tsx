import type { Dispatch } from "react"
import { useTranslation } from "react-i18next"

import { NumberInput } from "@/components/common-ui/Form"
import { CollapsibleSection } from "@/components/Display"
import { FONT_SIZE_SM } from "@/theme"
import { UsagePanel } from "@/translation/components/UsagePanel"
import type { CloudProvider } from "@/translation/types"

import { Row } from "./shared"
import type { TranslationSectionAction, TranslationSectionState } from "./useTranslationSectionState"

/**
 * On-page translation settings (T024/T026/T027/T059, `specs/004-ocr-manga-translation`).
 *
 * Two halves with deliberately different storage (FR-003, research.md §8): cloud provider settings
 * go to the server — so the API key stays server-side (Principle V) and the choice follows the user
 * across devices — while a locally-hosted backend is written to `localStorage` and never leaves
 * this device, because a loopback address means a different machine on every device.
 *
 * Target language is its own control, distinct from the Phase 1 interface-language setting (FR-004),
 * and lives in the reader itself rather than here (see `ToggleButtonMenu` in `Reader.tsx`) — this
 * section only configures which backend a translation, wherever it's turned on, would use.
 *
 * A pure controlled component, like every other `SettingsPage` section: state lives in
 * `useTranslationSectionState` at `SettingsPage`'s top level, and `SettingsPage`'s single save
 * button submits it alongside the rest of Phase 1's settings — see that hook's own docs for why a
 * reducer rather than a flat pile of primitives.
 */
export function TranslationSection({
  state,
  dispatch,
}: {
  state: TranslationSectionState
  dispatch: Dispatch<TranslationSectionAction>
}) {
  const { t } = useTranslation()
  const { settings, category, local, apiKey } = state

  if (!settings) return null

  const update = (patch: Partial<typeof settings>) => dispatch({ kind: "patched", patch })

  // Derived from the currently *selected* provider (which may not be saved yet), not
  // `settings.usesGlobalKey` — that field reflects the provider already persisted on the server,
  // so it wouldn't update the moment a user picks "DeepSeek" in the dropdown, only after a round
  // trip through Save. The project-wide key isn't overridable from here, so both the input and the
  // save payload drop out whenever it applies.
  const usesGlobalKey =
    category === "cloud" && settings.provider === "deepseek" && settings.globalDeepseekKeySet

  return (
    <CollapsibleSection id="translation" icon="fa-language" title={t("translation.title")}>
      <div className="settings-table" style={{ margin: "auto", fontSize: FONT_SIZE_SM }}>
        <Row label={t("translation.backend")} noHelp>
          <select
            className="stdinput"
            style={{ width: "100%" }}
            value={category}
            onChange={(e) => dispatch({ kind: "categoryChanged", category: e.target.value as "cloud" | "local" })}
          >
            <option value="cloud">{t("translation.backendCloud")}</option>
            <option value="local">{t("translation.backendLocal")}</option>
          </select>
        </Row>

        {category === "cloud" ? (
          <>
            <Row label={t("translation.provider")} noHelp>
              <select
                className="stdinput"
                style={{ width: "100%" }}
                value={settings.provider ?? ""}
                onChange={(e) => update({ provider: (e.target.value || null) as CloudProvider | null })}
              >
                <option value="">—</option>
                <option value="deepseek">DeepSeek</option>
                <option value="anthropic">Anthropic</option>
                <option value="openai-compatible">OpenAI-compatible</option>
              </select>
            </Row>

            {usesGlobalKey ? (
              // The project-wide DeepSeek key already covers this provider, and a second one
              // would just compete with it — so there is no input to show at all.
              <Row label={t("translation.apiKey")} noHelp>
                {t("translation.apiKeyGlobal")}
              </Row>
            ) : (
              <Row label={t("translation.apiKey")}>
                <input
                  className="stdinput"
                  style={{ width: "100%" }}
                  type="password"
                  autoComplete="off"
                  value={apiKey}
                  placeholder={settings.credentialSet ? "••••••••" : ""}
                  onChange={(e) => dispatch({ kind: "apiKeyChanged", apiKey: e.target.value })}
                />
                <br />
                {t("translation.apiKeyHelp")}
                {settings.credentialSet ? ` ${t("translation.apiKeySet")}` : ""}
              </Row>
            )}

            <Row label={t("translation.model")} noHelp>
              <input
                className="stdinput"
                style={{ width: "100%" }}
                type="text"
                value={settings.model ?? ""}
                onChange={(e) => update({ model: e.target.value || null })}
              />
            </Row>
          </>
        ) : (
          <>
            <Row label={t("translation.endpoint")}>
              <input
                className="stdinput"
                style={{ width: "100%" }}
                type="text"
                value={local.endpoint}
                placeholder="http://127.0.0.1:11434/v1"
                onChange={(e) => dispatch({ kind: "localChanged", local: { ...local, endpoint: e.target.value } })}
              />
              <br />
              {t("translation.localEndpointHelp")}
            </Row>

            <Row label={t("translation.model")} noHelp>
              <input
                className="stdinput"
                style={{ width: "100%" }}
                type="text"
                value={local.model}
                onChange={(e) => dispatch({ kind: "localChanged", local: { ...local, model: e.target.value } })}
              />
            </Row>
          </>
        )}

        <Row label={t("translation.lookaheadPages")}>
          <NumberInput
            style={{ width: "100%" }}
            min={0}
            max={20}
            value={settings.lookaheadPages}
            onValueChange={(v) => update({ lookaheadPages: v })}
          />
          <br />
          {t("translation.lookaheadHelp")}
        </Row>

        {category === "cloud" && settings.provider && <UsagePanel />}
      </div>
    </CollapsibleSection>
  )
}
