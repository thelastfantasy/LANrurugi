import { useTranslation } from "react-i18next"
import { useNavigate } from "react-router-dom"

import type { QueueError } from "@/api/types"
import { routes } from "@/lib/routes"

/** Renders a structured `QueueError` as translated text, mapping `.kind` to an i18n key. A
 * `plugin_reported` error's `error_code` is itself the i18n key (see `plugin-sdk.ts`'s `PluginError`). */
export function QueueErrorText({ error }: { error: QueueError }) {
  const { t } = useTranslation()
  const navigate = useNavigate()

  switch (error.kind) {
    case "plugin_reported":
      return <span>{t(error.error_code, error.data)}</span>
    case "plugin_execution_failed":
      return <span>{t("components.layout.pluginPluginFailedToRun", { plugin: error.plugin })}</span>
    case "malformed_plugin_response":
      return <span>{t("components.layout.pluginPluginReturnedAnUnrecognized", { plugin: error.plugin })}</span>
    case "empty_plugin_result":
      return <span>{t("components.layout.pluginPluginReturnedNothingUsable", { plugin: error.plugin })}</span>
    case "invalid_url":
      return <span>{t("components.layout.invalidUrlUrl", { url: error.url })}</span>
    case "invalid_http_method":
      return <span>{t("components.layout.invalidHttpMethodMethod", { method: error.method })}</span>
    case "http_request_failed":
      return <span>{t("components.layout.requestFailedUrl", { url: error.url })}</span>
    case "http_status":
      return (
        <span>{t("components.layout.serverRespondedWithStatusStatus", { url: error.url, status: error.status })}</span>
      )
    case "write_failed":
      return <span>{t("components.layout.failedToWriteTheDownloaded")}</span>
    case "bundle_failed":
      return <span>{t("components.layout.failedToBundleDownloadedPages")}</span>
    case "duplicate_archive":
      return (
        <span>
          {t(
            error.reason === "content_hash"
              ? "A matching archive already exists"
              : "An archive with this filename already exists",
          )}{" "}
          <a
            href={routes.reader(error.existing_id)}
            onClick={(e) => {
              e.preventDefault()
              navigate(routes.reader(error.existing_id))
            }}
          >
            {error.existing_id}
          </a>
        </span>
      )
    case "duplicate_filename":
      return (
        <span>
          {t("components.layout.theFilenameFilenameIsAlready", { filename: error.filename })}{" "}
          <a
            href={routes.reader(error.existing_id)}
            onClick={(e) => {
              e.preventDefault()
              navigate(routes.reader(error.existing_id))
            }}
          >
            {error.existing_id}
          </a>
        </span>
      )
    case "duplicate_filename_cleaned":
      return <span>{t("components.layout.expiredPleaseDownloadAgain")}</span>
    case "internal":
      return <span>{t("components.layout.anInternalErrorOccurred")}</span>
    case "stale_after_restart":
      return <span>{t("components.layout.downloadWasInterruptedByA")}</span>
    case "already_patched":
      return (
        <span style={{ color: "#c79121" }}>
          {t("components.layout.alreadyPatchedTheUnique", {
            filename: error.filename,
          })}{" "}
          <a
            href={routes.reader(error.existing_id)}
            onClick={(e) => {
              e.preventDefault()
              navigate(routes.reader(error.existing_id))
            }}
            style={{ color: "inherit" }}
          >
            {error.existing_id}
          </a>
        </span>
      )
  }
}
