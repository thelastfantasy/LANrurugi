import { type ReactNode, useState } from "react"
import { useTranslation } from "react-i18next"
import { Link } from "react-router-dom"

import { Button } from "@/components/common-ui/Form"
import { FormatBadge, StatusIcon } from "@/components/Display"
import { isBookExtension } from "@/lib/utils/bookFormats"

import { ShareTags } from "./ShareTags"
import type { ShareTransport } from "./transport"
import type { ArchiveEntry, SharePermissions } from "./types"

/** One shared archive or tankoubon, in the library's own card markup so the theme styles it. */
export function ShareItemCard({
  transport,
  title,
  cover,
  tags,
  subtitle,
  permissions,
  canDownload,
  readHref,
  downloadUrl,
  onDownload,
  members,
}: {
  transport: ShareTransport
  title: string
  cover: ArchiveEntry | null
  tags: string
  subtitle: string
  permissions: SharePermissions
  canDownload: boolean
  /** Where reading this archive starts. A real link, so middle-click and "open in new tab" work. */
  readHref?: (entry: ArchiveEntry) => string
  downloadUrl?: string
  /** Called once a download starts: it may use up the share's last claim. */
  onDownload?: () => void
  members?: ArchiveEntry[]
}) {
  const { t } = useTranslation()
  const [failed, setFailed] = useState(false)
  const thumb = cover && !failed ? transport.thumbnailUrl(cover.archive_id) : "/legacy/img/noThumb.png"
  const href = permissions.read && cover && readHref ? readHref(cover) : undefined
  const extension = members ? undefined : cover?.filename?.split(".").pop()?.toLowerCase()
  // Like a library card: the title and the cover open the reader when reading is allowed.
  const readable = (content: ReactNode, to = href) =>
    to ? (
      <Link to={to} title={title}>
        {content}
      </Link>
    ) : (
      content
    )

  return (
    <div className="id1 share-item-card">
      {/* Same title row as a library card: e-book format or tankoubon mark after the title. */}
      <div className="id2" style={{ display: "flex", alignItems: "baseline", justifyContent: "center", gap: 2 }}>
        {readable(<span title={title}>{title}</span>)}
        {(members || isBookExtension(extension)) && (
          <div className="status-icons" style={{ flexShrink: 0, whiteSpace: "nowrap" }}>
            {members ? (
              <StatusIcon kind="tankoubon" title={t("library.tankoubon") ?? undefined} style={{ marginLeft: 2 }} />
            ) : (
              <FormatBadge extension={extension} style={{ marginLeft: 2 }} />
            )}
          </div>
        )}
      </div>
      <div className="id3">
        {readable(<img src={thumb} alt={title} onError={() => setFailed(true)} referrerPolicy="no-referrer" />)}
      </div>
      <div className="id4">
        <div>{subtitle}</div>
        <ShareTags tags={tags} />
      </div>
      {/* Reading opens from the title or cover, as in the library; only Download needs a button. */}
      {permissions.download && canDownload && downloadUrl && (
        <div className="share-card-actions">
          <Button
            onClick={() => {
              // A real button (an `<a class="stdbtn">` neither centres its text nor sizes like one);
              // the file comes from a throwaway link, which keeps the browser's download handling.
              const link = document.createElement("a")
              link.href = downloadUrl
              link.download = ""
              link.rel = "noreferrer"
              link.click()
              onDownload?.()
            }}
          >
            {t("share.action.download")}
          </Button>
        </div>
      )}
      {members && members.length > 1 && permissions.read && readHref && (
        <div className="share-tank-members">
          {members.map((m, i) => (
            <Link key={m.archive_id} className="favtag-btn" title={m.title} to={readHref(m)}>
              {i + 1}
            </Link>
          ))}
        </div>
      )}
    </div>
  )
}
