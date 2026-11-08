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

## S4 — A dropped file is relocated into `_byname`

```sh
mkdir -p "$LIB/series"
cp /tmp/fixture.zip "$LIB/series/new one.zip"
curl -sS -X POST "$LRR/api/scan"     # or wait for the watcher
```

Expected: catalogued; `file` becomes `$LIB/_byname/series/new one.zip`; `original_path` = `series/new one.zip`;
`original_filename` = `new one.zip`; no sidecar is written (it is not in the hash layout); the filename is unchanged.
The scan response reports `moved_to_byname: 1`.

## S5 — An existing library moves on its first scan

```sh
# 1. Put three archives in subdirectories, scan, note their ids.
curl -sS "$LRR/api/archives" | jq -r '.archives[] | [.id, .file] | @tsv' > /tmp/before.tsv

# 2. Trigger a scan; there is no preview or confirm step.
curl -sS -X POST "$LRR/api/scan" | jq .

# 3. Ids are unchanged and paths are under _byname/.
curl -sS "$LRR/api/archives" | jq -r '.archives[] | [.id, .file] | @tsv' > /tmp/after.tsv
diff <(cut -f1 /tmp/before.tsv) <(cut -f1 /tmp/after.tsv)   # empty
grep -v '/_byname/' /tmp/after.tsv                          # empty
```

Expected: `diff` empty; every path is `<LIB>/_byname/<original relative path>` with its filename unchanged; no sidecar
was added under `_byname`; category and tankoubon counts (from `/api/categories`) are unchanged; a second scan reports
`moved_to_byname: 0`.

## S6 — Interrupt and resume

Start S5 with a larger library (say 1,000 files), `kill -9` the server mid-scan, restart, scan again.

Expected: files already under `_byname/` are untouched, the rest move, no file is moved twice and none is lost.

## S7 — Move one file back by hand

```sh
mv "$LIB/_byname/series/vol1.zip" "$LIB/series/vol1.zip"
curl -sS -X POST "$LRR/api/scan" | jq .
```

Expected: the file is catalogued at its new path with the same id (no duplicate), then relocated into `_byname/`
again, because the rule is continuous.

## S8 — Database loss

```sh
# On a scratch Redis only.
redis-cli -n 0 FLUSHDB
mise run rebuild-index           # or POST /api/rebuild
curl -sS "$LRR/api/archives" | jq '.archives | length'
```

Expected: the count matches the pre-flush snapshot; hash-layout ids and titles are restored from the files and their
sidecars, `_byname` titles from the filenames; the report shows zero unreadable sidecars.

## S9 — The conflict UI is gone

- `grep -rn "PendingFilenameConflict\|renameArchive\|FilenameTemplateEditor" crates apps/frontend/src` → no hits.
- Open the upload queue, download two archives whose titles are identical: both finish, no menu, no dialog.
- Open an archive's edit page: title editable, no filename field, no rename action.
- `curl -sS -X PUT "$LRR/api/archives/<id>/rename"` → 404.

## S10 — Downloads keep the title

Download an archive after relocation and inspect the response headers.

Expected: `Content-Disposition` carries the title, not the id; the bytes match.
