// Shared lookup and formatting for the generated API pages' components.
//
// Plain JS on purpose: this module's only consumer is the build (Astro renders these components
// server-side, so nothing here reaches a browser), and typing a 400KB JSON document's loose shape
// would cost more than it catches.

// The import attribute is what lets this module also load under bare Node (the smoke checks a
// contributor runs by hand); Vite/Astro accept it too. Without it Node rejects the JSON import.
import spec from '../generated/openapi.json' with { type: 'json' }

export const METHODS = ['get', 'post', 'put', 'delete', 'patch', 'head', 'options']

/** Resolve an `operationId` back to its method and path. Throws rather than rendering a blank section. */
export function findOperation(operationId) {
  for (const [path, item] of Object.entries(spec.paths ?? {})) {
    for (const method of METHODS) {
      const op = item[method]
      if (op && op.operationId === operationId) return { method, path, op }
    }
  }
  throw new Error(
    `openapi.mjs: no operation with operationId "${operationId}". The generated pages and the ` +
      `document have drifted apart — re-run the docs build from a clean generated/ directory.`,
  )
}

/**
 * What credentials an operation actually accepts, read from the document rather than assumed.
 *
 * The document derives this from the server's own route policy (`route_policy.csv`), so this is the
 * same authority the server enforces. It matters here because the first version of this module
 * emitted `--header 'Authorization: Bearer YOUR_SECRET_TOKEN'` unconditionally: for the 25
 * endpoints that deliberately refuse API tokens, copy-as-cURL produced a command guaranteed to
 * fail, and the try-it panel offered a token field that could not work.
 *
 * Returns `'none'` (no credentials at all), `'session'` (cookie only — a token is refused),
 * `'token'` (bearer only), or `'either'`.
 */
export function credentialRequirement(operation) {
  const requirement = operation.security ?? spec.security ?? []
  if (Array.isArray(requirement) && requirement.length === 0) return 'none'
  const schemes = new Set(
    (Array.isArray(requirement) ? requirement : []).flatMap((entry) => Object.keys(entry ?? {})),
  )
  const token = schemes.has('bearerAuth')
  const session = schemes.has('sessionCookie')
  if (token && session) return 'either'
  if (token) return 'token'
  if (session) return 'session'
  return 'none'
}

/**
 * Which API token role an operation accepts: `'admin'`, `'any'`, or `'none'`.
 *
 * Read from `x-lanrurugi-token-role` combined with `security`, because the marker alone is only half
 * the answer — it is absent both for "any token works" and for "no token works". 117 of this API's
 * routes refuse a guest token, and without stating that a reader holding one only finds out from a
 * 403 that looks like a permissions mistake.
 */
export function tokenRole(operation) {
  if (credentialRequirement(operation) === 'session' || credentialRequirement(operation) === 'none') {
    return 'none'
  }
  return operation['x-lanrurugi-token-role'] === 'admin' ? 'admin' : 'any'
}

/** The document's own default server. The reader replaces it with their instance's address. */
export function defaultServer() {
  return (spec.servers ?? [])[0]?.url ?? 'http://localhost:3000'
}

export function fullUrl(path) {
  return `${defaultServer().replace(/\/+$/, '')}${path}`
}

/**
 * Follow a schema's `$ref` chain into the document's own components.
 *
 * Needed rather than optional: the upstream document describes several parameters as a bare
 * `{"$ref": "#/components/schemas/CategoryId"}` (and friends), so reading `schema.type` directly
 * renders no type at all — which is exactly how `category` lost its `string · min: 14 · max: 14`
 * line on `/api/search` the first time this was built.
 */
function resolveRef(schema) {
  let current = schema
  for (let depth = 0; depth < 5 && current && current.$ref; depth += 1) {
    const name = current.$ref.split('/').pop()
    current = spec.components?.schemas?.[name]
  }
  return current ?? {}
}

/**
 * Render the subset of markdown the document's prose actually uses inside an HTML string.
 *
 * The document's parameter descriptions are markdown — `/api/search`'s `filter` uses `**bold**` for
 * each character class it documents, and 136 descriptions quote identifiers in backticks. Rendering
 * them as plain text showed the asterisks literally. HTML is escaped *first*, so this can only ever
 * produce the two inline tags below.
 */
