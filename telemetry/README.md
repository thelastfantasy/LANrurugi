# telemetry — the anonymous heartbeat sink

One instance of this serves **every** deployment of the project. It is not part of a normal
LANrurugi installation: a normal install neither runs this nor needs it, and it works fine without
ever contacting one.

## What it stores

Exactly three facts per installation per day, and nothing else:

| Fact | Shape | Why |
|---|---|---|
| how many | `LANRURUGI_TELEMETRY:DAY:<date>` — a Set of install ids | count distinct installations |
| where | `LANRURUGI_TELEMETRY:GEO:<date>:<cc>` — a HyperLogLog | count per country, 12 KB each, ~0.8% error |
| which version | `LANRURUGI_TELEMETRY:VER:<date>:<version>` — a Set | upgrade rate / version mix |
| which artifact | `LANRURUGI_TELEMETRY:CHAN:<date>:<channel>` — a Set | GHCR vs Docker Hub vs native vs a local build |

The install id is a random UUIDv4 created on first run. There is no library path, file name, user,
comment or request body in any key — and **no IP address is ever read**: the country comes from
Cloudflare's own `CF-IPCountry` header, so it is decided at the edge and never touches this process.

A client sends at most one request per day, 3-second timeout, never retried, never blocking
startup. Opting out is `LANRURUGI_NO_TELEMETRY=1` on the client side.

## Deploying it

This assumes the host already serves HTTPS through nginx (the existing `ix-nginx-certbot`
container runs with `network_mode: host`, which is why the sink only needs loopback). If it does
not, see the commented-out `cloudflared` service in `compose.yaml` instead.

### 1. Build the image

The `telemetry-ingest` subcommand is not in any released image yet, so build from this repository.
`Dockerfile.ingest` reuses the application Dockerfile's `rust-builder` stage (so the glibc matches
bookworm) and ships only the one binary — 255 MB instead of the full image's several GB:

```sh
podman build --target rust-builder -t lrr-telemetry-builder .
podman create --name lrr-tb lrr-telemetry-builder
# A build context outside the repository: this is a 126 MB binary, not a source file.
mkdir -p /tmp/lanrurugi-ingest
podman cp lrr-tb:/build/target/release/lanrurugi-server /tmp/lanrurugi-ingest/
podman rm lrr-tb
cp telemetry/Dockerfile.ingest /tmp/lanrurugi-ingest/
podman build -f /tmp/lanrurugi-ingest/Dockerfile.ingest \
  -t localhost/lanrurugi-telemetry:local /tmp/lanrurugi-ingest
```

The `localhost/` prefix is not optional: without it docker reads the name as a registry path and
tries to pull it.

To move it to another host: `podman save --format docker-archive … | gzip | ssh truenas 'gunzip |
sudo docker load'`.

### 2. Bring it up

Create a dataset for Redis data (e.g. `apps/lanrurugi-telemetry/redis`), copy `compose.yaml` and
`.env.example` into a directory on that pool, then:

```sh
cp .env.example .env      # fill REDIS_PASSWORD, LANRURUGI_IMAGE, REDIS_DATA, STATS_TOKEN
docker compose -p lanrurugi-telemetry up -d
```

The `-p` matters on TrueNAS: an explicit project name keeps this stack out of the `ix-*` namespace
its own apps use.

### 3. Point nginx at it

`/mnt/tank/app-settings/docker-nginx-certbot/user_conf.d/hb.lrrg.dev.conf`:

```nginx
server {
    listen 443 ssl;
    http2 on;
    server_name hb.lrrg.dev;

    ssl_certificate     /etc/nginx/ssl/cloudflare/lrrg.dev.pem;
    ssl_certificate_key /etc/nginx/ssl/cloudflare/lrrg.dev.key;

    # 1 KB is the sink's whole payload; the body cap is also enforced in the service.
    client_max_body_size 2k;

    location / {
        proxy_pass http://127.0.0.1:8099;
        proxy_set_header Host $host;
        # `CF-Connecting-IP` is set by Cloudflare at the edge and passes through unchanged; the
        # sink rate-limits on it, since every request arrives from this loopback address.
        proxy_set_header CF-Connecting-IP $http_cf_connecting_ip;
        proxy_set_header CF-IPCountry $http_cf_ipcountry;
    }
}
```

The `user_conf.d` entries are symlinked into `conf.d`, so **restart the nginx container** after
adding one — editing the file alone does not reload it.

Then add an `A` record for `hb`, proxied (orange cloud) so Cloudflare keeps supplying
`CF-IPCountry`.

### 4. Verify

