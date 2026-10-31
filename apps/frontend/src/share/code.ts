/**
 * Share codes and the `lrrg:` text form — the browser-side mirror of
 * `crates/lanrurugi-share-proto/src/code.rs`. Keep the two in step: `tests/unit/share/code.test.ts`
 * uses the same cases as the Rust tests.
 */

const ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ"
const CODE_LEN = 10
const SHORT_CODE_LEN = 8

export interface Via {
  endpointId: string
  /** Comma-separated addresses, as the text form carries them; may be empty when `relay` is set. */
  addr: string
  /** Comma-separated https relays the sharer is reachable through. */
  relay?: string
}

export interface ShareRef {
  /** Normalised: uppercase, no separators. */
  code: string
  via: Via | null
  /** Comma-separated `<endpoint_id>@<host:port>` of the sharer's LANrurugi nodes (older codes). */
  nodes?: string
  /** The sealed route hint (`r`), passed through as is: only the server opens it. */
  hint?: string
}

/** Normalises user input, or returns `null` when it is not a code. */
export function normalizeShareCode(input: string): string | null {
  let code = ""
  for (const raw of input) {
    if (raw === "-" || /\s/.test(raw)) continue
    let c = raw.toUpperCase()
    if (c === "O") c = "0"
    else if (c === "I" || c === "L") c = "1"
    if (!ALPHABET.includes(c)) return null
    code += c
  }
  return code.length === CODE_LEN || code.length === SHORT_CODE_LEN ? code : null
}

/** `XXXXX-XXXXX`, or `XXXX-XXXX` for a short code. */
export function formatShareCode(code: string): string {
  const half = code.length / 2
  return `${code.slice(0, half)}-${code.slice(half)}`
}

/** The comma-separated items of a parameter that pass `keep`, or `""`. */
function listParam(value: string | undefined, keep: (item: string) => boolean): string {
  return (value ?? "")
    .split(",")
    .map((item) => item.trim())
    .filter((item) => item && keep(item))
    .join(",")
}

function parseVia(value: string, relayParam: string | undefined): Via | null {
  const at = value.indexOf("@")
  if (at <= 0) return null
  const endpointId = value.slice(0, at)
  if (!/^[A-Za-z0-9]+$/.test(endpointId)) return null
  const addr = listParam(value.slice(at + 1), () => true)
  const relay = listParam(relayParam, (r) => r.startsWith("https://"))
  if (!addr && !relay) return null
  return relay ? { endpointId, addr, relay } : { endpointId, addr }
}

/** The code in a share link: the segment after `/s/`, or a `#` fragment. */
function linkCode(url: string): string | null {
  const hash = url.indexOf("#")
  if (hash >= 0) {
    const fromFragment = normalizeShareCode(url.slice(hash + 1))
    if (fromFragment) return fromFragment
  }
  const withoutFragment = hash >= 0 ? url.slice(0, hash) : url
  const path = withoutFragment.split("?")[0]
  const segments = path.split("/").filter((s) => s.length > 0)
  const last = segments.at(-1)
  if (last && segments.at(-2) === "s") return normalizeShareCode(last)
  return null
}

/** Accepts `lrrg:<code>[?r=…]` (or an older code's `via`/`relay`/`node`), a share link (`…/s/<code>`, `…#<code>`) or a bare code. */
export function parseShareRef(input: string): ShareRef | null {
  const text = input.trim()
  const colon = text.indexOf(":")
  if (colon > 0 && text.slice(0, colon).toLowerCase() === "lrrg") {
    const rest = text.slice(colon + 1).replace(/^\/\//, "")
    const [codePart, query = ""] = rest.split("?", 2)
    const code = normalizeShareCode(codePart.replace(/\/+$/, ""))
    if (!code) return null
    const params = new Map<string, string>()
    for (const pair of query.split("&")) {
      const eq = pair.indexOf("=")
      if (eq > 0 && !params.has(pair.slice(0, eq))) params.set(pair.slice(0, eq), pair.slice(eq + 1))
    }
    const hint = params.get("r")
    if (hint && /^[A-Za-z0-9_-]+$/.test(hint)) return { code, via: null, hint }
    const viaText = params.get("via")
    const via = viaText ? parseVia(viaText, params.get("relay")) : null
    const nodes = listParam(params.get("node"), (n) => n.includes("@"))
    return nodes ? { code, via, nodes } : { code, via }
  }
  if (text.includes("://")) {
    const code = linkCode(text)
    return code ? { code, via: null } : null
  }
  const code = normalizeShareCode(text)
  return code ? { code, via: null } : null
}

/** `lrrg:XXXXX-XXXXX`, plus the route hint or an older code's route parameters. */
export function toTextForm(ref: ShareRef): string {
  if (ref.hint) return `lrrg:${formatShareCode(ref.code)}?r=${ref.hint}`
  const params: string[] = []
  if (ref.via) {
    params.push(`via=${ref.via.endpointId}@${ref.via.addr}`)
    if (ref.via.relay) params.push(`relay=${ref.via.relay}`)
  }
  if (ref.nodes) params.push(`node=${ref.nodes}`)
  const base = `lrrg:${formatShareCode(ref.code)}`
  return params.length > 0 ? `${base}?${params.join("&")}` : base
}
