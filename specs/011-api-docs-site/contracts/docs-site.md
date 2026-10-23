> **Superseded in part (2026-10-11) — see [`contracts/deliverables.md`](./contracts/deliverables.md).**
> The rendering decisions recorded in this file were revisited after comparing the result against
> the reference documentation site: the whole-page Scalar viewer was replaced by generated
> per-group pages, and the API document now also feeds a standalone Swagger UI. Where this file
> still says `@scalar/starlight` or `docs/public/sdk/`, the deliverable contract is authoritative.

# Contract: Documentation Site Build & Deployment

**Feature**: `011-api-docs-site` | **Date**: 2026-10-11

## Package identity

| Property | Value |
|---|---|
| pnpm package name | `lanrurugi-docs` |
| Directory | `docs/` |
| Workspace registration | `pnpm-workspace.yaml` gains `"docs"` (it currently lists only `apps/*`) |
| Framework | Astro 7 + `@astrojs/starlight` + `@scalar/starlight` |

Deliberately **not** placed under `apps/`: `apps/*` is where deployable applications live, and this
is a static documentation build, not a second application. It shares no source with
`apps/frontend`.

## Inputs

The site build consumes two generated artifacts and never produces them:

| Path | Produced by | Contents |
|---|---|---|
| `docs/public/openapi.json` | `mise run docs-api` (the Rust generator) | The API description the reference renders. |
| `docs/dist-sdk/` | `mise run docs-sdk` (Deno) | The browsable SDK reference (HTML tree) and `plugin-sdk.json` (machine-readable) — its own Cloudflare Pages project; see `deliverables.md`. |

Both are gitignored. If either is missing, the build fails — an API reference page that silently
renders an empty document is worse than a failed build, so the build does not tolerate absence.

`docs/api/**` holds the generator's *inputs* (the vendored upstream document and the sidecar). Astro
never reads that directory; it exists inside `docs/` because the two belong to the same feature and
keeping them adjacent is what makes the sidecar discoverable.

## Configuration contract

| Setting | Source | Cloudflare Pages | In-image (`LANRURUGI_DOCS_DIR`) |
|---|---|---|---|
| `base` | env `DOCS_BASE` | `/` | `/docs/` |
| `site` | env `DOCS_SITE_URL` | the deployment's public origin | unset for a relative build |

`defaultLocale` is not set; the site uses Starlight's `root` locale
(`locales: { root: { label: '简体中文', lang: 'zh-CN' } }`), which is the framework's documented
pattern for a monolingual site in a non-English language and keeps default-language content directly
in `src/content/docs/`. Adding a language later is a `locales` entry plus a content subdirectory —
no restructuring, and the generated API reference is never translated or duplicated per language.

Both builds are the same command against the same sources and the same generated document; only the
environment differs. This is what keeps the published site and the offline copy from becoming two
independently maintained documents (spec FR-001).

`base` is not cosmetic here: the API reference plugin strips Astro's `base` before resolving its own
route, and every internal link, asset reference, and the SDK sub-tree link must carry the prefix when
the site is mounted under `/docs`. A root-based build served from `/docs` produces a site whose links
404 on every click.

## Site structure

```text
docs/src/content/docs/
├── index.mdx                    # landing page
├── guide/
│   ├── install.md               # Docker / NAS installation
│   ├── quickstart.md
│   └── api-testing.md           # the "Test it" preconditions (CORS, HTTPS/mixed content, cURL)
└── plugin/
    └── getting-started.md       # links into the SDK reference
```

The API reference itself is not a content page: `@scalar/starlight` injects its own route and sidebar
entry. It is configured with `configuration.url = "<base>openapi.json"` — a same-origin path, so the
document is fetched from the site itself and never from an external host (FR-023).

The SDK reference is linked as a static sub-path (`<base>sdk/`), because `deno doc --html` output is
already a complete, self-contained, searchable HTML reference. Re-rendering it inside Starlight would
mean writing a `deno doc` JSON renderer for no gain over the tool's own output.

## Scalar configuration contract

