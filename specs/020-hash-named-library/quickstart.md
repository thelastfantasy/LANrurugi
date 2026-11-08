# Quickstart — Hash-Named Library

Runnable scenarios that prove the feature end to end. Run them against the dev instance (`mise run dev-up`, port 3000)
with a scratch library directory, never against real data.

## Prerequisites

```sh
mise run dev-up
export LRR=http://127.0.0.1:3000
export LIB=/tmp/lrr-hash-lib
rm -rf "$LIB" && mkdir -p "$LIB"
# point the dev instance's archive directory at $LIB (settings → paths, or LANRURUGI_ARCHIVE_DIR before start)
```

## S1 — A 300-byte title ingests

```sh
python3 - <<'PY'
name = "テスト" * 100            # 300 characters, 900 UTF-8 bytes
open("/tmp/big-title.zip", "wb").write(open("/tmp/fixture.zip", "rb").read())
PY
curl -sS -F 'file=@/tmp/big-title.zip;filename=テスト…zip' "$LRR/api/upload"
```

Expected: `200`; the archive exists; `redis-cli HGETALL archive:<id>` shows `title` and `original_filename` with the
long name; `file` is `<LIB>/<ab>/<id>.zip`; `ls "$LIB"/<ab>/` shows the zip and its `.json`.

Failure modes to check: no `File name too long` anywhere in the server log; the queue never shows a conflict.

## S2 — A 255-byte boundary is irrelevant

```sh
# 255 bytes of filename, then 256, then 1024: all must ingest and open.
```

Expected: identical outcome for all three; the stored basename is always `{id}.{ext}`.

## S3 — Two identical files, different names

Upload the same fixture twice under different filenames.

Expected: the second upload is rejected as a duplicate naming the first archive's id; no second record; no overwrite.

## S4 — Hand-dropped file

```sh
cp /tmp/fixture.zip "$LIB/series/new one.zip"
curl -sS -X POST "$LRR/api/scan"     # or wait for the watcher
```

Expected: catalogued; `file` still `$LIB/series/new one.zip`; `original_filename` = `new one.zip`; a sidecar is written
next to it; the file is not renamed.

## S5 — Migration on a scratch library

```sh
# 1. Put three archives in subdirectories, scan, note their ids.
curl -sS "$LRR/api/archives" | jq -r '.archives[] | [.id, .file] | @tsv' > /tmp/before.tsv

# 2. Preview.
curl -sS "$LRR/api/migration/preview" | jq .

# 3. Run.
curl -sS -X POST "$LRR/api/migration" -H 'content-type: application/json' \
  -d '{"confirm":true}' | jq .

# 4. After completion, the ids are unchanged and paths are under .classic/.
curl -sS "$LRR/api/archives" | jq -r '.archives[] | [.id, .file] | @tsv' > /tmp/after.tsv
diff <(cut -f1 /tmp/before.tsv) <(cut -f1 /tmp/after.tsv)   # empty
grep -v '/.classic/' /tmp/after.tsv                          # empty
```

Expected: preview counts add up to the archive count; `diff` empty; every path starts with `.classic/<ab>/`; each
archive has a sidecar; category and tankoubon counts (from `/api/categories`) are unchanged.

## S6 — Interrupt and resume

Start S5 with a larger library (say 1,000 files), `kill -9` the server mid-run, restart, then start the migration
again.

Expected: it resumes (no file moved twice, none lost), and the journal has one line per completed move.

## S7 — Rollback

```sh
curl -sS -X POST "$LRR/api/migration/rollback" | jq .
```

Expected: every file returns to its pre-migration path; `diff` against `/tmp/before.tsv` is empty; the library still
resolves.

## S8 — Database loss

```sh
# On a scratch Redis only.
redis-cli -n 0 FLUSHDB
mise run rebuild-index           # or POST /api/rebuild
curl -sS "$LRR/api/archives" | jq '.archives | length'
```

Expected: the count matches the pre-flush snapshot; ids and titles are restored from the files and sidecars; the report
shows zero unreadable sidecars.

## S9 — The conflict UI is gone

- `grep -rn "PendingFilenameConflict\|renameArchive\|FilenameTemplateEditor" crates apps/frontend/src` → no hits.
- Open the upload queue, download two archives whose titles are identical: both finish, no menu, no dialog.
- Open an archive's edit page: title editable, no filename field, no rename action.
- `curl -sS -X PUT "$LRR/api/archives/<id>/rename"` → 404.

## S10 — Downloads keep the title

Download an archive after migration and inspect the response headers.

Expected: `Content-Disposition` carries the title, not the id; the bytes match.
