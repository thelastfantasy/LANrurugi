// Tests for `plugins/discovery/ehentai.ts`. Kept outside `plugins/`, where every `.ts` file is loaded
// as a plugin.
//
// Run: deno test --config plugins/deno.json --allow-read tests/plugins/ehentai_discovery_test.ts

import { parseRating, refreshSources } from "../../plugins/discovery/ehentai.ts";

function assertEquals(actual: unknown, expected: unknown, message = "") {
  const a = JSON.stringify(actual);
  const e = JSON.stringify(expected);
  if (a !== e) throw new Error(`${message}\n  expected ${e}\n  actual   ${a}`);
}

// The sprite table measured against each gallery's own `Average: N` (see `parseRating`'s docs).
Deno.test("parseRating reads the half-star row as -21", () => {
  const table: [string, number][] = [
    ["0px -21px", 4.5],
    ["0px -1px", 5],
    ["-16px -21px", 3.5],
    ["-16px -1px", 4],
    ["-32px -21px", 2.5],
    ["-32px -1px", 3],
    ["-48px -21px", 1.5],
    ["-48px -1px", 2],
    // A gallery nobody has voted on.
    ["-80px -1px", 0],
  ];
  for (const [position, rating] of table) {
    assertEquals(parseRating(`background-position:${position};opacity:1`), rating, position);
  }
});

function stubHost(gmetadata: unknown[]) {
  const g = globalThis as unknown as Record<string, unknown>;
  g.legacyCompat = { getLogger: () => ({ warn() {}, debug() {}, info() {} }) };
  const original = globalThis.fetch;
  globalThis.fetch = (() =>
    Promise.resolve(new Response(JSON.stringify({ gmetadata })))) as typeof fetch;
  return () => {
    globalThis.fetch = original;
  };
}

Deno.test("refreshSources takes gdata's fields and reads 0.00 as not rated", async () => {
  const restore = stubHost([
    {
      gid: 1001,
      title: "Main &amp; Title &#039;A&#039;",
      title_jpn: "原題 &amp; B",
      posted: "1700000000",
      uploader: "someone",
      filesize: 12345,
      filecount: "20",
      rating: "0.00",
      category: "Doujinshi",
      tags: ["language:chinese", 7, "artist:placeholder"],
      expunged: true,
    },
    {
      gid: 1002,
      title: "Second",
      posted: "1700000100",
      rating: "4.01",
      tags: ["female:example"],
    },
  ]);
  try {
    const result = await refreshSources({
      sources: [
        "https://e-hentai.org/g/1001/0123456789/",
        "exhentai.org/g/1002/abcdef0123",
        // Not a gallery URL: skipped rather than sent to the API.
        "e-hentai.org/tag/foo",
      ],
    });
    const [first, second] = result.candidates ?? [];
    assertEquals(result.candidates?.length, 2, "only gallery URLs are refreshed");
    assertEquals(first.source, "e-hentai.org/g/1001/0123456789");
    assertEquals(first.title, { en: "Main & Title 'A'", ja: "原題 & B" }, "entities decoded");
    assertEquals(first.rating, undefined, "no votes is not a 0 rating");
    assertEquals(first.tags, ["language:chinese", "artist:placeholder"], "non-string tags dropped");
    assertEquals(first.pages, 20);
    assertEquals(first.size, 12345);
    assertEquals(first.category, "Doujinshi");
    assertEquals(first.posted_at, "1700000000");
    assertEquals(second.rating, 4.01, "the exact average, not a half-star step");
  } finally {
    restore();
  }
});
