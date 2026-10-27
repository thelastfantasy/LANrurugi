import "./share.css"

import { useEffect, useMemo, useState } from "react"
import { useTranslation } from "react-i18next"
import { Navigate, useNavigate, useParams, useSearchParams } from "react-router-dom"

import { Button } from "@/components/common-ui/Form"

import { copyWithToast } from "./clipboard"
import { formatShareCode, normalizeShareCode } from "./code"
import { ShareItemCard } from "./ShareItemCard"
import { ShareQr } from "./ShareQr"
import { ShareReader } from "./ShareReader"
import { HttpTransport, ShareRequestError } from "./transport"
import type { ArchiveEntry, RecipientView } from "./types"

/** Keeps the page's address — which holds the code — out of every outgoing request (FR-013b), and
 * out of search engines: the address is a credential. */
function useNoReferrerMeta() {
  useEffect(() => {
    const referrer = document.createElement("meta")
    referrer.name = "referrer"
    referrer.content = "no-referrer"
    const robots = document.createElement("meta")
    robots.name = "robots"
    robots.content = "noindex"
    document.head.append(referrer, robots)
    return () => {
      referrer.remove()
      robots.remove()
    }
  }, [])
}

/** What a reader address names: a shared archive, or a tankoubon volume (1-based) and its siblings. */
function readingTarget(
  view: RecipientView,
  archiveId: string | undefined,
  tankId: string | undefined,
  volume: string | undefined,
): { entry: ArchiveEntry; tank?: { id: string; members: ArchiveEntry[]; current: number } } | null {
  for (const item of view.manifest.items) {
    if (archiveId && "Archive" in item && item.Archive.archive_id === archiveId) return { entry: item.Archive }
    if (tankId && "Tankoubon" in item && item.Tankoubon.id === tankId) {
      const members = item.Tankoubon.members
      const current = Math.min(Math.max(Number(volume) || 1, 1), members.length)
      const entry = members[current - 1]
      return entry ? { entry, tank: { id: tankId, members, current } } : null
    }
  }
  return null
}

/** `/s/:code` — a share as its recipient sees it, outside the library's own shell (017 US1). Its
 * reader lives at `/s/:code/a/:archiveId` and `/s/:code/t/:tankId/:volume`, with `?p=` the page. */
