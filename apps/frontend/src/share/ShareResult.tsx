import { useTranslation } from "react-i18next"

import { useShareCode } from "./api"
import { copyWithToast } from "./clipboard"
import { EmbedConfigurator } from "./EmbedConfigurator"
import { qrPngBlob } from "./qr"
import { ShareQr } from "./ShareQr"
import type { ShareCodeInfo } from "./types"

function CopyRow({ label, value }: { label: string; value: string }) {
  const { t } = useTranslation()
  return (
    <div className="share-result-row">
      <span className="share-result-label">{label}</span>
      <code className="share-result-value">{value}</code>
      <button
        type="button"
        className="stdbtn"
        onClick={() => void copyWithToast(value, t("share.result.copied"), t("share.result.copyFailed"))}
      >
        {t("share.result.copy")}
      </button>
    </div>
  )
}

/** The code, link, QR code and embed snippets of a share (FR-009, FR-010a, FR-030). */
export function ShareResult({ shareId, info }: { shareId: string; info?: ShareCodeInfo }) {
  const { t } = useTranslation()
  const fetched = useShareCode(info ? null : shareId)
  const code = info ?? fetched.data
  if (!code) {
    return fetched.isError ? <p>{t("share.result.codeUnavailable")}</p> : <i className="fa fa-2x fa-cog fa-spin" aria-hidden="true"></i>
  }
  // A phone camera can open a link but not the `lrrg:` scheme, so the QR code holds the link when
  // there is one.
  const qrText = code.link ?? code.text_form
  const savePng = async () => {
    const blob = await qrPngBlob(qrText)
    const a = document.createElement("a")
    a.href = URL.createObjectURL(blob)
    a.download = `share-${code.code}.png`
    a.click()
    URL.revokeObjectURL(a.href)
  }
  return (
    <div className="share-result">
      <div className="share-import-box">
        <ShareQr text={qrText} />
        <div>
          {/* The short code to read out; Copy takes the full text form, whose sealed routes let an
              import skip the lookup. */}
          <p className="share-code-text">{code.code}</p>
          <button
            type="button"
            className="stdbtn"
            onClick={() => void copyWithToast(code.text_form, t("share.result.copied"), t("share.result.copyFailed"))}
          >
            {t("share.result.copy")}
          </button>
          <button type="button" className="stdbtn" onClick={() => void savePng()}>
            {t("share.result.savePng")}
          </button>
        </div>
      </div>
      {code.text_form.includes("?") && <p className="share-result-note">{t("share.result.copyCarriesRoute")}</p>}
      {code.link ? (
        <>
          <CopyRow label={t("share.result.link")} value={code.link} />
          <p className="share-result-note">{t("share.result.linkNote")}</p>
        </>
      ) : (
        <p className="share-result-note">{t("share.result.noLinkNote")}</p>
      )}
      {code.needs_direct_route && <p className="share-result-note">{t("share.result.needsDirectRoute")}</p>}
      {code.embed && code.link && <EmbedConfigurator origin={code.link.replace(/\/s\/[^/]*$/, "")} code={code.code} />}
      <p className="share-disclaimer">{t("share.disclaimer.share")}</p>
    </div>
  )
}
