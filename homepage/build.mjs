#!/usr/bin/env node
/*
 * Builds the homepage into dist/: one fully pre-rendered page per language (/, /en/, /ja/) so
 * crawlers see real text, hreflang alternates and per-language metadata without running any JS.
 * No dependencies — `node homepage/build.mjs`, then serve homepage/dist.
 *
 * SITE_URL sets the absolute origin used by canonical/hreflang/og:url/sitemap
 * (env HOMEPAGE_SITE_URL, default the production domain; the Pages project URL serves the same
 * files and these canonical links point search engines at the domain).
 */
import { createHash } from "node:crypto";
import { copyFileSync, existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { COPY } from "./src/copy.mjs";

const ROOT = dirname(fileURLToPath(import.meta.url));
const SRC = join(ROOT, "src");
const DIST = join(ROOT, "dist");
const SITE_URL = (process.env.HOMEPAGE_SITE_URL || "https://lrrg.dev").replace(/\/$/, "");
const REPO_URL = "https://github.com/thelastfantasy/LANrurugi";
// The three sibling sites, for llms.txt.
const DOCS_URL = "https://docs.lrrg.dev";
const API_URL = "https://api.lrrg.dev";
const SDK_URL = "https://sdk.lrrg.dev";

const LANGS = [
  { code: "zh", hreflang: "zh-CN", ogLocale: "zh_CN", dir: "" },
  { code: "en", hreflang: "en", ogLocale: "en_US", dir: "en/" },
  { code: "ja", hreflang: "ja", ogLocale: "ja_JP", dir: "ja/" },
];
// Search engines send unmatched languages here.
const X_DEFAULT = "en";

const STATIC_FILES = ["style.css", "script.js", "favicon.svg", "dafeiyu.webp"];
// Content hash per static file, appended as `?v=` so a changed file is never served from cache.
const VERSIONS = Object.fromEntries(
  STATIC_FILES.filter((f) => existsSync(join(SRC, f))).map((f) => [
    f,
    createHash("sha256").update(readFileSync(join(SRC, f))).digest("hex").slice(0, 10),
  ]),
);
const GENERATED_FILES = ["apple-touch-icon.png"]; // rendered by tools/og.mjs
// Only these glyphs use the display face, so the font request is subset to them.
const DISPLAY_GLYPHS = "第話最終つづく0123456789";
const DISPLAY_FONT = `https://fonts.googleapis.com/css2?family=Dela+Gothic+One&display=swap&text=${encodeURIComponent(DISPLAY_GLYPHS)}`;

const pageUrl = (lang) => `${SITE_URL}/${lang.dir}`;
const ogImageUrl = (lang) => `${SITE_URL}/og-${lang.code}.png`;

const escapeHtml = (s) =>
  s.replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/>/g, "&gt;").replace(/"/g, "&quot;");

/** YAML comments get the dim code color; everything else is plain escaped text. */
const highlightYaml = (yaml) =>
  yaml
    .split("\n")
    .map((line) => {
      const i = line.indexOf("#");
      if (i < 0) return escapeHtml(line);
      return `${escapeHtml(line.slice(0, i))}<span class="c-dim">${escapeHtml(line.slice(i))}</span>`;
    })
    .join("\n");

/**
 * The compose block's host port edits in place, so it is the one part of the YAML that is real
 * inline markup rather than text; script.js keeps step 3's URL in step with it. Everything else
 * still goes through `highlightYaml`.
 */
function composeBody(yaml, label) {
  const attr = escapeHtml(label);
  const html = highlightYaml(yaml);
  if (!/- &quot;\d+:\d+&quot;/.test(html)) throw new Error("start.compose has no ports line to edit");
  return html.replace(
    /- &quot;(\d+):(\d+)&quot;/,
    (_, host, container) =>
      `- &quot;<span class="port-edit" contenteditable="true" role="textbox" aria-multiline="false" ` +
      `inputmode="numeric" spellcheck="false" aria-label="${attr}" title="${attr}">${host}</span>:${container}&quot;`,
  );
}

function text(copy, key, where) {
  const value = copy[key];
  if (value === undefined) throw new Error(`missing copy key "${key}" (${where})`);
  return value;
}

function jsonLd(lang, copy) {
  const data = {
    "@context": "https://schema.org",
    "@graph": [
      {
        "@type": "WebSite",
        "@id": `${SITE_URL}/#website`,
        url: `${SITE_URL}/`,
        name: "LANrurugi",
        inLanguage: LANGS.map((l) => l.hreflang),
      },
      {
        "@type": "SoftwareApplication",
        "@id": `${SITE_URL}/#software`,
        name: "LANrurugi",
        url: pageUrl(lang),
        description: copy["doc.description"],
        inLanguage: lang.hreflang,
        applicationCategory: "MultimediaApplication",
        applicationSubCategory: "Manga reader",
        operatingSystem: "Linux, macOS, FreeBSD, Docker",
        image: ogImageUrl(lang),
        license: "https://opensource.org/licenses/MIT",
        isAccessibleForFree: true,
        offers: { "@type": "Offer", price: "0", priceCurrency: "USD" },
        featureList: [
          copy["ai.translate.title"],
          copy["ai.wizard.title"],
          copy["ai.recommend.title"],
          copy["ai.tank.title"],
          copy["reader.f1.title"],
          copy["reader.f5.title"],
        ],
        softwareHelp: { "@type": "CreativeWork", url: copy["link.docsHref"] },
        sameAs: [REPO_URL],
      },
      {
        "@type": "SoftwareSourceCode",
        name: "LANrurugi",
        codeRepository: REPO_URL,
        programmingLanguage: ["Rust", "TypeScript"],
        license: "https://opensource.org/licenses/MIT",
      },
    ],
  };
  // `</` can't appear inside a script element; none of the copy has it, but stay safe.
  return JSON.stringify(data, null, 2).replace(/<\//g, "<\\/");
}

function render(template, lang) {
  const copy = COPY[lang.code];
  const base = lang.dir ? "../" : "./";
  let html = template;

  // Element text: <tag ... data-i18n="key">…</tag>. Tagged elements contain only text.
  html = html.replace(
    /(<([a-z0-9]+)\b[^>]*\bdata-i18n="([^"]+)"[^>]*>)([\s\S]*?)(<\/\2>)/g,
    (_, open, _tag, key, _old, close) => {
      const value = text(copy, key, "data-i18n");
      const body =
        key === "start.compose"
          ? composeBody(value, text(copy, "start.portEdit", "data-i18n"))
          : escapeHtml(value);
      return open.replace(/\s*\bdata-i18n="[^"]+"/, "") + body + close;
    },
  );

  const hreflang = [
    ...LANGS.map((l) => `    <link rel="alternate" hreflang="${l.hreflang}" href="${pageUrl(l)}" />`),
    `    <link rel="alternate" hreflang="x-default" href="${pageUrl(LANGS.find((l) => l.code === X_DEFAULT))}" />`,
  ].join("\n");
  const ogAlternates = LANGS.filter((l) => l !== lang)
    .map((l) => `    <meta property="og:locale:alternate" content="${l.ogLocale}" />`)
    .join("\n");

  const vars = {
    htmlLang: lang.hreflang,
    url: pageUrl(lang),
    base,
    hreflang,
    ogLocale: lang.ogLocale,
    ogLocaleAlternates: ogAlternates,
    ogImage: ogImageUrl(lang),
    displayFont: DISPLAY_FONT,
    jsonLd: jsonLd(lang, copy),
    // Chinese is typeset vertically like the source; English (also on the Japanese page) is not.
    readerClass: lang.code === "zh" ? " tr-vertical" : "",
  };

  html = html.replace(/\{\{([a-zA-Z0-9.:]+)\}\}/g, (_, token) => {
    if (token.startsWith("t:")) return escapeHtml(text(copy, token.slice(2), "{{t:}}"));
    if (token.startsWith("href:")) {
      const target = LANGS.find((l) => l.code === token.slice(5));
      return base + target.dir;
    }
    if (token.startsWith("current:")) return token.slice(8) === lang.code ? ' aria-current="page"' : "";
    if (!(token in vars)) throw new Error(`unknown placeholder {{${token}}}`);
    return vars[token];
  });

  const leftover = html.match(/\{\{[^}]*\}\}|data-i18n=/);
  if (leftover) throw new Error(`unresolved template token: ${leftover[0]}`);
  for (const [file, version] of Object.entries(VERSIONS)) {
    html = html.replaceAll(`${base}${file}"`, `${base}${file}?v=${version}"`);
  }
  return html;
}

