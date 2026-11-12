// Parse-only syntax gate for the frontend (issue #112).
//
// `tsc -b` only checks what `tsconfig.app.json` / `tsconfig.node.json` include, so `tests/` is never
// compiled, and eslint's TypeScript parser recovers from errors such as an `export` inside a
// function body, `await` outside an async function or a duplicate `let` without reporting them.
// esbuild rejects all of those, the same way Vite and the browser do, in a few milliseconds per file.
//
// Usage: node scripts/check-syntax.mjs <file-or-dir>...  (directories are walked for .ts/.tsx)
import { readFile, readdir, stat } from "node:fs/promises";
import { extname, join } from "node:path";
import { transform } from "esbuild";

const LOADERS = { ".ts": "ts", ".tsx": "tsx", ".mts": "ts", ".cts": "ts" };

async function collect(path, out) {
  const info = await stat(path).catch(() => null);
  if (!info) return;
  if (info.isDirectory()) {
    for (const entry of await readdir(path)) {
      if (entry !== "node_modules" && !entry.startsWith(".")) await collect(join(path, entry), out);
    }
  } else if (LOADERS[extname(path)]) {
    out.push(path);
  }
}

const files = [];
for (const arg of process.argv.slice(2)) await collect(arg, files);

let failed = 0;
await Promise.all(
  files.map(async (file) => {
    try {
      await transform(await readFile(file, "utf8"), {
        loader: LOADERS[extname(file)],
        jsx: "automatic",
        sourcefile: file,
        logLevel: "silent",
      });
    } catch (err) {
      failed++;
      for (const e of err.errors ?? [{ text: String(err) }]) {
        const at = e.location ? `${e.location.file}:${e.location.line}:${e.location.column + 1}` : file;
        console.error(`${at}: ${e.text}`);
      }
    }
  }),
);

if (failed) {
  console.error(`check-syntax: ${failed} of ${files.length} file(s) failed to parse`);
  process.exit(1);
}
