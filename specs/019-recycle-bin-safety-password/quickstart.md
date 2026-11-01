# Quickstart: Recycle Bin and Safety Password

Runnable checks for the scenarios in [spec.md](./spec.md). They assume a working instance with at least one
archive in the library. Commands use:

```sh
BASE=${BASE:-http://127.0.0.1:3000}
REDIS=${REDIS:-redis://127.0.0.1:6379}
COOKIE=/tmp/lrr.cookies
```

Sign in first (the login form, not JSON):

```sh
curl -sc "$COOKIE" -X POST "$BASE/api/login" -d 'password=kamimamita'
```

## 1. Delete, find it in the bin, restore it with its metadata (US1)

```sh
ID=<archive id>; ls -l "<library>/<path of that archive>"

# Give it something to lose: title, tag, category, a bookmark at page 3, a reading position.
curl -sb "$COOKIE" -X PUT "$BASE/api/archives/$ID/metadata" \
  -H 'content-type: application/json' \
  -d '{"title":"Recycle smoke test","tags":"recycle:smoke"}'
curl -sb "$COOKIE" -X POST "$BASE/api/archives/$ID/bookmarks/3"
curl -sb "$COOKIE" -X PUT "$BASE/api/archives/$ID/progress/5"

curl -sb "$COOKIE" -X DELETE "$BASE/api/archives/$ID"
# → {"operation":"delete_archive","id":"…","filename":"…","success":1,"recycled":true}

ls "<library>/.lanrurugi-recycle/$ID/payload/"        # the file is here, same inode-ish rename
curl -sb "$COOKIE" "$BASE/api/recycle-bin" | jq '.entries[0], .usage'
curl -sb "$COOKIE" -X POST "$BASE/api/recycle-bin/$ID/restore" | jq
# → {"operation":"restore_archive","success":1,"id":"…","path":"<original path>","renamed":false}

curl -sb "$COOKIE" "$BASE/api/archives/$ID/metadata" | jq '.title, .tags, .progress'
curl -sb "$COOKIE" "$BASE/api/archives/$ID/bookmarks" | jq '.[0].page'
```

Expected: the metadata, the tag, the bookmark and the progress are all back; the original path is reported; the
entry is no longer in `/api/recycle-bin`.

## 2. A full rescan never re-catalogues a recycled archive (SC-003)

```sh
for i in $(seq 1 20); do curl -sb "$COOKIE" -X DELETE "$BASE/api/archives/$(…)"; done   # or batch-delete
curl -sb "$COOKIE" -X POST "$BASE/api/shinobu/rescan"
sleep 5
curl -sb "$COOKIE" "$BASE/api/recycle-bin" | jq '.usage.count'   # still 20
curl -sb "$COOKIE" "$BASE/api/search?q=recycle" | jq '.data | length'   # 0
```

Also check the live watcher: while the server runs, `mv` an archive into the bin directory by hand and confirm no
new archive appears in the library (the exclusion lives in `is_watched_archive_path`, so the watcher, the rescan
and the subfolder scripts all share it).

## 3. Safety password: setup, wrong guesses, lockout, correct PIN (US2)

```sh
# With no safety password configured, emptying the bin must be refused with a setup prompt.
curl -sb "$COOKIE" -X DELETE "$BASE/api/recycle-bin" -H 'content-type: application/json' -d '{}' | jq
# → 403 {"error":"safety_password_not_set", …}

# Set one (four ASCII digits).
curl -sb "$COOKIE" -X POST "$BASE/api/safety-password" -H 'content-type: application/json' \
  -d '{"new_password":"1234"}' | jq
# → {"configured":true,"locked":false,"locked_until":null,"attempts_remaining":5}

# Five wrong guesses lock verification; even the correct digits are then refused.
for i in 1 2 3 4 5; do
  curl -sb "$COOKIE" -o /dev/null -w '%{http_code}\n' -X DELETE "$BASE/api/recycle-bin/<id>" \
    -H 'content-type: application/json' -d '{"safety_password":"0000"}'
done
curl -sb "$COOKIE" -X DELETE "$BASE/api/recycle-bin/<id>" \
  -H 'content-type: application/json' -d '{"safety_password":"1234"}' | jq
# → 429 {"error":"safety_password_locked","locked_until":…}

# Simulate the five minutes elapsing without waiting (test-only):
redis-cli -u "$REDIS" -n 2 DEL "LANRURUGI_SAFETY_LOCK:<family>"   # config DB = 2
curl -sb "$COOKIE" -X DELETE "$BASE/api/recycle-bin" -H 'content-type: application/json' \
  -d '{"safety_password":"1234"}' | jq
# → {"operation":"empty_recycle_bin","success":1,"deleted":N,"failed":[]}
```

