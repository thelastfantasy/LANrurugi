/** Private/loopback/link-local IPs have no meaningful geolocation, so `IpGeoLink` renders them as
 *  plain text instead of a dead-end link. */
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

/** Links an IP to an external geolocation lookup. `ip` is only ever a URL-encoded path segment,
 *  never the href's scheme/host, since it can come from a spoofable header. */
export function IpGeoLink({ ip }: { ip: string }) {
  if (isPrivateOrLocalIp(ip)) return <>{ip}</>
  return (
    <a href={`https://ipinfo.io/${encodeURIComponent(ip)}`} target="_blank" rel="noopener noreferrer">
      {ip}
    </a>
  )
}