export function inlineMarkdown(text) {
  if (!text) return ''
  const escaped = String(text)
    .replace(/&/g, '&amp;')
    .replace(/</g, '&lt;')
    .replace(/>/g, '&gt;')
    .replace(/"/g, '&quot;')
  return escaped
    .replace(/`([^`]+)`/g, '<code>$1</code>')
    // Non-greedy and line-scoped. A `[^*]+` body looks tempting but breaks on the document's own
    // `**Asterisk (*), Percentage Sign (%)**` — the inner `*` stops the match, so the regex pairs
    // across lines and leaves stray `**` behind (which is exactly what the first attempt shipped).
    .replace(/\*\*([^\n]+?)\*\*/g, '<strong>$1</strong>')
}

/** Human-readable type + constraint badges for one parameter, matching the reference's inline form. */
export function paramDetail(parameter) {
  const schema = resolveRef(parameter.schema)
  const bits = []
  if (schema.type) bits.push(schema.type)
  if (schema.minLength !== undefined) bits.push(`min: ${schema.minLength}`)
  if (schema.maxLength !== undefined) bits.push(`max: ${schema.maxLength}`)
  if (schema.minimum !== undefined) bits.push(`min: ${schema.minimum}`)
  if (schema.maximum !== undefined) bits.push(`max: ${schema.maximum}`)
  if (schema.pattern) bits.push('pattern')
  if (schema.enum) bits.push(`enum: ${schema.enum.join(' | ')}`)
  if (schema.default !== undefined) bits.push(`default: ${schema.default}`)
  return bits
}

/** The request example shown next to a parameter list, and copied by the panel's cURL button. */
export function exampleRequest({ method, path, parameters = [], requirement = 'either' }) {
  const query = parameters
    .filter((p) => p.in === 'query')
    .slice(0, 2)
    .map((p) => {
      const sample = p.example ?? p.schema?.example ?? p.schema?.enum?.[0] ?? sampleFor(p.schema)
      return `${encodeURIComponent(p.name)}=${encodeURIComponent(sample ?? '')}`
    })
    .join('&')
  const url = `${fullUrl(path)}${query ? `?${query}` : ''}`
  const lines = [`curl '${url}'`]
  // Only when a token is actually accepted. `session` and `none` get no Authorization header: the
  // command should be one the reader can paste and have work.
  if (requirement !== 'session' && requirement !== 'none') {
    lines.push(`  --header 'Authorization: Bearer YOUR_SECRET_TOKEN'`)
  }
  if (method !== 'get' && method !== 'head') {
    lines.push(`  --request ${method.toUpperCase()}`)
  }
  return { url, curl: lines.join(' \\\n'), requirement }
}

function sampleFor(schema) {
  if (!schema) return ''
  if (schema.enum?.length) return schema.enum[0]
  switch (schema.type) {
    case 'integer':
    case 'number':
      return '0'
    case 'boolean':
      return 'true'
    default:
      return ''
  }
}

/**
 * The response body to show for a status code: an explicit `example` when the document carries one,
 * otherwise a guess built from the schema — and `null` when neither exists, so the caller can say
 * so instead of printing an empty block.
 */
export function responseExample(response) {
  const json = response?.content?.['application/json']
  if (!json) return null
  if (json.example !== undefined) return JSON.stringify(json.example, null, 2)
  if (json.examples) {
    const first = Object.values(json.examples)[0]
    if (first?.value !== undefined) return JSON.stringify(first.value, null, 2)
  }
  if (!json.schema) return null
  return JSON.stringify(sampleFromSchema(json.schema, 0), null, 2)
}

function sampleFromSchema(schema, depth) {
  if (!schema || depth > 3) return null
  if (schema.example !== undefined) return schema.example
  if (schema.enum?.length) return schema.enum[0]
  if (schema.$ref) {
    const name = schema.$ref.split('/').pop()
    const resolved = spec.components?.schemas?.[name]
    return resolved ? sampleFromSchema(resolved, depth + 1) : null
  }
  if (schema.type === 'array') return [sampleFromSchema(schema.items, depth + 1)]
  if (schema.type === 'object' || schema.properties) {
    const out = {}
    for (const [key, value] of Object.entries(schema.properties ?? {})) {
      out[key] = sampleFromSchema(value, depth + 1)
    }
    return out
  }
  switch (schema.type) {
    case 'integer':
    case 'number':
      return 0
    case 'boolean':
      return false
    case 'string':
      return ''
    default:
      return null
  }
}

/** Query parameters a reader can fill in before sending, in the order the document lists them. */
export function editableParams(operation) {
  return (operation.parameters ?? []).filter((p) => p.in === 'query' || p.in === 'path')
}