```sh
# from anywhere on the internet
curl -i -X POST https://hb.example.com/v1/hb \
  -H 'content-type: application/json' \
  -d '{"id":"3f2a1c4e-0000-4000-8000-000000000000","version":"0.2.0","arch":"x86_64","os":"linux","channel":"ghcr"}'
# expect: 204

# your own curl arrives via this NAS's egress, so its country is the NAS's, not the client's —
# repeat from a phone on mobile data to see a different `GEO:<date>:<cc>` fill in

# on the NAS
docker compose -p lanrurugi-telemetry exec redis redis-cli -a "$REDIS_PASSWORD" SCARD "LANRURUGI_TELEMETRY:DAY:$(date +%F)"
docker compose -p lanrurugi-telemetry exec redis redis-cli -a "$REDIS_PASSWORD" PFCOUNT "LANRURUGI_TELEMETRY:GEO:$(date +%F):JP"

# the read side, once STATS_TOKEN is set
curl -s https://hb.example.com/stats -H "Authorization: Bearer $STATS_TOKEN"
```

A 204 whose country lands under `ZZ` means the write path works but the request never passed
through Cloudflare — with an orange-clouded record that should not happen; check that the proxied
`A` record exists and that nginx forwards `CF-IPCountry`.

### 5. Point clients at it

Nothing to configure: `lanrurugi_api::heartbeat` carries the project endpoint as a constant, so
every release build — image and native package alike — reports on its own. This page exists only so
the receiving end is documented; an operator who wants out sets:

```yaml
environment:
  LANRURUGI_NO_TELEMETRY: "1"   # or pass --no-telemetry
```

To aim a build somewhere else (a local sink, a staging one):

```yaml
environment:
  LANRURUGI_TELEMETRY_URL: https://hb.example.com/v1/hb
```

## Reporting

With `STATS_TOKEN` set, the sink serves a dashboard at `/` and its data at `/stats`. The dashboard
asks for that token as a password once, then keeps a signed session cookie (HttpOnly, Secure,
SameSite=Lax, 7 days) — so it works in a browser and still lets a password manager save it.

```sh
# JSON — DAU/WAU/MAU plus 30-day breakdowns by country, version and channel
curl -s https://hb.example.com/stats -H "Authorization: Bearer $STATS_TOKEN" | jq

# daily distinct installations for the chart (1–90 days)
curl -s "https://hb.example.com/api/series?days=30" -H "Authorization: Bearer $STATS_TOKEN"
```

The dashboard itself is `telemetry/ui` (Svelte + Vite). Build it with `npm ci && npm run build`
there; the image copies `dist/` to `/usr/local/share/lanrurugi/telemetry`
(`LANRURUGI_TELEMETRY_UI_DIR`). Without that directory the binary still runs — `/` just 404s.

```
{
  "generated_at": "2026-11-02T01:20:11+00:00",
  "dau": 3, "wau": 11, "mau": 27,
  "countries": [{"name": "JP", "count": 14}, {"name": "CN", "count": 7}],
  "versions":  [{"name": "0.2.0", "count": 22}],
  "channels":  [{"name": "ghcr", "count": 19}, {"name": "native", "count": 5}]
}
```

Both routes **do not exist** when `STATS_TOKEN` is empty — the public surface is write-only, which
is the safer default for something sitting on the internet.

The 30-day window is a deliberate cap: `SUNIONSTORE` costs O(total members), so aggregating the
whole `RETENTION_DAYS` would get slower every month. For a longer trend, snapshot the JSON monthly.

If you would rather query Redis directly:

```sh
# distinct installations over the last 7 days
redis-cli -a "$REDIS_PASSWORD" SUNIONSTORE tmp $(redis-cli -a "$REDIS_PASSWORD" --scan --pattern 'LANRURUGI_TELEMETRY:DAY:*' | tail -7)
redis-cli -a "$REDIS_PASSWORD" SCARD tmp
redis-cli -a "$REDIS_PASSWORD" DEL tmp
```

## Things worth knowing

- **Redis is never published.** If `ports:` ever appears on the `redis` service, an unauthenticated
  Redis is on the internet — that finds a miner within hours.
- **Keys expire.** `RETENTION_DAYS` (default 400) is about disk, not history: nothing here needs a
  migration to change, and the reporting above only ever looks at the last few days.
- **This is not `diagnostics`.** `specs/018`'s error/feedback pipeline stays local and only leaves
  a machine through an administrator-opened window. The heartbeat is a separate, much smaller
  signal, and the two never share a payload.
- **Losing the home uplink is fine.** The client drops a failed send silently and tries again the
  next day; nothing queues up and nothing retries in a loop.
