// @ts-check
import { readFileSync } from 'node:fs'

import { defineConfig } from 'astro/config'
import starlight from '@astrojs/starlight'

// Read rather than `import ... with { type: 'json' }`: the config is loaded by Astro's own config
// loader, and a plain `fs` read has no import-attribute compatibility to depend on.
const apiNav = JSON.parse(
  readFileSync(new URL('./src/generated/api-nav.json', import.meta.url), 'utf8'),
)

// One source, two builds. The public site is served from a domain root; the copy shipped inside the
// server image is served from the `/docs` sub-path that `LANRURUGI_DOCS_DIR` already provides. Only
// `base` differs between them — the sources, and the generated API document they render, are the
// same bytes. Building twice from one source is what stops the published site and the offline copy
// from drifting into two independently maintained documents.
const base = process.env.DOCS_BASE ?? '/'
const site = process.env.DOCS_SITE_URL

/** @type {import('astro/config').AstroUserConfig} */
// Where the SDK reference lives. Its own Cloudflare Pages project in production, and a sibling
// directory of the composed `/docs` tree inside the server image — so the same page links correctly
// in both, and neither build needs to know about the other's hosting.
const sdkUrl = process.env.DOCS_SDK_URL ?? `${base.replace(/\/$/, '')}/sdk/`

export default defineConfig({
  ...(site ? { site } : {}),
  base,
  // `DOCS_SDK_URL` has no `PUBLIC_` prefix, so it is not in `import.meta.env` by default; injecting
  // it here is what lets the content pages read it (see `plugin/sdk.mdx`).
  vite: {
    define: { 'import.meta.env.DOCS_SDK_URL': JSON.stringify(sdkUrl) },
  },
  integrations: [
    starlight({
      title: 'LANrurugi',
      description:
        'LANraragi 的 Rust + React 重写版：自托管漫画/同人志库管理器。这里放安装、使用、插件开发与 HTTP API 参考。',
      // Two locales, each with its own content directory: `src/content/docs/` is the default
      // (Simplified Chinese, served without a path prefix) and `src/content/docs/en/` is English.
      // Declaring both is what puts the language switcher in the header.
      locales: {
        root: { label: '简体中文', lang: 'zh-CN' },
        en: { label: 'English', lang: 'en' },
      },
      // Rendered by the `SocialIcons` override below as text links. Kept in the config because it is
      // the framework's declared place for these, and the override asserts every entry here is
      // actually rendered — a configured link that silently never appears is its own bug.
      social: [
        { icon: 'github', label: 'GitHub', href: 'https://github.com/thelastfantasy/LANrurugi' },
      ],
      // The sidebar is explicit rather than auto-generated, and every group carries `translations`
      // so the headings follow the reader's chosen language. Group *names* for the API section come
      // from `docs/api/tag-names.json` via the generated nav, so a new API group cannot appear as a
      // raw module name in the sidebar.
      sidebar: [
        {
          label: '用户指南',
          translations: { en: 'User Guide' },
          items: [
            { label: '安装（Docker / NAS）', translations: { en: 'Install (Docker / NAS)' }, slug: 'guide/install' },
            { label: '快速开始', translations: { en: 'Quickstart' }, slug: 'guide/quickstart' },
            { label: '用文档站测试 API', translations: { en: 'Testing the API from these docs' }, slug: 'guide/api-testing' },
          ],
        },
        {
          label: '插件开发',
          translations: { en: 'Writing Plugins' },
          items: [
            { label: '入门', translations: { en: 'Getting started' }, slug: 'plugin/getting-started' },
            // A content page rather than `link: '<base>sdk/'`: Starlight locale-prefixes sidebar
            // `link` values, so a raw path produced `/en/sdk/` — 43 dead links, since the
            // generated reference is a `public/` asset and exists only at `/sdk/`. The page links
            // to it with `import.meta.env.BASE_URL`, which is correct under any base.
            { label: 'SDK 参考', translations: { en: 'SDK Reference' }, slug: 'plugin/sdk' },
          ],
        },
        {
          label: 'API 文档',
          translations: { en: 'API Documentation' },
          // One page per API group, generated from the OpenAPI document by
          // `scripts/generate-api-pages.mjs`. Each page renders its operations as real headings, so
          // Starlight's own right-hand outline lists them — the same structure the reference site
          // uses, and the reason a single embedded API viewer was not good enough.
          items: apiNav.map((group) => ({
            label: group.zh,
            translations: { en: group.en },
            slug: `api/${group.slug}`,
          })),
        },
      ],
      customCss: ['./src/styles/custom.css'],
      components: {
        // Text links instead of icon-only ones; see the component's own docs for which of the
        // reference site's four top-bar links this project can honestly offer.
        SocialIcons: './src/components/SocialIcons.astro',
      },
    }),
  ],
})
