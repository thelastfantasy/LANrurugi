# Contract: the three published deliverables

This feature publishes **three independently deployable artefacts** from one generated
`openapi.json`. They are separate because they answer separate questions for separate readers, and
because bundling them would put a 1.6 MB viewer bundle in front of someone reading the install guide.

| # | Artefact | Build output | Build command | Cloudflare Pages project |
|---|----------|--------------|---------------|--------------------------|
| 1 | Documentation site | `docs/dist/` | `mise run docs-build` | `lanrurugi-docs` |
| 2 | Swagger UI page | `docs/dist-swagger/` | `mise run docs-swagger` | `lanrurugi-api-swagger` |
| 3 | Plugin SDK reference | `docs/dist-sdk/` | `mise run docs-sdk` | `lanrurugi-sdk` |

## What each one is for

**1 — Documentation site.** Guides (`安装`, `快速开始`, `用文档站测试 API`, `插件开发入门`) plus the API
reference as *prose*: one page per API group, each operation a real heading with its method, full path,
description, parameters and response examples, and an in-page request panel. This is the shape the
project's reference site uses.

**2 — Swagger UI page.** The same document, rendered by Swagger UI: browsable schemas (via its JSON
Schema 2020-12 renderer), models, and a request panel that can be pointed at the reader's own
instance. This is the capability deliverable 1 deliberately does not have: a prose reference cannot
answer "what shape is this response?".

**Two locales, `/` (Chinese) and `/en/` (English)** — matching the documentation site's routing.
Swagger UI has no localisation mechanism of its own, and that was measured rather than assumed:
`i18n`, `intl`, `messages:`, `defaultLocale`, `currentLocale` and `lang:` each occur **zero** times
in `swagger-ui-bundle.js`; the six `locale` hits are `localeCompare`/`KeyboardEvent.locale`; every
label is a hardcoded literal (`"Parameters"`, `"Responses"`, `"Authorize"`, …). What *can* be
localised is the document the widget renders, because that text comes from the document. So the build
emits two copies of the spec (`openapi.json` and `openapi.zh.json`, the latter produced by the same
`i18n-core` module the documentation site uses, so the two surfaces cannot disagree) and one static
page per locale, each with a top-bar link to the other. The widget's own chrome stays English on
both pages; that residue is deliberate and is recorded here so it is not mistaken for a bug later.

**3 — Plugin SDK reference.** `deno doc --html` over the same TypeScript the plugin runtime loads,
plus a path-normalised `plugin-sdk.json`. A self-contained static site with its own search and symbol
index.

## Rules

- **One source.** All three derive from artefacts generated in the `docs-generate` job at one
  revision (`docs/public/openapi.json` from `lanrurugi-api-docgen`, `docs/dist-sdk/` from `deno doc`).
  No deliverable regenerates its own input.
- **Cloudflare Pages never builds.** Each project receives a prebuilt directory via
  `wrangler pages deploy`. Pages cannot see this repository's pinned toolchain or the generated
  document, so asking it to build would produce a different site from the one CI validated.
- **Each is served from a domain root.** Every deliverable is built with `base: /`, so no project
  needs a sub-path configuration and no artefact depends on another's hostname — except through one
  explicit link, below.
- **Every Pages project ships a root `404.html`.** Without one, Pages treats the project as a
  single-page app: an unmatched path returns `index.html` with a 200, which hides a broken link
  instead of showing it.
- **Exactly one cross-artefact link.** The docs site's sidebar SDK entry points at the SDK project.
  It is configured with `DOCS_SDK_URL` (default `<base>sdk/`), injected into the build by
  `astro.config.mjs`'s `vite.define` because a non-`PUBLIC_`-prefixed variable is not otherwise in
  `import.meta.env`.
- **Links inside the docs site are relative.** A root-absolute internal link works at `base: /` and
  silently 404s at `base: /docs/`, which is the in-image shape — the two builds must both be correct.

## The server image composes all three

`LANRURUGI_DOCS_DIR` serves one tree, so the `docs-builder` stage copies the three outputs into
`/build/docs/` as:

```
/docs/            ← deliverable 1 (built with DOCS_BASE=/docs/)
/docs/sdk/        ← deliverable 3
/docs/swagger/    ← deliverable 2
```

It builds deliverable 1 with `DOCS_SDK_URL=/docs/sdk/`, so the single cross-artefact link resolves
inside the image too. Composing here rather than teaching deliverable 1 about its siblings is what
lets the same sources publish three projects *and* one offline tree.

## Why a separate Swagger UI rather than a page in the docs site

The first implementation answered "do we need Swagger HTML?" with "no — one JSON rendered by the docs
site". That conflated two different renderings of one document and lost the schema-browsing and
spec-URL capabilities entirely. A page inside the docs site would also mean every reader of the
install guide downloads Swagger UI's bundle. Separate artefact, separate project, shared input.
