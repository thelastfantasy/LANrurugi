import { useTranslation } from "react-i18next"

import type { CandidateRecord } from "@/api/types"
import { Tooltip } from "@/components/common-ui/Display"

import { preferredTitle, sourceHref, titleHref } from "./preferredTitle"

/** A candidate's title, linked where it is actually useful.
 *
 * The library's own copy when it has one (see `titleHref`), the source page otherwise. When those
 * differ, the source page is still one click away behind the small icon, so "this is already in your
 * library" never turns into "and now you cannot get back to where it came from".
 *
 * Shared by the subscription preview and both history modals — the three places a candidate's title
 * is shown — so the rule cannot drift between them. */
export function CandidateTitleLink({
  record,
  order,
  zIndex = 9700,
}: {
  record: CandidateRecord
  /** Ordered language codes, most preferred first. */
  order: readonly string[]
  /** Above `Modal`'s own 9001 by default: all three call sites are inside a dialog. */
  zIndex?: number
}) {
  const { t } = useTranslation()
  const local = titleHref(record) !== sourceHref(record.source_url)

  return (
    <>
      <a href={titleHref(record)} target="_blank" rel="noreferrer">
        {preferredTitle(record.title, order) ?? record.source_url}
      </a>
      {local && (
        <Tooltip label={t("subscriptions.openSource") ?? ""} zIndex={zIndex}>
          <a
            href={sourceHref(record.source_url)}
            target="_blank"
            rel="noreferrer"
            className="preview-source-link"
            aria-label={t("subscriptions.openSource") ?? "Open the source page"}
          >
            <i className="fa fa-external-link" aria-hidden="true"></i>
          </a>
        </Tooltip>
      )}
    </>
  )
}