export function SharePage() {
  useNoReferrerMeta()
  const { t } = useTranslation()
  const { code: rawCode = "", archiveId, tankId, volume } = useParams()
  const [search, setSearch] = useSearchParams()
  const navigate = useNavigate()
  const code = normalizeShareCode(rawCode)
  const transport = useMemo(() => new HttpTransport(), [])
  const [view, setView] = useState<RecipientView | null>(null)
  const [failure, setFailure] = useState<{ status: number; code: string } | null>(
    code ? null : { status: 404, code: "not_found" },
  )
  const base = `/s/${encodeURIComponent(rawCode)}`
  // Reading targets are plain addresses: the item cards render them as real links, so a
  // middle-click or "open in new tab" works like anywhere else.
  const archiveHref = (entry: ArchiveEntry) => `${base}/a/${encodeURIComponent(entry.archive_id)}`
  const volumeHref = (id: string, n: number) => `${base}/t/${encodeURIComponent(id)}/${n}`
  const readVolume = (id: string, n: number) => navigate(volumeHref(id, n))

  useEffect(() => {
    if (!code) return
    transport
      .open(code)
      .then(setView)
      .catch((e: unknown) =>
        setFailure(e instanceof ShareRequestError ? { status: e.status, code: e.code } : { status: 404, code: "not_found" }),
      )
  }, [code, transport])

  // A download can use up the share's last claim; refresh the state shortly after one starts.
  const refresh = () => {
    window.setTimeout(() => {
      transport
        .manifest()
        .then(setView)
        .catch((e: unknown) => {
          // A real refusal closes the page; any other failure keeps the last state showing.
          if (e instanceof ShareRequestError) setFailure({ status: e.status, code: e.code })
        })
    }, 1500)
  }

  if (failure) {
    // A closed share says why (the server tells a right code's recipient); anything else is the
    // generic "not available", which also covers a wrong code. A used-up share is not closed: it
    // keeps browsing and reading, and only refuses downloads (see the notice below).
    const closed = ["expired", "revoked"].includes(failure.code) ? failure.code : null
    return (
      <div className="ido share-closed">
        <p className="share-closed-status">{failure.status}</p>
        <h1 className="ih">{closed ? t(`share.closed.${closed}.title`) : t("share.notAvailable.title")}</h1>
        <p>
          {closed
            ? t(`share.closed.${closed}.body`)
            : failure.code === "rate_limited"
              ? t("share.refusal.rate_limited")
              : t("share.notAvailable.body")}
        </p>
      </div>
    )
  }
  if (!view || !code) {
    return (
      <div className="ido" style={{ textAlign: "center", padding: 40 }}>
        <i className="fa fa-3x fa-cog fa-spin" aria-hidden="true"></i>
      </div>
    )
  }
  if (archiveId || tankId) {
    const target = view.permissions.read ? readingTarget(view, archiveId, tankId, volume) : null
    if (!target) return <Navigate to={base} replace />
    const tank = target.tank
    return (
      <ShareReader
        // A new volume or archive starts from its own first page.
        key={target.entry.archive_id}
        transport={transport}
        entry={target.entry}
        onClose={() => navigate(base)}
        initialPage={Math.max(Number(search.get("p")) || 1, 1) - 1}
        onPageChange={(i) => setSearch(i > 0 ? { p: String(i + 1) } : {}, { replace: true })}
        volume={tank && { current: tank.current, total: tank.members.length, go: (n) => readVolume(tank.id, n) }}
      />
    )
  }

  const { manifest, permissions, status } = view
  // Browsing and reading stay; a used-up share only stops offering downloads.
  const canDownload = status === "active"
  return (
    <div className="ido">
      <div className="share-page-header">
        <h1 className="ih">{manifest.title}</h1>
        <span>{t("share.page.items", { count: manifest.items.length })}</span>
      </div>
      {status === "exhausted" && (
        <p className="share-notice share-status-exhausted">{t("share.page.exhaustedNotice")}</p>
      )}
      <div id="thumbs_container" className="library-thumb-grid">
        {manifest.items.map((item, i) => {
          if ("Archive" in item) {
            const a = item.Archive
            return (
              <ShareItemCard
                key={a.archive_id}
                transport={transport}
                title={a.title}
                cover={a}
                tags={a.tags}
                subtitle={t("share.page.pages", { count: a.pagecount })}
                permissions={permissions}
                canDownload={canDownload}
                readHref={archiveHref}
                downloadUrl={transport.downloadUrl(a.archive_id)}
                onDownload={refresh}
              />
            )
          }
          if ("Tankoubon" in item) {
            const tank = item.Tankoubon
            return (
              <ShareItemCard
                key={tank.id}
                transport={transport}
                title={tank.name}
                cover={tank.members[0] ?? null}
                tags={tank.tags}
                subtitle={t("share.page.volumes", { count: tank.members.length })}
                permissions={permissions}
                canDownload={canDownload}
                readHref={(m) =>
                  volumeHref(tank.id, Math.max(tank.members.findIndex((x) => x.archive_id === m.archive_id), 0) + 1)
                }
                downloadUrl={transport.tankoubonDownloadUrl(tank.id)}
                onDownload={refresh}
                members={tank.members}
              />
            )
          }
          return (
            <div key={`unavailable-${i}`} className="id1">
              <div className="id2">{item.Unavailable.title ?? t("share.page.unavailableItem")}</div>
              <div className="id3">
                <img src="/legacy/img/noThumb.png" alt="" />
              </div>
              <div className="id4">{t("share.page.unavailableItem")}</div>
            </div>
          )
        })}
      </div>
      {permissions.import && (
        <div className="share-import-box">
          <ShareQr text={`lrrg:${formatShareCode(code)}`} />
          <div>
            <h2>{t("share.page.howToImport")}</h2>
            <p>{t("share.page.howToImportBody")}</p>
            <div className="share-code-line">
              <p className="share-code-text">lrrg:{formatShareCode(code)}</p>
              <Button
                className="share-code-copy"
                onClick={() =>
                  void copyWithToast(`lrrg:${formatShareCode(code)}`, t("share.result.copied"), t("share.result.copyFailed"))
                }
              >
                {t("share.result.copy")}
              </Button>
            </div>
          </div>
        </div>
      )}
    </div>
  )
}
