import { z } from "zod"

// Identifiers must stay unique per archive so the backend can dedup (`add_toc_entry`).
export const TOC_CHAPTER_COUNT = 20

export const TOC_IDENTIFIER_TABLE_OF_CONTENTS = "toc"

export function tocChapterIdentifier(n: number): string {
  return `c${n}`
}

const RESERVED_TOC_IDENTIFIERS = new Set([
  TOC_IDENTIFIER_TABLE_OF_CONTENTS,
  ...Array.from({ length: TOC_CHAPTER_COUNT }, (_, i) => tocChapterIdentifier(i + 1)),
])

export function isReservedTocIdentifier(value: string): boolean {
  return RESERVED_TOC_IDENTIFIERS.has(value.trim().toLowerCase())
}

export function displayTocName(name: string, t: (key: string, opts?: Record<string, unknown>) => string | null): string {
  const lower = name.trim().toLowerCase()
  if (lower === TOC_IDENTIFIER_TABLE_OF_CONTENTS) return t("Table of Contents") ?? "Table of Contents"
  if (/^c([0-9]{1,2})$/.test(lower)) {
    const n = Number(lower.slice(1))
    if (n >= 1 && n <= TOC_CHAPTER_COUNT) return t("Chapter {{n}}", { n }) ?? `Chapter ${n}`
  }
  return name
}

// Blocks a hand-typed title from colliding with a reserved preset identifier.
export const tocTitleSchema = z
  .string()
  .trim()
  .min(1)
  .refine((value) => !isReservedTocIdentifier(value), {
    message: "reserved-toc-identifier",
  })
