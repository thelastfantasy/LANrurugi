import { useState } from "react"
import { useTranslation } from "react-i18next"

import { sendJson } from "@/api/client"
import { toast } from "@/toast"

import type { DomainLookupResult, TypeCoverage } from "./useWizardSession"

/** Raw `snake_case` wire shape of `/plugin-wizard/lookup`; mapped below to the frontend's
 * `camelCase` `DomainLookupResult`/`TypeCoverage` rather than asserted directly. */
interface WireTypeCoverage {
  covered: boolean
  namespace?: string
  declared_namespace?: string
  source_code?: string
}

interface WireDomainLookupResult {
  login: WireTypeCoverage
  metadata: WireTypeCoverage
  download: WireTypeCoverage
}

function toTypeCoverage(wire: WireTypeCoverage): TypeCoverage {
  if (!wire.covered || !wire.namespace || !wire.declared_namespace || wire.source_code === undefined) {
    return { covered: false }
  }
  return {
    covered: true,
    namespace: wire.namespace,
    declaredNamespace: wire.declared_namespace,
    sourceCode: wire.source_code,
    // `custom/` is `CUSTOM_PLUGIN_DIR` on the Rust side — only wizard-saved plugins land there.
    coverageSource: wire.namespace.startsWith("custom/") ? "ai-generated" : "built-in",
  }
}

function toDomainLookupResult(wire: WireDomainLookupResult): DomainLookupResult {
  return {
    login: toTypeCoverage(wire.login),
    metadata: toTypeCoverage(wire.metadata),
    download: toTypeCoverage(wire.download),
  }
}

/** The wizard's entry point — a domain input that triggers `POST /plugin-wizard/lookup` and hands
 * the result to the parent session state. */
export function DomainLookupStep({
  initialDomain,
  onLookupSucceeded,
}: {
  initialDomain?: string
  onLookupSucceeded: (domain: string, result: DomainLookupResult) => void
}) {
  const { t } = useTranslation()
  const [domain, setDomain] = useState(initialDomain ?? "")
  const [loading, setLoading] = useState(false)

  async function submit(e: React.FormEvent) {
    e.preventDefault()
    const trimmed = domain.trim()
    if (!trimmed) return
    setLoading(true)
    try {
      const wireResult = await sendJson<WireDomainLookupResult>("POST", "/plugin-wizard/lookup", {
        domain: trimmed,
      })
      onLookupSucceeded(trimmed, toDomainLookupResult(wireResult))
    } catch (err) {
      toast({ heading: t("pluginWizard.lookupFailed") ?? undefined, text: String(err), icon: "error" })
    } finally {
      setLoading(false)
    }
  }

  return (
    <form onSubmit={(e) => void submit(e)} style={{ display: "flex", gap: 8, alignItems: "center" }}>
      <input
        className="stdinput"
        type="text"
        value={domain}
        onChange={(e) => setDomain(e.target.value)}
        placeholder={t("pluginWizard.domainInputPlaceholder") ?? undefined}
        style={{ flex: "1 1 auto" }}
      />
      <input
        type="submit"
        className="stdbtn"
        value={loading ? (t("pluginWizard.lookingUp") ?? "") : (t("pluginWizard.lookUp") ?? "")}
        disabled={loading || !domain.trim()}
      />
    </form>
  )
}
