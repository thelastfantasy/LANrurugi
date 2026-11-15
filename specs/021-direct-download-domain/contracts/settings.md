# Contract: Settings validation — Direct Download Domain

## `POST /api/settings`

Existing endpoint, unchanged request/response shape except for the new rejection below.

### Accepted

```json
{ "direct_download_url": "https://fs.example.org" }
```

`200`; the value is stored verbatim (trailing slash removed).

### Rejected — scheme mismatch

```json
{ "direct_download_url": "http://fs.example.org" }
```

```json
{
  "error": "The archive download address must use https because the public address is https. Browsers block insecure downloads from an https page; use https on the download host too, or leave this empty.",
  "field": "direct_download_url",
  "code": "download_url_scheme_mismatch"
}
```

`400`; nothing is stored. The message must name the mismatch and the fix (the operator's next question is always "why
not, it is my LAN").

### Rejected — shape

| Input | Code |
|---|---|
| `fs.example.org` (no scheme) | `download_url_shape` |
| `ftp://fs.example.org` | `download_url_shape` |
| `https://user:pass@fs.example.org` | `download_url_credentials` |
| `https://fs.example.org/?a=1` | `download_url_query` |
| `https://fs.example.org/#x` | `download_url_fragment` |

### Warning on page-origin change

```json
{ "share_public_url": "https://lrrg.example.com" }
```

```json
{ "ok": true, "warnings": [ { "field": "direct_download_url", "code": "download_url_scheme_mismatch" } ] }
```

`200`; the page URL is stored, the download value is kept, and the UI shows the warning next to the field.

## Host policy (unchanged, restated as a contract)

For a request whose `Host` is the download host:

| Path prefix | Result |
|---|---|
| `/api/dl/` | served (signed links) |
| `/api/share/r/` | served, with CORS and `Access-Control-Expose-Headers: x-lrr-*` |
| anything else | `404`, for every method, before auth |

`CF-Connecting-IP` is never trusted on the download host; on the page host it is trusted only when the peer is a
Cloudflare edge address.

## Frontend contract

- The settings field shows the server's error message under the input and keeps the previous value.
- The message is translated (en/zh/ja) but the server's code is what the UI keys on; the raw English message is the
  fallback.