function notFoundPage() {
  const links = LANGS.map(
    (l) => `<a href="/${l.dir}" hreflang="${l.hreflang}" lang="${l.hreflang}">${escapeHtml(COPY[l.code]["notfound.back"])}</a>`,
  ).join("");
  return `<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8" />
    <meta name="viewport" content="width=device-width, initial-scale=1" />
    <meta name="robots" content="noindex" />
    <title>404 — LANrurugi</title>
    <link rel="icon" href="/favicon.svg" type="image/svg+xml" />
    <link rel="stylesheet" href="/style.css" />
  </head>
  <body>
    <main class="notfound">
      <p class="chapter-no">404</p>
      <h1>${escapeHtml(COPY.en["notfound.title"])}</h1>
      <nav class="notfound-links">${links}</nav>
    </main>
  </body>
</html>
`;
}

function sitemap() {
  const alternates = [
    ...LANGS.map((l) => `    <xhtml:link rel="alternate" hreflang="${l.hreflang}" href="${pageUrl(l)}" />`),
    `    <xhtml:link rel="alternate" hreflang="x-default" href="${pageUrl(LANGS.find((l) => l.code === X_DEFAULT))}" />`,
  ].join("\n");
  const today = new Date().toISOString().slice(0, 10);
  const urls = LANGS.map(
    (l) => `  <url>
    <loc>${pageUrl(l)}</loc>
    <lastmod>${today}</lastmod>
${alternates}
  </url>`,
  ).join("\n");
  return `<?xml version="1.0" encoding="UTF-8"?>
<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9" xmlns:xhtml="http://www.w3.org/1999/xhtml">
${urls}
</urlset>
`;
}

