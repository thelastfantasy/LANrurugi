---
title: Quickstart
description: First login, creating an API token, scanning the library, importing existing data.
---

## 1. Log in

Open `http://<your instance address>:3000`, log in with the default password `kamimamita`, then
**change it in the settings page immediately**.

## 2. Point it at your library

`--library-path` (or `LANRURUGI_LIBRARY_PATH`) points at the directory holding your archives. It
defaults to `./library`; under the Compose deployment that corresponds to the container's
`/home/koyomi/lanrurugi/content`.

## 3. Scan

Trigger a full scan from the settings page. Scanning is concurrent (Rayon) and does **not** delete
archive records by itself — "a file disappeared during the scan" only cleans up path bookkeeping.
Removing archive records is the separate, explicitly-triggered database cleanup.

## 4. Create an API token

The request panels throughout the API reference need a token. Create one under token management in
the settings page, then:

```sh
curl -H "Authorization: Bearer <your token>" http://localhost:3000/api/archives
```

Tokens come in two roles, administrator and guest; a guest token can only read.

## 5. Migrating from LANraragi

There is no migration step. LANrurugi reads the data the old version wrote into Redis directly:
archive records, tags, categories, reading progress, thumbnails, Tankoubon groupings, settings. Point
it at your `data/database` directory and you are done.

Two things worth knowing:

- **The archive ID algorithm.** New scans use a size-aware fingerprint
  (`hash(first 512KB ++ big-endian u64 file size)`), while the old `SHA-1(first 512KB)` remains
  readable. The old algorithm treated different archives that happened to share their leading bytes as
  duplicates and merged them — a known defect this project fixes. The fix only applies to new
  content; existing IDs are never silently rewritten.
- **Recovering files hidden by a mistaken merge.** Run `lanrurugi rebuild-index`: it recomputes IDs
  with the new algorithm and re-exposes files that were historically hidden by a bad merge.

## 6. Try the API

Open the [API reference](../../api/) and, in the request panel, set the server address to your own
instance (it already defaults to `http://localhost:3000`), fill in a token, and send the request.
Requests go from your browser straight to your instance, never through this project.

When the browser will not allow it (for example, this documentation is served over HTTPS while your
instance is plain HTTP on a LAN address), every endpoint also carries a command line you can copy.

Details are in [Testing the API from these docs](../api-testing/).
