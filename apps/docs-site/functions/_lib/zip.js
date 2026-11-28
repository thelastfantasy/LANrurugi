// Reads single files out of a zip stored in R2 without fetching the whole archive: the end-of-central-
// directory record and the central directory with two ranged reads (cached per archive), then one
// ranged read per file. Only what a docs archive uses is supported: stored (0) and deflate (8)
// entries, no zip64, no encryption — `release.yml`'s `zip -r` of a few MB of HTML never needs more.

const EOCD_SIGNATURE = 0x06054b50
const CENTRAL_SIGNATURE = 0x02014b50
const LOCAL_SIGNATURE = 0x04034b50
// The EOCD record is 22 bytes plus a comment of at most 65535 bytes.
const EOCD_SEARCH = 22 + 0xffff

/** Parsed central directories, per archive key. Archives are immutable once uploaded. */
const directories = new Map()

async function readRange(bucket, key, range) {
  const object = await bucket.get(key, { range })
  if (!object) return null
  return new Uint8Array(await object.arrayBuffer())
}

/** `name -> { method, compressedSize, size, offset }` for every entry, or `null` if the archive is
 *  missing or not a zip this reader understands. */
export async function centralDirectory(bucket, key) {
  if (directories.has(key)) return directories.get(key)
  const head = await bucket.head(key)
  if (!head) return null
  const tailLength = Math.min(head.size, EOCD_SEARCH)
  const tail = await readRange(bucket, key, { offset: head.size - tailLength, length: tailLength })
  if (!tail) return null
  const view = new DataView(tail.buffer, tail.byteOffset, tail.byteLength)
  let eocd = -1
  for (let i = tail.length - 22; i >= 0; i -= 1) {
    if (view.getUint32(i, true) === EOCD_SIGNATURE) {
      eocd = i
      break
    }
  }
  if (eocd < 0) return null
  const count = view.getUint16(eocd + 10, true)
  const size = view.getUint32(eocd + 12, true)
  const offset = view.getUint32(eocd + 16, true)
  const raw = await readRange(bucket, key, { offset, length: size })
  if (!raw) return null
  const dir = new DataView(raw.buffer, raw.byteOffset, raw.byteLength)
  const decoder = new TextDecoder()
  const entries = new Map()
  let at = 0
  for (let n = 0; n < count; n += 1) {
    if (at + 46 > raw.length || dir.getUint32(at, true) !== CENTRAL_SIGNATURE) return null
    const method = dir.getUint16(at + 10, true)
    const compressedSize = dir.getUint32(at + 20, true)
    const uncompressedSize = dir.getUint32(at + 24, true)
    const nameLength = dir.getUint16(at + 28, true)
    const extraLength = dir.getUint16(at + 30, true)
    const commentLength = dir.getUint16(at + 32, true)
    const localOffset = dir.getUint32(at + 42, true)
    const name = decoder.decode(raw.subarray(at + 46, at + 46 + nameLength))
    entries.set(name, { method, compressedSize, size: uncompressedSize, offset: localOffset })
    at += 46 + nameLength + extraLength + commentLength
  }
  directories.set(key, entries)
  return entries
}

/** The bytes of `name` inside the archive, or `null` when it has no such file. */
export async function readEntry(bucket, key, name) {
  const entries = await centralDirectory(bucket, key)
  const entry = entries?.get(name)
  if (!entry || name.endsWith('/')) return null
  const header = await readRange(bucket, key, { offset: entry.offset, length: 30 })
  if (!header) return null
  const view = new DataView(header.buffer, header.byteOffset, header.byteLength)
  if (view.getUint32(0, true) !== LOCAL_SIGNATURE) return null
  // The local header's own name/extra lengths, which may differ from the central directory's.
  const start = entry.offset + 30 + view.getUint16(26, true) + view.getUint16(28, true)
  if (entry.compressedSize === 0) return new Uint8Array(0)
  const data = await readRange(bucket, key, { offset: start, length: entry.compressedSize })
  if (!data) return null
  if (entry.method === 0) return data
  if (entry.method !== 8) return null
  const stream = new Blob([data]).stream().pipeThrough(new DecompressionStream('deflate-raw'))
  return new Uint8Array(await new Response(stream).arrayBuffer())
}

/** Test seam: forget cached directories. */
export function clearCache() {
  directories.clear()
}
