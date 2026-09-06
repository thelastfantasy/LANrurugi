// Generates src/i18n/emoji-names-zh.json: a flat `{ [emoji]: chineseName }` map covering every
// emoji + group name unicode-emoji-json's own data-by-group.json exposes in the stamp icon picker
// (IconPicker in dialog.tsx), sourced from Unicode's own CLDR short-name ("tts") annotations —
// authoritative official localization data, not machine translation. Two CLDR packages are
// needed: `cldr-annotations-full` covers most emoji, `cldr-annotations-derived-full` covers flags
// (CLDR mechanically derives flag names from territory names rather than annotating each flag
// directly) — verified empirically that the union of both gives 100% coverage of the 1914 emoji
// unicode-emoji-json ships, where either alone leaves gaps (326 missing using only the base
// package, almost all flags).
//
// Run manually with `node scripts/generate-emoji-zh-names.mjs` after either upstream dataset
// updates — output is checked in as a static asset, not regenerated at build time, since neither
// CLDR package is a runtime dependency of the app itself.

import { writeFileSync } from 'node:fs'
import { createRequire } from 'node:module'
import { fileURLToPath } from 'node:url'

const require = createRequire(import.meta.url)

const emojiGroups = require('unicode-emoji-json/data-by-group.json')
const base = require('cldr-annotations-full/annotations/zh/annotations.json').annotations.annotations
const derived = require('cldr-annotations-derived-full/annotationsDerived/zh/annotations.json').annotationsDerived.annotations

// CLDR's own annotation keys drop the U+FE0F variation selector that unicode-emoji-json's `emoji`
// field always includes (e.g. base heart `❤` vs. `❤️`) — normalizing both sides to the same
// stripped form is what makes the lookup below actually hit.
const stripVariationSelector = (s) => s.replace(/️/g, '')

// Derived-first, base-second (object spread lets a later assignment win) — annotationsDerived is
// specifically the *fallback* tier in CLDR's own model (mechanically-derived names for sequences
// the main annotations file doesn't cover directly), so a real, more specific `annotations` entry
// should always win over it when both exist for the same emoji.
const merged = {}
for (const [key, value] of Object.entries(derived)) merged[stripVariationSelector(key)] = value
for (const [key, value] of Object.entries(base)) merged[stripVariationSelector(key)] = value

const GROUP_NAME_ZH = {
  smileys_emotion: '表情与情感',
  people_body: '人物与身体',
  animals_nature: '动物与自然',
  food_drink: '食物与饮品',
  travel_places: '旅行与地点',
  activities: '活动',
  objects: '物品',
  symbols: '符号',
  flags: '旗帜',
}

const emojiNames = {}
const missing = []
for (const group of emojiGroups) {
  for (const entry of group.emojis) {
    const ann = merged[stripVariationSelector(entry.emoji)]
    const name = ann?.tts?.[0]
    if (!name) {
      missing.push(`${entry.emoji} ${entry.name}`)
      continue
    }
    emojiNames[entry.emoji] = name
  }
}

if (missing.length > 0) {
  console.error(`Missing CLDR zh names for ${missing.length} emoji:`)
  console.error(missing.join('\n'))
  process.exit(1)
}

const groupNames = {}
for (const group of emojiGroups) {
  const name = GROUP_NAME_ZH[group.slug]
  if (!name) {
    console.error(`No zh name mapped for emoji group slug "${group.slug}" — add one to GROUP_NAME_ZH.`)
    process.exit(1)
  }
  groupNames[group.slug] = name
}

const outPath = fileURLToPath(new URL('../src/i18n/emoji-names-zh.json', import.meta.url))
writeFileSync(outPath, JSON.stringify({ groups: groupNames, emojis: emojiNames }, null, 2) + '\n')
console.log(`Wrote ${Object.keys(emojiNames).length} emoji names + ${Object.keys(groupNames).length} group names to ${outPath}`)
