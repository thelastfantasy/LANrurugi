#!/usr/bin/env node
// Writes THIRD_PARTY_LICENSES.txt: the license and NOTICE files of every Rust crate and frontend
// package a release ships, as MIT and Apache-2.0 both require when their code is redistributed.
//
//   node scripts/third-party-licenses.mjs --out <file>
//     [--cargo-metadata <json>]    output of `cargo metadata --format-version 1 --locked`; run here when omitted
//     [--path-map <from>=<to>]...  rewrite paths in that output, e.g. when it came from a container
//     [--frontend <dir>]           pnpm package whose production dependencies to list (apps/frontend)
//
// THIRD_PARTY_CARGO_METADATA and THIRD_PARTY_PATH_MAP (comma-separated) supply the same defaults.
// The crate set is every non-workspace package reachable from the workspace through normal (not
// dev or build) dependencies, on any target: listing a crate one platform does not link is harmless,
// leaving out one it does is not.
import { execFileSync } from "node:child_process"
import { existsSync, readdirSync, readFileSync, statSync, writeFileSync } from "node:fs"
import { dirname, join, resolve } from "node:path"
import { fileURLToPath } from "node:url"

const repo = resolve(dirname(fileURLToPath(import.meta.url)), "..")
const args = process.argv.slice(2)
const option = (name) => {
  const i = args.indexOf(name)
  return i === -1 ? undefined : args[i + 1]
}
const options = (name) => args.flatMap((a, i) => (a === name ? [args[i + 1]] : []))

const out = option("--out")
if (!out) {
  console.error("usage: third-party-licenses.mjs --out <file> [--cargo-metadata <json>] [--path-map from=to]... [--frontend <dir>]")
  process.exit(2)
}
const metadataFile = option("--cargo-metadata") ?? process.env.THIRD_PARTY_CARGO_METADATA
const pathMaps = [...options("--path-map"), ...(process.env.THIRD_PARTY_PATH_MAP ?? "").split(",")]
  .filter(Boolean)
  .map((m) => m.split("="))
  // Longest prefix first, so a nested mount wins over the directory that contains it.
  .sort((a, b) => b[0].length - a[0].length)
const frontendDir = resolve(repo, option("--frontend") ?? "apps/frontend")

const mapPath = (p) => {
  for (const [from, to] of pathMaps) {
    const prefix = from.endsWith("/") ? from : `${from}/`
    if (p === from || p.startsWith(prefix)) return `${to.replace(/\/$/, "")}/${p.slice(prefix.length)}`
  }
  return p
}

/** LICENSE*, LICENCE*, COPYING*, NOTICE* (any case) directly in `dir`, plus a LICENSES/ folder. */
function licenseFiles(dir) {
  if (!existsSync(dir)) return []
  const found = []
  for (const name of readdirSync(dir).sort()) {
    const path = join(dir, name)
    if (/^(licen[cs]e|copying|notice)/i.test(name) && statSync(path).isFile()) found.push(path)
    else if (/^licenses$/i.test(name) && statSync(path).isDirectory()) {
      for (const inner of readdirSync(path).sort()) {
        if (statSync(join(path, inner)).isFile()) found.push(join(path, inner))
      }
    }
  }
  return found
}

function section(title, license, homepage, files) {
  const lines = [`${"=".repeat(78)}`, title, `License: ${license || "(not declared)"}`]
  if (homepage) lines.push(`Source: ${homepage}`)
  lines.push(`${"=".repeat(78)}`, "")
  if (files.length === 0) {
    lines.push("(This package ships no license file of its own; the declared license above applies.)", "")
  }
  for (const file of files) lines.push(readFileSync(file, "utf8").replace(/\s+$/, ""), "")
  return lines.join("\n")
}

