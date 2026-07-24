// Converted from LANraragi/lib/LANraragi/Plugin/Scripts/nHentaiSourceConverter.pm, then hand-fixed
// since the converter's raw output called a nonexistent JS equivalent of
// `LANraragi::Model::Config->get_redis` (a Deno-sandboxed plugin has no direct Redis/archive-
// storage access) — the host now fetches every archive's `id`/`tags` into `hostArgs.archives`
// before this call, and this plugin returns its intended rewrites in `updates` for the host to
// apply afterward, rather than writing storage itself (see `ScriptHostArgs`'s own docs in
// `plugin-sdk.ts`).

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

export function pluginInfo() {
  return {
    namespace: "nhsrcconv",
    type: "script" as const,
    parameters: [],
    declared_permissions: { net: [], read: false, write: false },
    name: "nHentai Source Converter",
    author: "Guerra24 (ported)",
    description: "Converts \"source:{id}\" tags with 6 or less digits into \"source:nhentai.net/g/{id}\"",
    version: "0.1",
  };
}

export async function runScript(hostArgs: Record<string, unknown>) {
  const logger = perlCompat.getLogger("nHentai Source Converter", "plugins");
  const archives = (hostArgs["archives"] as { id: string; tags: string }[] | undefined) ?? [];

  let modified = 0;
  const updates: { id: string; tags: string }[] = [];

  for (const archive of archives) {
    let changed = false;
    const newTags = archive.tags
      .split(",")
      .map((raw) => {
        const tag = raw.trim();
        const rewritten = tag.replace(/^source:(\d{1,6})$/, (_match, digits: string) => {
          changed = true;
          return `source:nhentai.net/g/${digits}`;
        });
        return rewritten;
      })
      .join(", ");

    if (changed) {
      modified++;
      updates.push({ id: archive.id, tags: newTags });
      logger.debug(`Rewriting source tag for archive ${archive.id}`);
    }
  }

  return { modified, updates };
}
