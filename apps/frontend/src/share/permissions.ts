import type { SharePermissions } from "./types"

/** The permissions named in a comma- (or newline-) separated setting such as
 * `share_default_permissions`, matching `share_settings::parse_permissions` on the server: spaces
 * around each item and empty items are ignored, so "import, read, download" enables all three. */
export function parseSharePermissions(value: unknown, fallback: string): SharePermissions {
  const names = new Set(
    String(value ?? fallback)
      .split(/[,\n]/)
      .map((name) => name.trim())
      .filter(Boolean),
  )
  return { import: names.has("import"), read: names.has("read"), download: names.has("download") }
}
