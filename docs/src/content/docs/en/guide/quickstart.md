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

LANrurugi has its own database and neither reads nor changes LANraragi's, so the two can run side
by side. The steps are under "Moving from LANraragi" on the [install](../install/) page: mount the same
manga directory as the library, give thumbnails and the database new directories, then import
LANraragi's backup JSON in the first-run wizard.

The import pairs archives in the backup with the ones LANrurugi scanned by file name, never by ID:
LANrurugi's IDs are a size-aware fingerprint (`hash(first 512KB ++ big-endian u64 file size)`), unlike
LANraragi's `SHA-1(first 512KB)`. When several files share a name nothing is guessed; they count as
ambiguous. Let the library scan finish before importing — archives not scanned yet count as skipped
with no match.
Different files that LANraragi wrongly merged as duplicates because their leading bytes matched show up
separately in LANrurugi.

## 6. Try the API

Open the [API reference](../../api/) and, in the request panel, set the server address to your own
instance (it already defaults to `http://localhost:3000`), fill in a token, and send the request.
Requests go from your browser straight to your instance, never through this project.

When the browser will not allow it (for example, this documentation is served over HTTPS while your
instance is plain HTTP on a LAN address), every endpoint also carries a command line you can copy.

Details are in [Testing the API from these docs](../api-testing/).
