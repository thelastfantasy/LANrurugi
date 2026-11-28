#!/usr/bin/env node
// Seed data for the uncommon-but-real states a fresh instance never shows (#286): a subscription
// with automatic download off and its approval list (titles by language, origin-only, none),
// reservations in both states, blocked works, and a check history whose candidates carry every
// verdict. All
// synthetic — no real titles, uploaders or gallery URLs.
//
// Used two ways: `mise run dev-seed` writes it into the dev container, and the smoke e2e
// (`tests/e2e/smoke.spec.ts`) writes it into its worker's Redis. Both go through `seedCommands`, so
// a new state added here reaches both. Every subscription is `disabled`, so the scheduler never
// contacts a source for seed data.
//
//   node tests/seed/seed.mjs --container lrr-dev --port 16379   (dev container)
//   node tests/seed/seed.mjs --port 6390                          (a local redis-server)

import { execFileSync } from "node:child_process"
import { pathToFileURL } from "node:url"

/** The config logical database (`RedisDbs`' `DB_CONFIG`), where every record below lives. */
const DB_CONFIG = "2"

/** The archive logical database, where the recycle bin's entries live. */
const DB_ARCHIVE = "0"

const SUB_MANUAL = "seed0000-0000-4000-8000-subscription1"
const SUB_AUTO = "seed0000-0000-4000-8000-subscription2"
/** History rows for works that were accepted and then deleted (see `historyCandidates`). */
const SUB_HISTORY = "seed0000-0000-4000-8000-subscription3"
const now = () => Math.floor(Date.now() / 1000)

function subscription(id, name, autoDownload) {
  return {
    id,
    name,
    source: "discovery/ehentai",
    criteria: { listing_url: "https://example.invalid/?f_search=seed" },
    scan: { mode: "pages", pages: 1 },
    filters: { condition: { kind: "all", children: [] } },
    interval_secs: 3600,
    enrich_metadata: true,
    auto_download: autoDownload,
    credit_policy: "pause",
    on_source_changed: "notify_only",
    on_source_removed: "mark_only",
    state: { state: "disabled" },
    last_checked_at: now() - 600,
    created_at: now() - 86_400,
  }
}

function pending(n, title) {
  return {
    id: `seed0000-0000-4000-8000-pending0000${n}`,
    subscription_id: SUB_MANUAL,
    source_url: `example.invalid/g/${1000 + n}/seed`,
    ...(title ? { title } : {}),
    created_at: now() - n * 60,
  }
}

function reservation(n, status, reason) {
  return {
    id: `seed0000-0000-4000-8000-reservation${n}`,
    subscription_id: SUB_MANUAL,
    source_url: `example.invalid/g/${2000 + n}/seed`,
    reason,
    status,
    created_at: now() - n * 60,
  }
}

function candidate(n, verdict, extra = {}) {
  return {
    source_url: `example.invalid/g/${3000 + n}/seed`,
    title: { ja: `シード作品 ${n}`, origin: `Seed work ${n}` },
    uploader: "seed-uploader",
    rating: 3.5,
    posted_at: now() - n * 3600,
    tags: ["language:chinese", "other:seed"],
    verdict,
    ...extra,
  }
}

/** A work whose accepted decision is no longer described by the download queue: it either arrived
 *  (`held`), or was thrown away from the queue, or was deleted from the library afterwards. The
 *  cycle keeps saying "queued", which is exactly the stale state the history view has to correct. */
function historyCandidate(n, source) {
  return candidate(n, { verdict: "queued" }, { source_url: source })
}

/** A recycle-bin entry for `source`, whose only purpose is the history's "deleted" join: it has no
 *  payload on disk, so restoring it reports a missing payload — nothing here is restorable. */
function recycledEntry(id, title, source) {
  return {
    schema: 1,
    id,
    title,
    name: title,
    original_path: `/home/koyomi/lanrurugi/content/${id.slice(0, 2)}/${id}.zip`,
    file_name: `${id}.zip`,
    size: 1024,
    page_count: 1,
    deleted_at: now() - 120,
    deleted_by: { kind: "session", label: "session" },
    snapshot: {
      tags: `source:${source},language:chinese,other:seed`,
      summary: "",
      isnew: false,
      lastreadpage: 0,
      lastreadtime: 0,
      primary_source: source,
    },
  }
}

