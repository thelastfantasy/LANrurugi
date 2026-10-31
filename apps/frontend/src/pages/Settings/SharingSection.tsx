import "@/share/share.css"

import { type MouseEvent, useState } from "react"
import { useTranslation } from "react-i18next"

import { Modal, Tooltip } from "@/components/common-ui/Display"
import { CollapsibleSection, DateTimeStack } from "@/components/Display"
import { useShares } from "@/share/api"
import { copyWithToast } from "@/share/clipboard"
import { type SettingField, ShareSettingsForm } from "@/share/ShareSettingsForm"
import type { SharePermissions } from "@/share/types"

import { ShareDetailPanel } from "./ShareDetailPanel"
import { SharingHelping } from "./SharingHelping"
import { AltScheduleEditor, SharingEndpointStatus, SharingEndpointSummary, SharingNodeString } from "./SharingStatus"

const DEFAULT_FIELDS: SettingField[] = [
  {
    key: "share_default_permissions",
    kind: "text",
    label: "share.settings.defaultPermissions",
    hint: true,
  },
  {
    key: "share_default_download_limit",
    kind: "number",
    label: "share.form.downloadLimit",
  },
  { key: "share_default_ip_allow", kind: "lines", label: "share.form.ipAllow" },
  {
    key: "share_default_ip_first_n",
    kind: "number",
    label: "share.form.firstN",
  },
  {
    key: "share_default_expiry_days",
    kind: "number",
    label: "share.form.expiryDays",
  },
  { key: "share_default_embed", kind: "bool", label: "share.form.embed" },
  {
    key: "share_default_allow_helpers",
    kind: "bool",
    label: "share.form.allowHelpers",
  },
]

const LIMIT_FIELDS: SettingField[] = [
  {
    key: "share_upload_limit",
    kind: "number",
    label: "share.limits.ownRate",
    hint: true,
  },
  { key: "share_max_peers", kind: "number", label: "share.limits.ownPeers" },
  {
    key: "share_helper_upload_limit",
    kind: "number",
    label: "share.limits.helpRate",
  },
  {
    key: "share_helper_max_peers",
    kind: "number",
    label: "share.limits.helpPeers",
  },
  {
    key: "share_alt_upload_limit",
    kind: "number",
    label: "share.limits.altOwnRate",
    hint: true,
  },
  {
    key: "share_alt_max_peers",
    kind: "number",
    label: "share.limits.altOwnPeers",
  },
  {
    key: "share_alt_helper_upload_limit",
    kind: "number",
    label: "share.limits.altHelpRate",
  },
  {
    key: "share_alt_helper_max_peers",
    kind: "number",
    label: "share.limits.altHelpPeers",
  },
  { key: "share_alt_manual", kind: "bool", label: "share.limits.manualSwitch" },
  {
    key: "share_import_concurrency",
    kind: "number",
    label: "share.limits.importConcurrency",
  },
]

const NODE_FIELDS: SettingField[] = [
  {
    key: "share_nodes",
    kind: "lines",
    label: "share.nodes.list",
    hint: true,
    placeholder: "lrrg-node:…@host:443",
  },
  {
    key: "share_node_rendezvous",
    kind: "bool",
    label: "share.nodes.offerRendezvous",
  },
  {
    key: "share_node_relay",
    kind: "bool",
    label: "share.nodes.offerRelay",
    hint: true,
  },
  {
    key: "share_node_relay_tls",
    kind: "choice",
    label: "share.nodes.relayTls",
    options: ["acme", "files", "proxy"],
  },
  {
    key: "share_node_relay_port",
    kind: "number",
    label: "share.nodes.relayPort",
    hint: true,
  },
  {
    key: "share_node_relay_cert",
    kind: "text",
    label: "share.nodes.relayCert",
  },
  { key: "share_node_relay_key", kind: "text", label: "share.nodes.relayKey" },
  {
    key: "share_node_relay_rate",
    kind: "number",
    label: "share.nodes.relayRate",
    hint: true,
  },
  {
    key: "share_node_relay_allow",
    kind: "lines",
    label: "share.nodes.relayAllow",
    hint: true,
    placeholder: "lrrg-node:…@host:443",
  },
  {
    key: "share_self_relay_url",
    kind: "text",
    label: "share.nodes.selfRelayUrl",
    hint: true,
    placeholder: "https://relay.example.org",
  },
]

