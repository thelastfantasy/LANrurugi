# Quickstart — Direct Download Domain

Validation scenarios for the application change and for the shipped reverse-proxy reference.

## A. Validator (no proxy needed)

```sh
mise run dev-up
export LRR=http://127.0.0.1:3000

# 1. HTTP page, HTTP download: accepted (LAN default).
curl -sS -X POST "$LRR/api/settings" -H 'content-type: application/json' \
  -d '{"share_public_url":"http://192.168.1.10:3000","direct_download_url":"http://192.168.1.10:3001"}'

# 2. HTTPS page, HTTP download: rejected, nothing stored.
curl -sS -o /dev/null -w '%{http_code}\n' -X POST "$LRR/api/settings" -H 'content-type: application/json' \
  -d '{"share_public_url":"https://lrrg.example.com","direct_download_url":"http://fs.example.com"}'
# expect 400, and GET /api/settings still returns the previous value

# 3. HTTPS page, HTTPS download: accepted.
# 4. Shape errors: fs.example.org, ftp://, user:pass@, ?x=1, #frag -> 400 with the matching code.
# 5. Change share_public_url to https with an http download value -> 200 + warning.
```

## B. Host policy (dev instance, no CDN)

The app enforces the boundary itself, so it can be checked without Cloudflare by sending the Host header:

```sh
for p in / /api/info /api/archives /api/settings /api/dl/x /api/share/r/x; do
  printf '%-20s ' "$p"
  curl -sS -o /dev/null -w '%{http_code}\n' -H 'Host: fs.example.org' "$LRR$p"
done
# expect: 404 404 404 404 <not 404> <not 404>
```

## C. Reverse-proxy reference on a scratch host

```sh
mkdir -p /tmp/lrr-proxy && cd /tmp/lrr-proxy
curl -sS -o lanrurugi-dual-domain.conf \
  http://127.0.0.1:3000/docs/nginx/lanrurugi-dual-domain.conf   # or copy from docs/public/nginx/
# Replace the two server_name values and the upstream address, then:
docker run --rm -v "$PWD:/etc/nginx/conf.d:ro" nginx:stable nginx -t
```

Expected: `syntax is ok` / `test is successful`.

Run it in front of the dev instance with two names mapped to 127.0.0.1 and check:

1. The page host refuses a client that is not a Cloudflare edge address (or, in the reference's commented-out
   local-testing form, the allow-list is explicitly documented as bypassed).
2. The download host serves `/api/dl/...` and returns 404 for `/`, `/api/info`, `/api/archives`.
3. A forged `CF-Connecting-IP` on the download host does not appear as the client address in the app's logs.
4. A large transfer through the download host does not create a spool file under nginx's `proxy_temp_path`.

## D. Documentation

```sh
mise run docs-build
grep -r 'few GB\|等不到首字节\|too large' docs/dist | head    # expect no stale claim
test -f docs/dist/nginx/lanrurugi-dual-domain.conf && echo ok
```

Also open `/guide/sharing/` and `/en/guide/sharing/` and follow the steps as a reader: every step needed for a working
deployment is present, including A/AAAA rather than CNAME, IPv6 parity, the DNS-01 certificate note and the CGNAT
limitation.
