/** Loopback/private/link-local ranges (IPv4 + IPv6) — a geolocation lookup for any of these
 *  resolves to nothing meaningful (there's no real-world location for "this machine" or "someone
 *  on the LAN"), so `IpGeoLink` skips linking for these and just renders plain text instead of a
 *  dead-end link. Deliberately loose/best-effort (string prefix checks, not a real CIDR parser) —
 *  this is a display convenience, not a security boundary; a value that slips through and gets
 *  linked anyway just means one extra click to a lookup that says "private range," not a defect
 *  that matters. */
function isPrivateOrLocalIp(ip: string): boolean {
  if (ip === "127.0.0.1" || ip === "::1" || ip === "0.0.0.0") return true
  if (/^10\./.test(ip)) return true
  if (/^192\.168\./.test(ip)) return true
  if (/^172\.(1[6-9]|2\d|3[01])\./.test(ip)) return true
  if (/^169\.254\./.test(ip)) return true // link-local (IPv4)
  if (/^f[cd][0-9a-f]{2}:/i.test(ip)) return true // fc00::/7, unique local (IPv6)
  if (/^fe80:/i.test(ip)) return true // link-local (IPv6)
  return false
}

/** Links a `last_used_ip`/`client_ip`-shaped value out to an IP-geolocation lookup — skipped for a
 *  private/local address (see `isPrivateOrLocalIp`), rendered as plain text instead since there's
 *  nothing a geo lookup could tell you about one. The scheme/host are always the fixed
 *  `https://ipinfo.io/` literal below — `ip` is only ever interpolated as an
 *  `encodeURIComponent`-escaped *path segment*, never used as the href's own scheme/host — because
 *  the source of this value is typically `X-Forwarded-For` (spoofable, display-only, never a
 *  security control). Building `href={ip}` directly would let whoever controls that header hand an
 *  admin viewing this table a `javascript:`-scheme link to click inside their own authenticated
 *  session — this construction makes that impossible regardless of what the header contains.
 *  `rel="noopener noreferrer"` — `noreferrer` so this admin's own presence on this page/instance
 *  isn't leaked to the third-party lookup site via the `Referer` header, `noopener` so the opened
 *  tab can't reach back into this one via `window.opener`. Shared by `Settings/ApiTokensSection.tsx`
 *  and the Activity page. */
export function IpGeoLink({ ip }: { ip: string }) {
  if (isPrivateOrLocalIp(ip)) return <>{ip}</>
  return (
    <a href={`https://ipinfo.io/${encodeURIComponent(ip)}`} target="_blank" rel="noopener noreferrer">
      {ip}
    </a>
  )
}