/**
 * llms.txt (https://llmstxt.org): what the project is and where AI tools find it as Markdown or
 * structured data — the docs site's own llms.txt, the OpenAPI document and the plugin SDK JSON.
 */
function llmsTxt() {
  return `# LANrurugi

> A self-hosted library server for manga and comics that also reads PDF, EPUB, MOBI, DjVu, FB2 and TXT books. It started from LANraragi, can open an existing LANraragi library directly, and is rewritten in Rust + React with on-page manga translation, similar-work recommendations and AI plugin generation.

LANrurugi is open source (MIT) and runs from a Docker image or as a native package for Linux, macOS and FreeBSD. This site is the project homepage, in Simplified Chinese (\`/\`), English (\`/en/\`) and Japanese (\`/ja/\`). The documentation, the HTTP API reference and the plugin SDK reference are separate sites, linked below; the documentation site publishes its own llms.txt with the full docs as Markdown.

HTTP API: every endpoint a LANrurugi server serves is described in one OpenAPI 3.1 document, generated from the server's own route registrations, so it lists exactly what the server serves. Endpoints shared with LANraragi keep that project's field-level documentation and stay compatible with its clients. Requests authenticate with an API token created in the instance's settings, sent as \`Authorization: Bearer <token>\`.

Plugin SDK: plugins are single TypeScript files that the server runs in their own Deno subprocess, with only the network and file permissions they declare. Each exports \`pluginInfo()\` plus one entry point for its type: \`execMetadata\` (metadata), \`execLogin\` (login), \`execDownload\` (download) or \`runScript\` (script). The SDK reference is generated by \`deno doc\` from the TypeScript the plugin runtime loads, and is also published as JSON.

## Documentation

- [Documentation llms.txt](${DOCS_URL}/llms.txt): index of the documentation as Markdown, split by language (English, Simplified Chinese) and by guides versus API reference
- [llms.txt for AI tools](${DOCS_URL}/en/guide/llms-txt/): what each of these files contains and which one to use for what
- [Install with Docker](${DOCS_URL}/en/guide/install/): Docker and NAS setup
- [Native install](${DOCS_URL}/en/guide/install-native/): packages for Linux, macOS and FreeBSD
- [Reverse proxy](${DOCS_URL}/en/guide/reverse-proxy/): HTTPS on your own domain with Nginx, Caddy, Traefik, Apache or Nginx Proxy Manager
- [Quickstart](${DOCS_URL}/en/guide/quickstart/): first login, scanning the library, API tokens, migrating from LANraragi
- [Supported formats](${DOCS_URL}/en/guide/formats/): the archive and e-book formats the library accepts and how each is shown

## HTTP API

- [OpenAPI document](${DOCS_URL}/openapi.json): the complete HTTP API as OpenAPI 3.1 JSON
- [API reference](${DOCS_URL}/en/api/): the same document, one page per endpoint group
- [Swagger UI](${API_URL}/en/): browse the schemas and send requests to your own instance
- [Testing the API](${DOCS_URL}/en/guide/api-testing/): sending requests from the browser to your own instance — cross-origin access, HTTPS limits, tokens

## Plugin SDK

- [Writing plugins](${DOCS_URL}/en/plugin/getting-started/): a minimal plugin, the permission model, debugging
- [SDK reference](${SDK_URL}/): every type a plugin is written against
- [plugin-sdk.json](${SDK_URL}/plugin-sdk.json): the SDK reference as JSON

## Optional

- [Complete documentation](${DOCS_URL}/llms-full.txt): every documentation page in both languages, as one Markdown file
- [Homepage (English)](${SITE_URL}/en/): features in prose
- [Source code](${REPO_URL}): repository, issues and releases
`;
}

