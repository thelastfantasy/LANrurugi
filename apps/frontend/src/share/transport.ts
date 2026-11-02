/**
 * How the recipient page talks to a share. Only the HTTP transport exists — a share is always
 * served by the sharer's own instance (FR-047) — but the reader and page code depend on this
 * interface rather than on URLs, so they never see the token handling.
 */

import { formatShareCode } from "./code"
import type { RecipientView } from "./types"

export class ShareRequestError extends Error {
  readonly status: number
  /** `not_found`, `permission_denied`, `exhausted`, `rate_limited`, … */
  readonly code: string

  constructor(status: number, code: string) {
    super(code)
    this.status = status
    this.code = code
  }
}

export interface ShareTransport {
  open(code: string): Promise<RecipientView>
  manifest(): Promise<RecipientView>
  thumbnailUrl(archiveId: string, page?: number): string
  pages(archiveId: string): Promise<string[]>
  downloadUrl(archiveId: string): string
  tankoubonDownloadUrl(tankId: string): string
}

async function readError(response: Response): Promise<ShareRequestError> {
  const body = (await response.json().catch(() => null)) as { error?: string } | null
  return new ShareRequestError(response.status, body?.error ?? "not_found")
}

/** Keeps the token in memory only; media URLs carry it as `?rt=` since `<img>` cannot send headers. */
export class HttpTransport implements ShareTransport {
  private token: string | null = null

  private withToken(path: string): string {
    const separator = path.includes("?") ? "&" : "?"
    return `${path}${separator}rt=${encodeURIComponent(this.token ?? "")}`
  }

  async open(code: string): Promise<RecipientView> {
    const response = await fetch("/api/share/r/open", {
      method: "POST",
      headers: { "Content-Type": "application/json" },
      body: JSON.stringify({ code: formatShareCode(code) }),
      credentials: "omit",
    })
    if (!response.ok) throw await readError(response)
    const body = (await response.json()) as RecipientView & { token: string }
    this.token = body.token
    return body
  }

  async manifest(): Promise<RecipientView> {
    const response = await fetch(this.withToken("/api/share/r/manifest"), { credentials: "omit" })
    if (!response.ok) throw await readError(response)
    return (await response.json()) as RecipientView
  }

  thumbnailUrl(archiveId: string, page?: number): string {
    const base = `/api/share/r/archives/${encodeURIComponent(archiveId)}/thumbnail`
    return this.withToken(page ? `${base}?page=${page}` : base)
  }

  async pages(archiveId: string): Promise<string[]> {
    const response = await fetch(this.withToken(`/api/share/r/archives/${encodeURIComponent(archiveId)}/pages`), {
      credentials: "omit",
    })
    if (!response.ok) throw await readError(response)
    const body = (await response.json()) as { pages: { url: string }[] }
    return body.pages.map((p) => this.withToken(p.url))
  }

  downloadUrl(archiveId: string): string {
    return this.withToken(`/api/share/r/archives/${encodeURIComponent(archiveId)}/download`)
  }

  tankoubonDownloadUrl(tankId: string): string {
    return this.withToken(`/api/share/r/tankoubons/${encodeURIComponent(tankId)}/download`)
  }
}
