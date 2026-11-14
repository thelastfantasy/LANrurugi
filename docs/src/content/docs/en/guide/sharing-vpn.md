---
title: Sharing over a VPN
description: Send sharing traffic out through a VPN (ExpressVPN + gluetun), keep the site itself on the LAN. Includes a compose file you can paste and how to check it is really tunnelled.
---

Sharing is the one feature that **reaches out on its own**: it opens QUIC connections to other
instances, may go through a relay, and may be dialled from outside. If you would rather that traffic
did not carry your home connection's address, run it over a VPN while the library, the web UI and your
phone keep talking to the LAN.

The example below uses **ExpressVPN + gluetun**; any OpenVPN or WireGuard provider works the same way
once you swap gluetun's environment variables.

## Two ways to do it

| Approach | What is inside the VPN | Good for |
|---|---|---|
| **A. Sidecar container** (this page) | The whole LANrurugi container | Simplest, never leaks your real address; the cost is that visitors reach your site through the VPN exit too |
| **B. Host VPN + bound interface** | Only the sharing endpoint | The site keeps serving from the LAN and only share traffic is tunnelled; needs a `tun0` on the host |

Approach B needs no extra container: connect the VPN on the host (or on the host of an ordinary
container), then pick `tun0` under **Settings → Sharing → Network → Network interface**. The sharing
endpoint binds to that interface only — imports, helping to deliver and relayed forwarding all use the
tunnel, while the site keeps listening on the LAN. If the chosen interface disappears (the VPN drops, the
name changes) **sharing turns off** and the log says why, rather than falling back to all interfaces
and leaking share traffic onto the physical link; the endpoint is rebuilt once the interface returns.

The compose file below is approach A.

## Step 1 — get the ExpressVPN OpenVPN credentials

1. Sign in at <https://portal.expressvpn.com/setup>, then **VPN → Set up your devices → Manual
   configuration → OpenVPN**.
2. That page shows a dedicated **Username / Password**. It is **not** the email/password you sign in
   with, and it can be reset from the same page.
3. Pick a country or city and download the matching `.ovpn` file. gluetun does not need it (it picks a
   server itself), but keeping one around is handy for checking the line with a plain OpenVPN client.

## Step 2 — the compose file

Save this as `compose.yaml`, replace the two `OPENVPN_USER` / `OPENVPN_PASSWORD` values with the ones
from step 1, and run `docker compose up -d`:

```yaml
services:
  vpn:
    image: qmcgaw/gluetun:latest
    container_name: lrrg-vpn
    cap_add:
      - NET_ADMIN
    devices:
      - /dev/net/tun:/dev/net/tun
    environment:
      - VPN_SERVICE_PROVIDER=expressvpn
      - VPN_TYPE=openvpn
      # Replace these: the pair from ExpressVPN's manual configuration page, not your login password
      - OPENVPN_USER=your-username
      - OPENVPN_PASSWORD=your-password
      # Pin the exit country by uncommenting this (ISO name, e.g. Japan / Netherlands)
      # - SERVER_COUNTRIES=Japan
      # Built-in firewall: everything except the published ports below is blocked
      - FIREWALL=on
      - HEALTH_VPN_DURATION_INITIAL=60s
    ports:
      # The web UI and its API
      - "3000:3000"
      - "3001:3001"
      # The sharing UDP port (changeable under Settings → Sharing → Connectivity; both must match)
      - "11204:11204/udp"
      # Only needed when this instance also acts as a relay node
      # - "11205:11205/tcp"
    restart: unless-stopped

  lanrurugi:
    image: ghcr.io/thelastfantasy/lanrurugi:beta
    container_name: lanrurugi
    # The key line: shares the vpn container's network namespace, so outbound traffic can only be tunnelled
    network_mode: "service:vpn"
    depends_on:
      - vpn
    volumes:
      - ./data/manga:/home/koyomi/lanrurugi/content
      - ./data/lanrurugi:/data
    environment:
      - LANRURUGI_DATA_DIR=/data
      - LANRURUGI_FORCE_SECURE_COOKIES=false
    restart: unless-stopped
```