rmSync(DIST, { recursive: true, force: true });
mkdirSync(DIST, { recursive: true });

const template = readFileSync(join(SRC, "index.html"), "utf8");
for (const lang of LANGS) {
  const out = join(DIST, lang.dir, "index.html");
  mkdirSync(dirname(out), { recursive: true });
  writeFileSync(out, render(template, lang));
}

for (const file of STATIC_FILES) {
  if (!existsSync(join(SRC, file))) throw new Error(`missing static file src/${file}`);
  copyFileSync(join(SRC, file), join(DIST, file));
}
for (const file of GENERATED_FILES) {
  if (existsSync(join(SRC, file))) copyFileSync(join(SRC, file), join(DIST, file));
  else if (!process.env.HOMEPAGE_SKIP_OG) throw new Error(`missing src/${file} — run node homepage/tools/og.mjs`);
}
for (const lang of LANGS) {
  const og = join(SRC, `og-${lang.code}.png`);
  if (!existsSync(og)) {
    if (process.env.HOMEPAGE_SKIP_OG) continue; // tools/og.mjs builds once without them to render them
    throw new Error(`missing src/og-${lang.code}.png — run node homepage/tools/og.mjs`);
  }
  copyFileSync(og, join(DIST, `og-${lang.code}.png`));
}

writeFileSync(join(DIST, "404.html"), notFoundPage());
writeFileSync(join(DIST, "sitemap.xml"), sitemap());
// Cloudflare Pages response headers: images never change in place, so cache them for a week;
// HTML/CSS/JS keep Pages' default revalidation so a deploy shows up immediately. The *.pages.dev
// hosts (the project URL and every preview deployment) are for checking a deploy only, so they are
// kept out of search indexes; the production domain is unaffected.
writeFileSync(
  join(DIST, "_headers"),
  `/*
  X-Content-Type-Options: nosniff
  Referrer-Policy: strict-origin-when-cross-origin
/*.png
  Cache-Control: public, max-age=604800
/favicon.svg
  Cache-Control: public, max-age=604800
https://:project.pages.dev/*
  X-Robots-Tag: noindex
https://:version.:project.pages.dev/*
  X-Robots-Tag: noindex
`,
);
writeFileSync(join(DIST, "llms.txt"), llmsTxt());
writeFileSync(join(DIST, "robots.txt"), `User-agent: *\nAllow: /\n\nSitemap: ${SITE_URL}/sitemap.xml\n`);

console.log(`homepage built → ${DIST} (${LANGS.map((l) => "/" + l.dir).join(", ")}) for ${SITE_URL}`);
