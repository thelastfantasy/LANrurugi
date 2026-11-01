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

The install id is a random UUIDv4 created on first run. There is no library path, file name, user,
comment or request body in any key — and **no IP address is ever read**: the country comes from
Cloudflare's own `CF-IPCountry` header, so it is decided at the edge and never touches this process.

A client sends at most one request per day, 3-second timeout, never retried, never blocking
startup. Opting out is `LANRURUGI_NO_TELEMETRY=1` on the client side.

## Deploying it

### 1. Create the tunnel

Zero Trust → **Networks → Tunnels → Create a tunnel** → Cloudflared → name it (e.g.
`lanrurugi-hb`) → copy the token (it starts `eyJ`).

Then under **Public Hostnames** add:

| Field | Value |
|---|---|
| Subdomain | `hb` |
| Domain | your zone |
| Service | `HTTP` → `http://ingest:8099` |

`ingest` is this compose file's service name — cloudflared reaches it over the compose network, so
no port is ever published to the host.

### 2. Bring it up

On TrueNAS, create a dataset for Redis data (e.g. `apps/lanrurugi-telemetry/redis`), copy
`compose.yaml` and `.env.example` next to it, then:

```sh
cp .env.example .env      # fill REDIS_PASSWORD, TUNNEL_TOKEN, REDIS_DATA
docker compose up -d
```

No Cloudflare **API** token is needed anywhere: the tunnel's own run token is the only credential.

### 3. Verify

```sh
# from anywhere on the internet
curl -i -X POST https://hb.example.com/v1/hb \
  -H 'content-type: application/json' \
  -d '{"id":"3f2a1c4e-0000-4000-8000-000000000000","version":"0.2.0","arch":"x86_64","os":"linux"}'
# expect: 204

# on the NAS
docker compose exec redis redis-cli -a "$REDIS_PASSWORD" SCARD "LANRURUGI_TELEMETRY:DAY:2026-11-01"
docker compose exec redis redis-cli -a "$REDIS_PASSWORD" PFCOUNT "LANRURUGI_TELEMETRY:GEO:2026-11-01:JP"
```

A 204 with a `ZZ` country means the header path works end to end but your own `curl` is not behind
Cloudflare — check `PFCOUNT` on `GEO:<date>:ZZ` versus the country you expect.

### 4. Point clients at it

A release image only sends when the sink is configured, so ship it as a deploy-time default:

```yaml
environment:
  LANRURUGI_TELEMETRY_URL: https://hb.example.com/v1/hb
  # LANRURUGI_NO_TELEMETRY: "1"   # every instance can still refuse
```

## Reporting

Active installations in a window — `SUNIONSTORE` into a scratch key, count it, delete it:

```sh
redis-cli -a "$REDIS_PASSWORD" SUNIONSTORE tmp $(redis-cli -a "$REDIS_PASSWORD" --scan --pattern 'LANRURUGI_TELEMETRY:DAY:*' | tail -7)
redis-cli -a "$REDIS_PASSWORD" SCARD tmp
redis-cli -a "$REDIS_PASSWORD" DEL tmp
```

Countries, today:

```sh
for k in $(redis-cli -a "$REDIS_PASSWORD" --scan --pattern 'LANRURUGI_TELEMETRY:GEO:2026-11-01:*'); do
  printf '%s %s\n' "${k##*:}" "$(redis-cli -a "$REDIS_PASSWORD" PFCOUNT "$k")"
done
```

Versions, today:

```sh
for k in $(redis-cli -a "$REDIS_PASSWORD" --scan --pattern 'LANRURUGI_TELEMETRY:VER:2026-11-01:*'); do
  printf '%s %s\n' "${k##*:}" "$(redis-cli -a "$REDIS_PASSWORD" SCARD "$k")"
done
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
