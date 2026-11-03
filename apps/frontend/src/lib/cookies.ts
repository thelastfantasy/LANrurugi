/** Utilities for the plugin-settings "fill from document.cookie" tool. */

/** Parses a pasted `document.cookie` header (`name=value; name2=value2`) into its pairs.
 *
 * Splits on the first `=` only — cookie values can legitimately contain `=` (base64, etc.) — and
 * values are kept verbatim (no percent-decoding: `document.cookie` returns what the site set, and
 * decoding it here could corrupt a value that merely looks encoded). Malformed segments are
 * skipped. */
export function parseCookieHeader(raw: string): Map<string, string> {
  const cookies = new Map<string, string>()
  for (const part of raw.split(";")) {
    const trimmed = part.trim()
    if (!trimmed) continue
    const separator = trimmed.indexOf("=")
    if (separator <= 0) continue
    const name = trimmed.slice(0, separator).trim()
    if (!name) continue
    let value = trimmed.slice(separator + 1).trim()
    if (value.length >= 2 && value.startsWith('"') && value.endsWith('"')) {
      value = value.slice(1, -1)
    }
    cookies.set(name, value)
  }
  return cookies
}

/** Which cookie a plugin parameter expects, if any.
 *
 * The plugin protocol has no cookie-name field, so this follows the convention the real login
 * plugins use in their parameter descriptions ("ipb_member_id cookie", "sk cookie (optional …)"),
 * falling back to the parameter name when the description mentions a cookie without naming one.
 * Parameters with neither are left alone. */
export function cookieNameForParameter(param: { name: string; desc: string }): string | null {
  const described = param.desc.match(/^\s*([A-Za-z_][\w.-]*)\s+cookie\b/i)
  if (described) return described[1]
  if (/cookie/i.test(param.desc) || /cookie/i.test(param.name)) return param.name
  return null
}

/** Whether this form has any cookie-shaped parameter, i.e. whether the import tool is useful. */
export function hasCookieParameters(params: Array<{ name: string; desc: string }>): boolean {
  return params.some((param) => cookieNameForParameter(param) !== null)
}