/** Every write, as `[db, ...redis-cli args]`. */
export function seedCommands() {
  const pendings = [
    pending(1, { ja: "多言語タイトルのシード作品", origin: "Seed work with titles by language" }),
    pending(2, { origin: "Seed work with an origin title only" }),
    pending(3, null),
    pending(4, { origin: "A".repeat(240) }),
  ]
  const reservations = [
    reservation(1, "waiting", "download_failed:seed: no installed download plugin matches"),
    reservation(2, "discarded", "insufficient_credit"),
  ]
  const cycle = {
    id: "seed0000-0000-4000-8000-cycle0000001",
    subscription_id: SUB_MANUAL,
    started_at: now() - 660,
    finished_at: now() - 600,
    outcome: "completed",
    candidates_seen: 6,
    candidates: [
      candidate(1, { verdict: "queued" }),
      candidate(2, { verdict: "awaiting_approval" }),
      candidate(3, { verdict: "reserved", reason: "insufficient_credit" }),
      candidate(4, { verdict: "rejected", rule: "rating" }),
      candidate(5, { verdict: "already_seen" }),
      candidate(6, { verdict: "queued" }, { title: { origin: "Seed work with an origin title only" } }),
    ],
  }
  const commands = []
  for (const s of [
    subscription(SUB_MANUAL, "Seed: approval required", false),
    subscription(SUB_AUTO, "Seed: automatic download", true),
  ]) {
    commands.push([DB_CONFIG, "SET", `LANRURUGI_SUBSCRIPTION_${s.id}`, JSON.stringify(s)])
    commands.push([DB_CONFIG, "SADD", "LANRURUGI_SUBSCRIPTIONS", s.id])
  }
  for (const p of pendings) {
    commands.push([DB_CONFIG, "SET", `LANRURUGI_SUBSCRIPTION_PENDING_${p.id}`, JSON.stringify(p)])
    commands.push([DB_CONFIG, "SADD", "LANRURUGI_SUBSCRIPTION_PENDING", p.id])
  }
  for (const r of reservations) {
    commands.push([DB_CONFIG, "SET", `LANRURUGI_RESERVATION_${r.id}`, JSON.stringify(r)])
    commands.push([DB_CONFIG, "SADD", "LANRURUGI_RESERVATIONS", r.id])
  }
  // Blocked works ("never download this"): one that the check history still describes, so the
  // blocked tab shows its title, and others known only by URL, in both subscriptions.
  const blocked = [
    [SUB_MANUAL, "example.invalid/g/3004/seed", 3],
    [SUB_MANUAL, "example.invalid/g/4001/seed", 2],
    [SUB_AUTO, "example.invalid/g/4002/seed", 1],
  ]
  for (const [sub, source, hoursAgo] of blocked) {
    commands.push([DB_CONFIG, "SADD", `LANRURUGI_SUBSCRIPTION_SEEN_${sub}`, source])
    commands.push([
      DB_CONFIG,
      "HSET",
      `LANRURUGI_SUBSCRIPTION_BLOCKED_${sub}`,
      source,
      String(Date.now() - hoursAgo * 3_600_000),
    ])
  }
  // A work skipped from the approval list: the cycle still says "awaiting approval", and the
  // history has to read the skip record to call it skipped.
  commands.push([DB_CONFIG, "SADD", `LANRURUGI_SUBSCRIPTION_SEEN_${SUB_MANUAL}`, "example.invalid/g/3002/seed"])
  commands.push([
    DB_CONFIG,
    "HSET",
    `LANRURUGI_SUBSCRIPTION_SKIPPED_${SUB_MANUAL}`,
    "example.invalid/g/3002/seed",
    JSON.stringify({ at: Date.now() - 1_800_000, title: { ja: "シード作品 2", origin: "Seed work 2" } }),
  ])
  commands.push([DB_CONFIG, "DEL", `LANRURUGI_SUBSCRIPTION_CYCLES_${SUB_MANUAL}`])
  commands.push([DB_CONFIG, "LPUSH", `LANRURUGI_SUBSCRIPTION_CYCLES_${SUB_MANUAL}`, JSON.stringify(cycle)])

  // Works that were accepted and then deleted — the two states the history cannot get from the
  // download queue at all: a row deleted from the queue (nothing left but the subscription's own
  // delete record) and an archive deleted from the library (still in the recycle bin).
  const recycledSource = "example.invalid/g/5001/seed"
  const queueDeletedSource = "example.invalid/g/5002/seed"
  const historyCycle = {
    id: "seed0000-0000-4000-8000-cycle0000002",
    subscription_id: SUB_HISTORY,
    started_at: now() - 720,
    finished_at: now() - 700,
    outcome: "completed",
    candidates_seen: 2,
    candidates: [
      historyCandidate(10, recycledSource),
      historyCandidate(11, queueDeletedSource),
    ],
  }
  const history = subscription(SUB_HISTORY, "Seed: deleted works", false)
  commands.push([DB_CONFIG, "SET", `LANRURUGI_SUBSCRIPTION_${history.id}`, JSON.stringify(history)])
  commands.push([DB_CONFIG, "SADD", "LANRURUGI_SUBSCRIPTIONS", history.id])
  // The work was delivered, so it is seen; the decision stays "queued" in the cycle.
  for (const source of [recycledSource, queueDeletedSource]) {
    commands.push([DB_CONFIG, "SADD", `LANRURUGI_SUBSCRIPTION_SEEN_${SUB_HISTORY}`, source])
  }
  commands.push([DB_CONFIG, "DEL", `LANRURUGI_SUBSCRIPTION_CYCLES_${SUB_HISTORY}`])
  commands.push([
    DB_CONFIG,
    "LPUSH",
    `LANRURUGI_SUBSCRIPTION_CYCLES_${SUB_HISTORY}`,
    JSON.stringify(historyCycle),
  ])
  commands.push([
    DB_ARCHIVE,
    "HSET",
    "LANRURUGI_RECYCLE_BIN",
    "5eed000000000000000000000000000000000001",
    JSON.stringify(recycledEntry("5eed000000000000000000000000000000000001", "Seed work 10", recycledSource)),
  ])
  commands.push([
    DB_CONFIG,
    "HSET",
    `LANRURUGI_SUBSCRIPTION_DELETED_${SUB_HISTORY}`,
    queueDeletedSource,
    String(Date.now() - 120_000),
  ])
  return commands
}

/** Runs every seed write through `redis-cli`, optionally inside a container. */
export function seed({ port, container }) {
  for (const [db, ...args] of seedCommands()) {
    const cli = ["redis-cli", "-p", String(port), "-n", db, ...args]
    // An argument array, never a shell string: the JSON values carry quotes and spaces.
    if (container) execFileSync("podman", ["exec", container, ...cli], { stdio: "ignore" })
    else execFileSync(cli[0], cli.slice(1), { stdio: "ignore" })
  }
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const arg = (name) => {
    const i = process.argv.indexOf(name)
    return i === -1 ? undefined : process.argv[i + 1]
  }
  const port = arg("--port") ?? "6379"
  seed({ port, container: arg("--container") })
  console.log(`seeded ${seedCommands().length} writes into redis port ${port}`)
}