Expected after a real empty: `<library>/.lanrurugi-recycle` contains no entry directories, and the archives'
thumbnails are gone from `thumb_dir`.

## 4. Reset paths (US2)

Settings page (login password):

```sh
curl -sb "$COOKIE" -X POST "$BASE/api/safety-password/reset" -H 'content-type: application/json' \
  -d '{"login_password":"kamimamita"}'
# → {"configured":false, …}; the next permanent delete asks for setup again.
```

CLI (no login required):

```sh
mise run dev-up   # or run the binary directly
docker exec <container> lanrurugi reset-safety-password --redis-url "$REDIS"
# → "safety password cleared; set a new one in Settings"
```

## 5. Retention and capacity sweep (US3)

```sh
# Keep one day, unlimited size; then backdate the entry so the next sweep removes it.
curl -sb "$COOKIE" -X PUT "$BASE/api/settings" -H 'content-type: application/json' \
  -d '{"recycle_retention_days":1}'
# Test-only: rewrite deleted_at through Redis (the entry is a JSON hash field).
redis-cli -u "$REDIS" -n 0 HGET LANRURUGI_RECYCLE_BIN <id>   # archive DB = 0
# …edit deleted_at to now - 2*86400 and HSET it back…
# Restart the server: startup reconciliation runs a sweep immediately.
```

Expected: the entry and its payload are gone from disk and from `/api/recycle-bin`, and the activity log shows a
`recycle.sweep` entry with `reason: "expired"` and a `System` actor. For the cap:

```sh
curl -sb "$COOKIE" -X PUT "$BASE/api/settings" -H 'content-type: application/json' \
  -d '{"recycle_max_size_mb":1}'     # bin now over cap
# The settings write triggers an immediate sweep: oldest entries go, the newest one survives
# even when it alone exceeds 1 MiB.
curl -sb "$COOKIE" "$BASE/api/recycle-bin" | jq '.usage'
```

## 6. Authorization boundaries (US2/FR-027)

```sh
TOKEN=<an admin API token>
curl -s -H "Authorization: Bearer $TOKEN" "$BASE/api/recycle-bin" | jq
# → 403; the same for /api/safety-password* and for the destructive verbs.
```

With guest mode on, open `/recycle-bin` in a private window: the nav entry is absent and the route does not
render the bin.

## 7. Crash recovery and leftovers (FR-013)

```sh
mise run dev-down        # stop the server between steps
# Simulate a crash after the rename but before the Redis write: move a file into a new entry directory
# (no meta.json), keeping its archive record in Redis.
mkdir -p "<library>/.lanrurugi-recycle/<id>/payload" && mv "<library>/file.cbz" "<library>/.lanrurugi-recycle/<id>/payload/"
mise run dev-up
curl -sb "$COOKIE" "$BASE/api/recycle-bin" | jq '.entries[0]'
# → the entry exists (reconciliation rebuilt meta.json from the record and finished the delete)
```

An unknown leftover with no record and no `meta.json` shows up in `leftovers` and is removed only after the 24 h
grace period.

## 8. Contract and UI test gates

```sh
mise run check-crate -- lanrurugi-storage lanrurugi-api lanrurugi-scanner
mise run test-crate -- lanrurugi-storage lanrurugi-api
pnpm --dir apps/frontend test:unit
pnpm --dir apps/frontend test:e2e -- --grep "recycle"
mise run check     # rust-check + frontend-lint, the pre-push gate
```

The docgen gate is part of `mise run check`'s Rust half: a new route without a sidecar entry fails it, so
`docs/api/sidecar/recycle-bin.yaml` is part of this feature, not an afterthought.
