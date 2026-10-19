/** Extensions the server reads as e-books (`lanrurugi_book::BookFormat`): their pages are generated
 *  from the book, so archive-only tools such as split suggestions do not apply. */
export const BOOK_EXTENSIONS: ReadonlySet<string> = new Set([
  "pdf",
  "epub",
  "mobi",
  "azw",
  "azw3",
  "prc",
  "djvu",
  "djv",
  "fb2",
  "txt",
])

export function isBookExtension(extension: string | null | undefined): boolean {
  return BOOK_EXTENSIONS.has((extension ?? "").toLowerCase())
}

/** Archive extensions the server reads (`lanrurugi_scanner::archive_format::ARCHIVE_EXTENSIONS`). */
export const ARCHIVE_EXTENSIONS: readonly string[] = [
  "zip",
  "cbz",
  "rar",
  "cbr",
  "7z",
  "cb7",
  "lzh",
  "lha",
  "tar",
  "cbt",
  "gz",
  "bz2",
  "xz",
  "lzma",
]

/** `accept` value for a file picker offering every format the library can catalogue. */
export const UPLOAD_ACCEPT = [...ARCHIVE_EXTENSIONS, ...BOOK_EXTENSIONS].map((e) => `.${e}`).join(",")
