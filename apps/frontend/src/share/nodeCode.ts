/** Mirrors `NodeCode::parse_prefixed` in `crates/lanrurugi-share-proto/src/node_code.rs`: a node
 *  code is 16 Crockford base32 characters, case-insensitive, `O`→`0`, `I`/`L`→`1`, and dashes or
 *  spaces are decoration. Normalising here means the list can be de-duplicated against what the
 *  server actually stores, and a malformed entry is caught before it is sent rather than rejected
 *  by a save that looks like it silently did nothing. */
const ALPHABET = "0123456789ABCDEFGHJKMNPQRSTVWXYZ";

export const NODE_CODE_LEN = 16;

/** Returns the canonical `lrrg-node:XXXX-XXXX-XXXX-XXXX` form, or null if this is not a node code
 *  (a full `lrrg-node:<id>@<host:port>` string is not one — it carries an address). */
export function normalizeNodeCode(input: string): string | null {
  const rest = input.trim().startsWith("lrrg-node:")
    ? input.trim().slice("lrrg-node:".length)
    : input.trim();
  if (rest.includes("@")) return null;
  let code = "";
  for (const raw of rest) {
    if (raw === "-" || /\s/.test(raw)) continue;
    let c = raw.toUpperCase();
    if (c === "O") c = "0";
    else if (c === "I" || c === "L") c = "1";
    if (!ALPHABET.includes(c)) return null;
    code += c;
  }
  if (code.length !== NODE_CODE_LEN) return null;
  const groups = code.match(/.{4}/g);
  return `lrrg-node:${groups ? groups.join("-") : code}`;
}

/** Two entries naming the same node, whatever their spelling. */
export function sameNode(a: string, b: string): boolean {
  const na = normalizeNodeCode(a);
  const nb = normalizeNodeCode(b);
  if (na && nb) return na === nb;
  return a.trim().toLowerCase() === b.trim().toLowerCase();
}
