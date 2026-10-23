#!/usr/bin/env node
/*
 * Renders the social-preview images (src/og-{zh,en,ja}.png, 1200×630) from the built page's own
 * hero, plus src/apple-touch-icon.png from the favicon. Dev-time only — needs the frontend's
 * Playwright install; the PNGs are committed so CI's build stays dependency-free.
 */
import { execFileSync } from "node:child_process";
import { readFileSync } from "node:fs";
import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const HOME = join(dirname(fileURLToPath(import.meta.url)), "..");
// Resolved through the frontend workspace, which owns the repo's Playwright dependency.
const require = createRequire(join(HOME, "..", "apps", "frontend", "package.json"));
const { chromium } = require("@playwright/test");

execFileSync(process.execPath, [join(HOME, "build.mjs")], {
  env: { ...process.env, HOMEPAGE_SKIP_OG: "1" },
  stdio: "inherit",
});

const browser = await chromium.launch();
const page = await browser.newPage({ viewport: { width: 1200, height: 630 }, colorScheme: "light" });

for (const [lang, dir] of [["zh", ""], ["en", "en/"], ["ja", "ja/"]]) {
  await page.goto(pathToFileURL(join(HOME, "dist", dir, "index.html")).href, { waitUntil: "networkidle" });
  // Freeze the demo on its translated frame and fit the hero into the card.
  await page.addStyleTag({
    content: `
      .topbar, .hero-tags, .hero-cta { display: none !important; }
      main { padding: 34px 40px 0 !important; max-width: none !important; zoom: 0.86; }
      .hero { grid-template-columns: 1fr 1.05fr !important; }
      .hero-copy { padding: 40px 44px !important; }
      * { animation: none !important; transition: none !important; }`,
  });
  await page.evaluate(() => {
    const reader = document.querySelector(".reader");
    reader.dataset.state = "translated";
    document.querySelector(".rf-status").textContent = reader.dataset.statusTranslated;
    document.querySelector(".rb-translate").setAttribute("aria-pressed", "true");
    document.querySelectorAll(".step").forEach((s) => s.classList.add("on"));
    // The top bar is hidden, so carry the name on the card itself.
    const brand = document.querySelector(".brand").cloneNode(true);
    brand.style.cssText = "font-size:1.6rem;margin-bottom:18px";
    brand.querySelector("img").setAttribute("width", "44");
    brand.querySelector("img").setAttribute("height", "44");
    document.querySelector(".hero-copy").prepend(brand);
  });
  await page.waitForTimeout(300);
  await page.screenshot({ path: join(HOME, "src", `og-${lang}.png`) });
}

const icon = await browser.newPage({ viewport: { width: 180, height: 180 } });
const svg = readFileSync(join(HOME, "src", "favicon.svg"), "utf8");
await icon.setContent(
  `<body style="margin:0;background:#e3e0d1">${svg.replace("<svg ", '<svg style="display:block;width:180px;height:180px" ')}</body>`,
);
await icon.screenshot({ path: join(HOME, "src", "apple-touch-icon.png") });

await browser.close();
execFileSync(process.execPath, [join(HOME, "build.mjs")], { stdio: "inherit" });