Two things that trip people up:

- **Ports belong on the `vpn` service.** With `network_mode: "service:vpn"` the app container has no
  network stack of its own, so its `ports:` are ignored — and gluetun's `FIREWALL=on` only lets through
  the ports it publishes itself.
- **Do not add `ports:` to `lanrurugi`** either, for the same reason.

## Step 3 — settings to change

Under **Settings → Sharing**:

- **Network → Network interface**: inside a VPN container there is usually only a `tun0` default
  route, so "all interfaces" works; picking `tun0` explicitly is safer, because then the sharing
  endpoint cannot leave through `eth0` even if the VPN drops and a route reappears.
- **Network → map the port on my router automatically**: turn it off. You do not control the NAT on the
  VPN exit, so UPnP only wastes a round trip; a relayed connection works anyway (speed permitting).
- **Connectivity → UDP port**: keep it in step with `11204:11204/udp` in the compose file.

Saving rebuilds the sharing endpoint; imports under way pause briefly and then resume from their
checkpoint.

## Step 4 — prove it is really tunnelled

```sh
# Exit address: the VPN server's, not your home connection's
docker exec lanrurugi curl -s https://api.ipify.org; echo
# The tunnel interface exists
docker exec lanrurugi ip -brief addr show tun0
```

Then look at **Settings → Sharing → Connectivity**: "running · direct addresses …" means the endpoint
is up. Seeing the VPN exit address there, or only relays, is normal.

## Side effects

- **Speed**: sharing traffic (imports, helping to deliver) goes out through the VPN and is bounded by
  that line; with approach A the web UI is too.
- **IP rules**: a recipient sees your **VPN exit** address, not your home one. Write `ip_allow` entries
  against the exit address, or simply do not combine VPN sharing with IP rules.
- **Relay role**: if you enable relaying inside the VPN container, publish `11205/tcp` and put the VPN
  exit's `https://…` into **Relay → this relay's public address**, otherwise importers never learn the
  right entry point.

## How penetration is measured

"At least 99% of NAT topologies" is a **measured** number, not an inference from "everything is
configured". The procedure is fixed so runs stay comparable across versions:

- **Topologies**: `scripts/share-network-matrix.sh` starts two instances, two router containers and a
  virtual WAN with podman on one host, then walks 15 network environments (same LAN, behind NAT,
  double NAT, symmetric NAT, relay-only, IPv6-only, WAN down, HTTP proxy, packet loss, ...). Each
  environment shares one archive from A and imports it on B.
- **Success**: the importing side **catalogues the archive completely** (all pages present and
  readable) — not "connected" and not "handshake succeeded". Every environment runs three times;
  all three have to land for it to count.
- **Record**: the script ends with `N/15` and a `summary.jsonl` (one line per environment: case,
  environment name, result, route taken, time to first byte). The reported rate is the `landed`
  count in that file divided by the total.
- **Baseline**: this repository has no recorded baseline yet. Run it on real hardware once, write
  down the date, machine and container versions, and that rate is what may be quoted.

## Bonus: use the same setup to test relaying

One machine can run a "LAN instance" and a "VPN instance" at once and import between them to exercise
both paths:

- the LAN instance keeps `3000`, give the VPN instance different web ports (for example `4100:3000`,
  `4101:3001`), otherwise they fight over the same port;
- give each its own data directory (`./data-lan`, `./data-vpn`) and the same files, so you can tell
  which path an import took;
- to test the relay path, start a third instance as a relay: enable **Relay node** on it and paste its
  node code (`lrrg-node:…`) into the other two instances' **Relay nodes → node code** box;
- the logs show whether a transfer went direct or through a relay; stopping the relay container shows
  the degradation.