| Setting | Value | Why |
|---|---|---|
| `url` | `<base>openapi.json` | Same-origin; no external fetch. |
| `cdn` | `<base>scalar/standalone.js`, copied from the installed package by `docs/scripts/prepare-static-assets.mjs` | **Unset would silently load the API reference's runtime from `https://cdn.jsdelivr.net`** (`@scalar/client-side-rendering`'s own default), making the API page the one part of the site that does not work offline. It must be the **UMD** bundle, not the code-split ESM one: `@scalar/astro` injects a plain `<script src>` with no `type="module"` and reads the `window.Scalar` global (`src/client.ts`'s `loadCdn`). Confirmed live — pointing it at `standalone.esm.js` gives "Cannot use import statement outside a module" and the reference never mounts. |
| `withDefaultFonts` | `false` | Scalar's default loads Inter/JetBrains Mono from `https://fonts.scalar.com` on every page view. That is a second external fetch, and the reference falls back to the site's own font stack. |
| `proxyUrl` | **unset, permanently** | The project does not relay readers' requests. Setting this would route their tokens and internal addresses through project infrastructure (spec FR-013). |
| `servers` | from the document (`http://localhost:3000`) | Just the default; the reader adds their own instance through Scalar's own "Add Server" control. |
| `authentication` | derived from the document's `bearerAuth` scheme | Gives the reader a Bearer token field without any custom UI. |
| `hideTestItPanel` | not set | The in-page request panel is a requested feature, not something to hide. |

No custom component wraps the reference. The server input, token input, code samples (including
cURL), and multi-language snippets are all Scalar's own UI; building a parallel version would be
duplicated surface area that drifts from the upstream component.

**FR-023 is verified, not assumed.** The built site was served locally and the API reference loaded
in a real browser with its network panel open: every request went to that local server — the page,
the Astro assets, `/scalar/standalone.js`, `openapi.json`, and the favicon. An earlier revision of
this arrangement produced two external requests (the jsdelivr runtime and the font host), which is
exactly why this is a check on the running page rather than on the configuration alone.

## Required hand-written content

`guide/api-testing.md` is not optional filler — it is the delivery vehicle for spec FR-015. It must
state, in plain terms:

1. The reference sends requests **from your browser directly to the address you type**. Nothing is
   sent to the project.
2. Your instance must be reachable from the browser you are using.
3. **Cross-origin access must be enabled on your instance** (Settings → "Enable CORS for the Client
   API"), and must actually allow the headers a JSON request needs.
4. **An HTTPS docs page can only call `https://` instances and `http://localhost`.** Plain-HTTP
   instances on a LAN address will be blocked by the browser as mixed content. Workarounds:
   put the instance behind HTTPS (reverse proxy, tunnel, VPN), or forward the port to localhost
   (`ssh -L 3000:127.0.0.1:3000 <host>`), which browsers treat as trustworthy.
5. **Every operation has a ready-to-copy command line.** When the in-browser request cannot work,
   use it — it never depends on browser policy.

## Build and deployment

| Step | Command | Where |
|---|---|---|
| Generate the API document | `mise run docs-api` | CI `docs-generate`; Docker `rust-builder` |
| Generate SDK artifacts | `mise run docs-sdk` | CI `docs-generate`; Docker `docs-builder` |
| Build the site (root base) | `DOCS_BASE=/ pnpm --filter lanrurugi-docs run build` | CI `docs-build` |
| Build the site (sub-path base) | `DOCS_BASE=/docs/ pnpm --filter lanrurugi-docs run build` | Docker `docs-builder` |
| Publish | `wrangler pages deploy docs/dist --project-name=<project>` | CI `docs-build`, only on a push to the release branch, only when the deployment secrets are present |

Deployment is **prebuilt-artifact deployment**: Cloudflare Pages receives the already-built `dist/`.
It is not asked to build the site, because it cannot see the generated document or this repository's
pinned toolchain.

The deploy step is conditional on its secrets. A fork, a pull request, or an instance of this
repository without Cloudflare credentials must still be able to run the full pipeline and get a
green build; publishing is the only thing that is skipped.

## What must never be published

- `specs/**` — the project's own spec-kit design record (spec FR-028). It is neither a content
  directory nor a `public/` entry, so it is excluded by construction; this is recorded here so that
  it stays excluded on purpose.
- The sidecar and vendored upstream sources under `docs/api/**` — they are inputs, and the generated
  document is the intended output.
- Any absolute filesystem path. The SDK HTML output is verified clean; the SDK JSON is normalised
  before it is written; `x-lanrurugi-source` values are repository-relative.