const NETWORK_FIELDS: SettingField[] = [
  {
    key: "share_external_addrs",
    kind: "lines",
    label: "share.network.externalAddrs",
    hint: true,
    placeholder: "203.0.113.7:443",
  },
  {
    key: "share_portmap",
    kind: "bool",
    label: "share.network.portmap",
    hint: true,
  },
]

const CONNECTIVITY_FIELDS: SettingField[] = [
  {
    key: "share_public_url",
    kind: "text",
    label: "share.settings.publicUrl",
    hint: true,
    placeholder: "https://library.example.org",
  },
  {
    key: "share_public_services",
    kind: "bool",
    label: "share.settings.publicServices",
    hint: true,
  },
  {
    key: "share_udp_port",
    kind: "number",
    label: "share.settings.udpPort",
    hint: true,
  },
  { key: "share_relay_urls", kind: "lines", label: "share.settings.relayUrls" },
  {
    key: "share_trusted_proxies",
    kind: "lines",
    label: "share.settings.trustedProxies",
    hint: true,
  },
  {
    key: "share_pkarr_relays",
    kind: "lines",
    label: "share.settings.pkarrRelays",
  },
]

const PERMISSIONS = [
  { key: "read", letter: "R" },
  { key: "import", letter: "I" },
  { key: "download", letter: "D" },
] as const

/** R / I / D chips: the letter is the permission, the colour sets it apart, the tooltip names it. */
function PermissionChips({ permissions }: { permissions: SharePermissions }) {
  const { t } = useTranslation()
  return (
    <span className="share-perms">
      {PERMISSIONS.map(({ key, letter }) => (
        <Tooltip key={key} label={t(`share.permission.${key}`)}>
          <span
            className={`share-perm share-perm-${key}${permissions[key] ? "" : " share-perm-off"}`}
            aria-label={t(`share.permission.${key}`)}
          >
            {letter}
          </span>
        </Tooltip>
      ))}
    </span>
  )
}

const STATUS_ICONS: Record<string, string> = {
  active: "fa-circle-check",
  exhausted: "fa-gauge-high",
  expired: "fa-hourglass-end",
  revoked: "fa-ban",
}

/** The state badge: same pill as before, with an icon so the three non-active states read at a
 *  glance instead of only by their (long) localized text. */
function StatusBadge({ status }: { status: string }) {
  const { t } = useTranslation()
  return (
    <Tooltip label={t(`share.status.${status}`)}>
      <span className={`share-badge share-status-${status}`}>
        <i className={`fa ${STATUS_ICONS[status] ?? "fa-circle"}`} aria-hidden="true" />
        {t(`share.statusShort.${status}`, { defaultValue: t(`share.status.${status}`) })}
      </span>
    </Tooltip>
  )
}

/** A column header with an explanation behind its own info icon (the header text stays short). */
function HeaderWithHint({ label, hint }: { label: string; hint: string }) {
  return (
    <>
      {label}
      <Tooltip label={hint}>
        <i className="fa fa-info-circle share-th-hint" aria-label={hint} />
      </Tooltip>
    </>
  )
}

/** The code, plus the two things one wants from a code in a list: copy it in the form another
 *  instance accepts (`lrrg:…`, not the bare letters), and open its share page. */