function rustSections() {
  const json = metadataFile
    ? readFileSync(metadataFile, "utf8")
    : execFileSync("cargo", ["metadata", "--format-version", "1", "--locked"], {
        cwd: repo,
        encoding: "utf8",
        maxBuffer: 1 << 30,
      })
  const metadata = JSON.parse(json)
  const members = new Set(metadata.workspace_members)
  const nodes = new Map(metadata.resolve.nodes.map((n) => [n.id, n]))
  const shipped = new Set()
  const queue = [...members]
  while (queue.length > 0) {
    const node = nodes.get(queue.pop())
    for (const dep of node?.deps ?? []) {
      const normal = dep.dep_kinds.some((k) => k.kind === null)
      if (!normal || shipped.has(dep.pkg)) continue
      shipped.add(dep.pkg)
      queue.push(dep.pkg)
    }
  }
  return metadata.packages
    .filter((p) => shipped.has(p.id) && !members.has(p.id))
    .sort((a, b) => a.name.localeCompare(b.name) || a.version.localeCompare(b.version))
    .map((p) =>
      section(
        `${p.name} ${p.version} (Rust crate)`,
        p.license ?? (p.license_file ? `see ${p.license_file}` : ""),
        p.repository ?? p.homepage,
        licenseFiles(dirname(mapPath(p.manifest_path))),
      ),
    )
}

function frontendSections() {
  const json = execFileSync("pnpm", ["licenses", "list", "--prod", "--json"], {
    cwd: frontendDir,
    encoding: "utf8",
    maxBuffer: 1 << 28,
  })
  // `--prod` follows peer and optional edges too (a font package's peer `next` pulls in `sharp`), so
  // keep only what the bundle really contains: packages named in its source maps, plus the direct
  // dependencies, which also covers fonts and stylesheets that have no source map.
  const bundled = new Set(Object.keys(JSON.parse(readFileSync(join(frontendDir, "package.json"), "utf8")).dependencies ?? {}))
  const assets = join(frontendDir, "dist", "assets")
  if (!existsSync(assets)) throw new Error(`${assets} is missing: build the frontend first`)
  for (const name of readdirSync(assets).filter((n) => n.endsWith(".map"))) {
    for (const source of JSON.parse(readFileSync(join(assets, name), "utf8")).sources ?? []) {
      const match = source.match(/.*node_modules\/((?:@[^/]+\/)?[^/]+)/)
      if (match) bundled.add(match[1])
    }
  }
  const packages = Object.values(JSON.parse(json))
    .flat()
    .filter((p) => bundled.has(p.name))
  return packages
    .sort((a, b) => a.name.localeCompare(b.name))
    .flatMap((p) =>
      p.versions.map((version, i) =>
        section(
          `${p.name} ${version} (frontend package)`,
          p.license,
          p.homepage,
          licenseFiles(p.paths[i] ?? p.paths[0]),
        ),
      ),
    )
}

const rust = rustSections()
const frontend = frontendSections()
const header = [
  "LANrurugi — third-party licenses",
  "",
  "LANrurugi itself is licensed under MIT OR Apache-2.0 (LICENSE-MIT, LICENSE-APACHE).",
  "This file reproduces the license and NOTICE files of the third-party code compiled into its",
  `binaries (${rust.length} Rust crates) and bundled into its web interface (${frontend.length} packages).`,
  "Separately distributed programs in the same package (Redis, Deno, PDFium, ONNX Runtime,",
  "DjVuLibre) carry their own license files alongside them.",
  "",
  "",
].join("\n")
// The texts the "ships no license file" entries above refer to most often.
const appendix = [
  section("Appendix: Apache License, Version 2.0 (full text)", "Apache-2.0", "", [join(repo, "LICENSE-APACHE")]),
  [
    "=".repeat(78),
    "Appendix: MIT License (standard text)",
    "=".repeat(78),
    "",
    readFileSync(join(repo, "LICENSE-MIT"), "utf8").replace(/^MIT License\n\n(Copyright.*\n)+\n/, "MIT License\n\n"),
  ].join("\n"),
]
writeFileSync(out, header + [...rust, ...frontend, ...appendix].join("\n"))
console.error(`third-party-licenses: ${rust.length} crates, ${frontend.length} frontend packages -> ${out}`)
