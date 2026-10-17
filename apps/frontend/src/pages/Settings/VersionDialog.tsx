import { useTranslation } from "react-i18next"

import { useServerInfo, useVersionDetails } from "@/api/hooks"
import { Modal } from "@/components/common-ui/Display"
import { Button } from "@/components/common-ui/Form"

/**
 * "LANrurugi version" dialog, opened from the version line on the Settings page — Zipline's own
 * `VersionDialog` is the model: what this instance runs, whether a newer release exists, and a way
 * to read that release's notes.
 *
 * Every state the backend can report has to read differently, because the two interesting ones look
 * alike from the outside: "up to date", "the update check is disabled" (`enabled: false` — the
 * `--disable-update-check` deployment flag) and "GitHub could not be reached" all mean *no newer
 * release is advertised*, but only the first is a statement about this build. Claiming "up to date"
 * for the other two would be a lie the operator cannot see through.
 */
export function VersionDialog({ onClose }: { onClose: () => void }) {
  const { t } = useTranslation()
  const info = useServerInfo()
  const check = useVersionDetails()

  const payload = check.data
  const latest = payload?.data.latest ?? null
  const outdated = Boolean(payload?.enabled && latest && !payload.data.isLatest)
  // The running release's own page: when there is nothing newer to read, that is still the release
  // whose notes belong to this build.
  const releaseUrl = outdated ? latest?.url : payload?.data.version?.url
  const version = info.data?.version ?? ""
  const commit = info.data?.version_name ?? payload?.details.sha?.slice(0, 7) ?? ""

  const row = (label: string, value: string) => (
    <div
      key={label}
      style={{
        display: "flex",
        justifyContent: "space-between",
        gap: 16,
        padding: "6px 0",
      }}
    >
      <strong>{label}</strong>
      <span>{value}</span>
    </div>
  )

  return (
    <Modal onClose={onClose} textAlign="left" width={560}>
      <h2 style={{ marginTop: 0 }}>{t("settings.versionDialog.title")}</h2>

      {outdated ? (
        <p>
          {t("settings.versionDialog.outdatedNotice")}{" "}
          {latest ? (
            <a href={encodeURI(latest.url)} target="_blank" rel="noreferrer">
              {t("settings.versionDialog.latestLink")}
            </a>
          ) : null}
        </p>
      ) : null}

      <h3 style={{ marginBottom: 4 }}>{t("settings.versionDialog.currentVersion")}</h3>
      {/* `width`/`display`/`margin` are all overrides on legacy's global `div.id1`, which is the
          library's 228px-wide card column: without them this panel inherits that fixed width (its
          `space-between` rows then only spread across 228px, leaving the modal's right half empty),
          and `inline-block` + its own margins make the panel's position depend on the parent's
          `text-align`. The card colours and border are what this actually wants from that class.

          `width: auto`, deliberately not `100%`: legacy is written against the browser default
          `box-sizing: content-box`, so a percentage resolves against the parent's *content* width
          and this panel's own padding + border are then added on top — `100%` overflowed by exactly
          34px (2×16 + 2×1) and put a horizontal scrollbar on the modal. `auto` makes a block box
          fill the space that is actually left. */}
      <div
        className="id1"
        style={{ width: "auto", display: "block", margin: "0 0 16px", padding: "4px 16px" }}
      >
        {row(t("settings.versionDialog.versionLabel"), version)}
        {row(t("settings.versionDialog.commitLabel"), commit)}
        {payload ? row(t("settings.versionDialog.upstreamLabel"), payload.data.isUpstream ? t("settings.versionDialog.yes") : t("settings.versionDialog.no")) : null}
      </div>

      {outdated && latest ? (
        <>
          <h3 style={{ marginBottom: 8 }}>
            {t("settings.versionDialog.latestAvailable", { version: latest.tag })}
          </h3>
          {releaseUrl ? (
            <a
              className="stdbtn"
              href={encodeURI(releaseUrl)}
              target="_blank"
              rel="noreferrer"
              style={{ display: "block", textAlign: "center", textDecoration: "none" }}
            >
              {t("settings.versionDialog.changelogs", { version: latest.tag })}
            </a>
          ) : null}
        </>
      ) : (
        <>
          <p>
            {payload?.enabled === false
              ? t("settings.versionDialog.checkDisabled")
              : check.isError || !payload
                ? t("settings.versionDialog.checkFailed")
                : t("settings.versionDialog.upToDate")}
          </p>
          {releaseUrl ? (
            <a
              className="stdbtn"
              href={encodeURI(releaseUrl)}
              target="_blank"
              rel="noreferrer"
              style={{ display: "block", textAlign: "center", textDecoration: "none" }}
            >
              {t("settings.versionDialog.changelogs", { version: payload?.data.version?.tag ?? version })}
            </a>
          ) : null}
        </>
      )}

      <div style={{ marginTop: 16, textAlign: "right" }}>
        <Button onClick={onClose}>{t("settings.versionDialog.close")}</Button>
      </div>
    </Modal>
  )
}