function ShareCodeCell({ code }: { code: string }) {
  const { t } = useTranslation()
  const textForm = `lrrg:${code}`
  const copy = (e: MouseEvent) => {
    e.stopPropagation()
    void copyWithToast(textForm, t("share.settings.codeCopied"), t("share.settings.copyFailed"))
  }
  return (
    <span className="share-code-cell">
      <Tooltip label={t("share.settings.copyCode")}>
        <button type="button" className="share-code" onClick={copy}>
          {code}
        </button>
      </Tooltip>
      <Tooltip label={t("share.settings.copyCode")}>
        <button
          type="button"
          className="share-copy-code"
          aria-label={t("share.settings.copyCode")}
          onClick={copy}
        >
          <i className="fa fa-copy" aria-hidden="true" />
        </button>
      </Tooltip>
      <Tooltip label={t("share.settings.openSharePage")}>
        <a
          className="share-open-page"
          href={`/s/${encodeURIComponent(code)}`}
          target="_blank"
          rel="noreferrer"
          aria-label={t("share.settings.openSharePage")}
          onClick={(e) => e.stopPropagation()}
        >
          <i className="fa fa-external-link" aria-hidden="true" />
        </a>
      </Tooltip>
    </span>
  )
}

type Panel = "defaults" | "connectivity" | "network" | "nodes" | "limits"

const PANELS: {
  id: Panel
  icon: string
  title: string
  description?: string
}[] = [
  {
    id: "defaults",
    icon: "fa-sliders",
    title: "share.settings.defaults",
    description: "share.panel.defaultsDesc",
  },
  {
    id: "connectivity",
    icon: "fa-tower-broadcast",
    title: "share.settings.connectivity",
  },
  {
    id: "network",
    icon: "fa-network-wired",
    title: "share.network.heading",
    description: "share.panel.networkDesc",
  },
  {
    id: "nodes",
    icon: "fa-server",
    title: "share.nodes.heading",
    description: "share.panel.nodesDesc",
  },
  {
    id: "limits",
    icon: "fa-gauge",
    title: "share.limits.heading",
    description: "share.panel.limitsDesc",
  },
]

function PanelBody({ panel }: { panel: Panel }) {
  const { t } = useTranslation()
  switch (panel) {
    case "defaults":
      return <ShareSettingsForm fields={DEFAULT_FIELDS} />
    case "connectivity":
      return (
        <>
          <h3>{t("share.panel.currentState")}</h3>
          <SharingEndpointStatus />
          <h3>{t("share.detail.settings")}</h3>
          <ShareSettingsForm fields={CONNECTIVITY_FIELDS} />
        </>
      )
    case "network":
      return <ShareSettingsForm fields={NETWORK_FIELDS} />
    case "nodes":
      return (
        <>
          <h3>{t("share.panel.ownNode")}</h3>
          <SharingNodeString />
          <h3>{t("share.detail.settings")}</h3>
          <ShareSettingsForm fields={NODE_FIELDS} />
        </>
      )
    case "limits":
      return (
        <>
          <ShareSettingsForm fields={LIMIT_FIELDS} />
          <h3>{t("share.limits.schedule")}</h3>
          <AltScheduleEditor />
        </>
      )
  }
}

/** Settings → Sharing: every share, plus the instance-wide sharing settings (017 US2, FR-005,
 * FR-006, FR-006a). Each settings group and each share's detail open in a modal. */
