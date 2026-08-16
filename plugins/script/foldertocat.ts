// Real `.ts` script-plugin equivalent of `LANraragi/lib/LANraragi/Plugin/Scripts/FolderToCat.pm`
// ("Subfolders to Categories") — written by hand (not run through `lanrurugi-plugin-converter`)
// specifically to benchmark this Deno-subprocess plugin path against the native Rust endpoint
// that does the identical job (`lanrurugi-api::scripts::subfolders_to_categories`), which stays
// the one actually wired into the Plugins page's "Maintenance Scripts" section — this file exists
// purely for the head-to-head comparison the user asked for, run manually via "Trigger Script"
// like any other script-type plugin.
//
// Deliberately walks the real filesystem itself via `Deno.readDir` (declares `read: true`, an
// unrestricted `--allow-read` grant) rather than having the host pre-walk the tree and hand over
// a file list — a host-side pre-walk would make any Rust-vs-TS timing comparison meaningless,
// since the actual I/O would only ever happen once, on the Rust side, either way.

// Mirrors `lanrurugi_scanner::watcher::WATCHED_EXTENSIONS` exactly (verified against that
// module's own doc comment, itself verified against legacy's `~/LANraragi/lib/Shinobu.pm`).
const WATCHED_EXTENSIONS = new Set([
  "zip", "rar", "7z", "tar", "gz", "lzma", "xz", "cbz", "cbr", "cb7", "cbt", "pdf", "epub", "zst",
]);

function isWatchedArchivePath(path: string): boolean {
  if (path.split("/").includes("thumb")) return false;
  const ext = path.split(".").pop()?.toLowerCase();
  return ext !== undefined && WATCHED_EXTENSIONS.has(ext);
}

export function pluginInfo() {
  return {
    namespace: "foldertocat",
    type: "script" as const,
    parameters: [
      { name: "delete_old_categories", description: "Delete all your static categories before creating the ones matching your subfolders", required: false, type: "bool" },
      { name: "by_top_folder", description: "Use top level subfolders only to create categories", required: false, type: "bool" },
    ],
    declared_permissions: { net: [], read: true, write: false },
    name: "Subfolders to Categories (TS benchmark)",
    author: "Difegue (ported)",
    description: "Scan your Content Folder and automatically create Static Categories for each subfolder. This Script will create a category for each subfolder with archives as direct children.<br>This is a real Deno-subprocess script plugin, kept alongside the native Rust endpoint the Plugins page actually uses, purely to benchmark the two against each other.",
    version: "0.1",
  };
}

async function walkSubfolders(
  root: string,
  current: string,
  byTopFolder: boolean,
  out: Map<string, string[]>,
): Promise<void> {
  let entries: Deno.DirEntry[];
  try {
    entries = await Array.fromAsync(Deno.readDir(current));
  } catch {
    return;
  }
  for (const entry of entries) {
    const path = `${current}/${entry.name}`;
    if (entry.isDirectory) {
      await walkSubfolders(root, path, byTopFolder, out);
      continue;
    }
    if (current === root) continue; // direct children of the library root are excluded, matching legacy
    if (!isWatchedArchivePath(path)) continue;

    const folderName = byTopFolder
      ? current.slice(root.length + 1).split("/")[0]
      : current.split("/").pop();
    if (!folderName) continue;

    const list = out.get(folderName) ?? [];
    list.push(path);
    out.set(folderName, list);
  }
}

export async function runScript(hostArgs: Record<string, unknown>) {
  const logger = legacyCompat.getLogger("Subfolders to Categories (TS)", "plugins");
  const libraryPath = hostArgs["library_path"] as string | undefined;
  const archiveIdByPath = (hostArgs["archive_id_by_path"] as Record<string, string> | undefined) ?? {};
  const customargs = (hostArgs["customargs"] as string[] | undefined) ?? [];
  const deleteOldCategories = customargs[0] === "1" || customargs[0] === "true";
  const byTopFolder = customargs[1] === "1" || customargs[1] === "true";

  if (!libraryPath) {
    return { error: { error_code: "No library path provided by host." } };
  }

  const start = performance.now();
  const subfolders = new Map<string, string[]>();
  await walkSubfolders(libraryPath, libraryPath, byTopFolder, subfolders);

  const categoriesToCreate = Array.from(subfolders.entries()).map(([name, paths]) => ({
    name,
    archive_ids: paths.map((p) => archiveIdByPath[p]).filter((id): id is string => Boolean(id)),
  }));
  const elapsedMs = performance.now() - start;

  logger.info(`Found ${categoriesToCreate.length} subfolders in ${elapsedMs.toFixed(1)}ms`);

  return {
    categories_to_create: categoriesToCreate,
    delete_old_categories: deleteOldCategories,
    elapsed_ms: elapsedMs,
  };
}
