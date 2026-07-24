// Converted from LANraragi/lib/LANraragi/Plugin/Scripts/SourceFinder.pm, then hand-fixed since the
// converter's raw output called a nonexistent JS equivalent of
// `LANraragi::Model::Stats::is_url_recorded` (a `LRR_URLMAP` Redis hash read this repo doesn't
// maintain, and a Deno-sandboxed plugin has no direct Redis access regardless) — the host now
// resolves the match itself (mirroring `GET /database/stats`'s own full-tag-scan simplification,
// including the E-Hentai/ExHentai domain-alias special case) and passes the result in as
// `hostArgs.existing_archive_id` (see `ScriptHostArgs`'s own docs in `plugin-sdk.ts`).

declare global {
  interface PerlLogger {
    debug(msg: string): void;
    info(msg: string): void;
    warn(msg: string): void;
    error(msg: string): void;
  }
  // deno-lint-ignore no-var
  var perlCompat: {
    getLogger(name: string, category: string): PerlLogger;
  };
}

// Mirrors `crates/lanrurugi-plugin/dispatcher/plugin-sdk.ts`'s `PluginErrorException` — defined
// locally (not imported) since a plugin file is loaded via a standalone `import()` with no
// relative-path relationship to the SDK file, and the dispatcher's catch block detects this by
// property shape (`error_code`/`data` on a thrown `Error`), not `instanceof`, for exactly that
// reason (see `dispatcher.ts`'s own comment on this). `error_code` is an i18n lookup key — write
// it as a natural, stable phrase that does not embed any dynamic value (that goes in `data`
// instead), so the same `error_code` translates regardless of which specific value triggered it.
class PluginErrorException extends Error {
  constructor(
    public error_code: string,
    public data?: Record<string, string | number>,
  ) {
    super(error_code);
  }
}

export function pluginInfo() {
  return {
    namespace: "urlfinder",
    type: "script" as const,
    parameters: [],
    declared_permissions: { net: [], read: false, write: false },
    name: "Source Finder",
    author: "Difegue (ported)",
    description: "Looks in the database if an archive has a 'source:' tag matching the given URL.",
    version: "0.1",
    icon: "data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAABQAAAAUCAIAAAAC64paAAAAAXNSR0IArs4c6QAAAARnQU1BAACxjwv8YQUAAAAJcEhZcwAADsMAAA7DAcdvqGQAAABZSURBVDhPzY5JCgAhDATzSl+e/2irOUjQSFzQog5hhqIl3uBEHPxIXK7oFXwVE+Hj5IYX4lYVtN6MUW4tGw5jNdjdt5bLkwX1q2rFU0/EIJ9OUEm8xquYOQFEhr9vvu2U8gAAAABJRU5ErkJggg==",
    oneshot_arg: "URL to search.",
  };
}

export async function runScript(hostArgs: Record<string, unknown>) {
  const url = (hostArgs["oneshot_param"] as string | undefined) ?? "";
  const logger = perlCompat.getLogger("Source Finder", "plugins");
  logger.debug(`Looking for URL ${url}`);

  if (url.trim() === "") {
    return { error: { error_code: "No URL specified!" }, total: 0 };
  }

  const existingId = hostArgs["existing_archive_id"] as string | null | undefined;
  if (existingId) {
    return { total: 1, id: existingId };
  }

  return { error: { error_code: "URL not found in database." }, total: 0 };
}