export function SharingSection() {
  const { t } = useTranslation()
  const shares = useShares("all")
  const [openShareId, setOpenShareId] = useState<string | null>(null)
  const [panel, setPanel] = useState<Panel | null>(null)
  const openPanel = PANELS.find((p) => p.id === panel)
  // Looked up from the list so the badge follows a revoke made inside the modal.
  const openShare = shares.data?.find((s) => s.id === openShareId)

  return (
    <CollapsibleSection id="sharing" icon="fa-share-nodes" title={t("share.settings.section")}>
      <div style={{ padding: "0 12px" }}>
        <h3>{t("share.settings.shares")}</h3>
        {shares.data && shares.data.length === 0 && <p>{t("share.settings.noShares")}</p>}
        {shares.data && shares.data.length > 0 && (
          <div className="share-list-scroll">
            <table className="itg share-list">
              <colgroup>
                <col className="share-list-col-title" />
                <col className="share-list-col-code" />
                <col className="share-list-col-status" />
                <col className="share-list-col-perms" />
                <col className="share-list-col-used" />
                <col className="share-list-col-items" />
                <col className="share-list-col-date" />
                <col className="share-list-col-date" />
              </colgroup>
              <thead>
                <tr>
                  <th>{t("share.form.title")}</th>
                  <th>{t("share.settings.codeColumn")}</th>
                  <th>
                    <HeaderWithHint label={t("share.settings.status")} hint={t("share.settings.statusHint")} />
                  </th>
                  <th>
                    <HeaderWithHint
                      label={t("share.form.permissions")}
                      hint={t("share.settings.permissionsHint")}
                    />
                  </th>
                  <th className="share-list-number">{t("share.settings.used")}</th>
                  <th className="share-list-number">
                    <HeaderWithHint label={t("share.settings.itemsColumn")} hint={t("share.settings.itemsHint")} />
                  </th>
                  <th>{t("share.settings.created")}</th>
                  <th>{t("share.settings.expires")}</th>
                </tr>
              </thead>
              <tbody>
                {shares.data.map((s) => (
                  <tr key={s.id} style={{ cursor: "pointer" }} onClick={() => setOpenShareId(s.id)}>
                    <td className="share-list-title" title={s.title || s.id}>
                      {s.title || s.id}
                    </td>
                    <td className="share-list-code">
                      {s.share_code ? <ShareCodeCell code={s.share_code} /> : "—"}
                    </td>
                    <td>
                      <StatusBadge status={s.status} />
                    </td>
                    <td>
                      <PermissionChips permissions={s.permissions} />
                    </td>
                    <td className="share-list-number">
                      {s.downloads_used} / {s.download_limit ?? "∞"}
                    </td>
                    <td className="share-list-number">{s.item_count}</td>
                    <td className="share-list-date">
                      <DateTimeStack epochSeconds={s.created_at} />
                    </td>
                    <td className="share-list-date">
                      {s.expires_at ? <DateTimeStack epochSeconds={s.expires_at} /> : "—"}
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}

        <h3>{t("share.helping.heading")}</h3>
        <SharingHelping />

        <h3>{t("share.detail.settings")}</h3>
        <div className="share-panels">
          {PANELS.map((p) => (
            <div key={p.id} className="share-panel-row">
              <i className={`fa ${p.icon} fa-fw`} aria-hidden="true"></i>
              <div className="share-panel-text">
                <div className="share-panel-title">{t(p.title)}</div>
                <div className="share-panel-desc">
                  {p.id === "connectivity" ? <SharingEndpointSummary /> : t(p.description ?? "")}
                </div>
              </div>
              <button type="button" className="stdbtn" onClick={() => setPanel(p.id)}>
                {t("share.panel.edit")}
              </button>
            </div>
          ))}
        </div>
      </div>

      {openPanel && (
        <Modal onClose={() => setPanel(null)} width={720} textAlign="left">
          <h2 className="share-modal-title">{t(openPanel.title)}</h2>
          <PanelBody panel={openPanel.id} />
        </Modal>
      )}
      {openShare && (
        <Modal onClose={() => setOpenShareId(null)} width={760} textAlign="left">
          <h2 className="share-modal-title">
            {openShare.title || openShare.id}{" "}
            <span className={`share-badge share-status-${openShare.status}`}>{t(`share.status.${openShare.status}`)}</span>
          </h2>
          <ShareDetailPanel shareId={openShare.id} />
        </Modal>
      )}
    </CollapsibleSection>
  )
}
