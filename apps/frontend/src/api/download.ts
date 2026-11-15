/**
 * Sends the browser to an archive's bytes.
 *
 * The server decides where that is: when the site runs behind a CDN proxy and a direct host is
 * configured, the link it hands back points at that host and carries its own signature (a cookie
 * would not follow a link to another origin). Without one, the plain same-origin route is used.
 */
export async function startDownload(path: string): Promise<void> {
  try {
    const response = await fetch(`${path}/download-link`, { credentials: "same-origin" })
    if (response.ok) {
      const body = (await response.json()) as { url?: string }
      if (body.url) {
        window.location.assign(body.url)
        return
      }
    }
  } catch {
    // Fall through to the plain route: a failed lookup must not leave the click doing nothing.
  }
  window.location.assign(`${path}/download`)
}
