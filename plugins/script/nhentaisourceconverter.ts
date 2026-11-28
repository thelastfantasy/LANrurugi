// Converted from LANraragi/lib/LANraragi/Plugin/Scripts/nHentaiSourceConverter.pm, then hand-fixed
// since the converter's raw output called a nonexistent JS equivalent of
// `LANraragi::Model::Config->get_redis` (a Deno-sandboxed plugin has no direct Redis/archive-
// storage access) — this plugin names `archives` in `host_arguments`, so the host fetches every
// archive's `id`/`tags` into `hostArgs.archives` before the call, and names `tag_updates` in
// `host_results`, so the rewrites this plugin returns in `updates` are written by the host rather
// than by the plugin itself (see `ScriptHostArgs`'s own docs in `plugin-sdk.ts`).

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
    // Whole-library by design: the rewrite is computed across every record, not just one archive.
    host_arguments: ["archives"],
    // The plugin returns intended rewrites in `updates`; the host writes them back.
    host_results: ["tag_updates"],
  };
}

export async function runScript(hostArgs: Record<string, unknown>) {
  const logger = legacyCompat.getLogger("nHentai Source Converter", "plugins");
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
