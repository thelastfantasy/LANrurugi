#!/usr/bin/env bash
# Shared helpers for share-network-matrix.sh and share-traffic-check.sh: throwaway LANrurugi
# instances, NAT routers and outside services as rootless podman containers on private networks.
#
# Every instance runs the dev image's server with its own Redis and the fixture archive as its
# library. API calls go through `podman exec … curl` because rootless container addresses are not
# reachable from the host.
#
#   SHARE_NET_IMAGE    server image (default localhost/lanrurugi-dev-full:latest)
#   SHARE_NET_TOOLS    image with iproute2, iptables, tcpdump, curl, openssl (built on first use)
#   SHARE_NET_PREFIX   container/network name prefix (default lrrnet); SHARE_NET_NET the
#                      address block's first two octets (default 10.99)
#   SHARE_NET_KEEP     non-empty: leave containers and networks behind for inspection
#   SHARE_NET_MB       size of the shared archive in MB (default 3)
#
# Only the sharer (instance `a`) gets a library: a stored zip of random bytes (3 MB unless set), so a transfer
# spans several 1 MiB pieces and the importer cannot reuse a copy of its own.
set -euo pipefail

SHARE_NET_IMAGE="${SHARE_NET_IMAGE:-localhost/lanrurugi-dev-full:latest}"
SHARE_NET_TOOLS="${SHARE_NET_TOOLS:-localhost/lanrurugi-share-net-tools:2}"
SHARE_NET_PREFIX="${SHARE_NET_PREFIX:-lrrnet}"
# First two octets of every emulated network (10.99.x.y by default).
SHARE_NET_NET="${SHARE_NET_NET:-10.99}"
SHARE_NET_WORK="${SHARE_NET_WORK:-$(mktemp -d)}"
SHARE_NET_PASSWORD="kamimamita"

_containers=()
_networks=()

log() { printf '%s %s\n' "$(date +%H:%M:%S)" "$*" >&2; }

net_cleanup() {
    if [ -n "${SHARE_NET_KEEP:-}" ]; then
        log "keeping ${#_containers[@]} containers and ${#_networks[@]} networks (SHARE_NET_KEEP)"
        return
    fi
    [ ${#_containers[@]} -gt 0 ] && podman rm -f --depend -t 1 "${_containers[@]}" >/dev/null 2>&1 || true
    [ ${#_networks[@]} -gt 0 ] && podman network rm -f "${_networks[@]}" >/dev/null 2>&1 || true
    _containers=()
    _networks=()
}

# The tools image: one small Debian with everything the routers, captures and helpers need.
ensure_tools_image() {
    podman image exists "$SHARE_NET_TOOLS" && return
    log "building $SHARE_NET_TOOLS"
    podman build -t "$SHARE_NET_TOOLS" -f - "$SHARE_NET_WORK" >/dev/null <<'EOF'
FROM debian:trixie-slim
RUN apt-get update && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      iproute2 iptables nftables tcpdump curl ca-certificates openssl libndpi-bin \
      miniupnpd wireguard-tools dnsmasq socat \
    && rm -rf /var/lib/apt/lists/* \
    && cp "$(command -v tcpdump)" /usr/local/bin/pcap-capture
# A host AppArmor profile for /usr/bin/tcpdump (Ubuntu) applies inside containers by path and
# refuses every signal, leaving captures unstoppable; the copy under another path is unconfined.
EOF
}

# network NAME SUBNET [--ipv6] — an internal network (no podman gateway to the host).
network() {
    local name="$SHARE_NET_PREFIX-$1" subnet="$2"
    shift 2
    # A previous case's network can outlive its cleanup (rootless netns teardown races): replace it.
    podman ps -aq --filter "network=$name" | xargs -r podman rm -f --depend -t 1 >/dev/null 2>&1 || true
    podman network rm -f "$name" >/dev/null 2>&1 || true
    podman network create --internal --subnet "$subnet" "$@" "$name" >/dev/null
    _networks+=("$name")
}

# router NAME LAN_NET LAN_IP WAN_NET WAN_IP [nat-mode] — a NAT box between two networks. Never give
# it `.1`: podman keeps that address for the bridge itself, even on an internal network.
# nat-mode: cone (default, MASQUERADE), symmetric (randomised source ports), none (plain routing).
router() {
    local name="$SHARE_NET_PREFIX-$1" lan="$SHARE_NET_PREFIX-$2" lan_ip="$3" wan="$SHARE_NET_PREFIX-$4" wan_ip="$5"
    local mode="${6:-cone}"
    ensure_tools_image
    podman run -d --name "$name" --cap-add NET_ADMIN --cap-add NET_RAW \
        --sysctl net.ipv4.ip_forward=1 \
        --network "$lan:ip=$lan_ip" --network "$wan:ip=$wan_ip" \
        "$SHARE_NET_TOOLS" sleep infinity >/dev/null
    _containers+=("$name")
    local wan_if
    wan_if=$(podman exec "$name" ip -o addr show | grep -F " $wan_ip/" | awk '{print $2}' | head -1)
    case "$mode" in
        cone) podman exec "$name" iptables -t nat -A POSTROUTING -o "$wan_if" -j MASQUERADE ;;
        symmetric) podman exec "$name" iptables -t nat -A POSTROUTING -o "$wan_if" -j MASQUERADE --random-fully ;;
        none) ;;
    esac
    # Like a home router: unsolicited inbound packets are dropped silently, not answered with ICMP.
    [ "$mode" = none ] || podman exec "$name" iptables -A INPUT -i "$wan_if" -m conntrack --ctstate NEW -j DROP
}

# Runs iptables inside a router (block UDP, allow only a proxy, …).
router_rule() {
    local name="$SHARE_NET_PREFIX-$1"
    shift
    podman exec "$name" iptables "$@"
}

# instance NAME NET IP GATEWAY [extra podman run args…] — a LANrurugi server with its own Redis.
instance() {
    local short="$1" name="$SHARE_NET_PREFIX-$1" net="$SHARE_NET_PREFIX-$2" ip="$3" gw="$4"
    shift 4
    local lib="$SHARE_NET_WORK/$short-content"
    mkdir -p "$lib"
    if [ "$short" = a ]; then
        local mb="${SHARE_NET_MB:-3}"
        [ -f "$SHARE_NET_WORK/fixture-$mb.zip" ] || python3 -c '
import os, sys, zipfile
with zipfile.ZipFile(sys.argv[1], "w", zipfile.ZIP_STORED) as z:
    for i in range(int(sys.argv[2])):
        z.writestr(f"{i + 1:03}.jpg", os.urandom(1_000_000))
' "$SHARE_NET_WORK/fixture-$mb.zip" "$mb"
        cp "$SHARE_NET_WORK/fixture-$mb.zip" "$lib/fixture.zip"
    fi
    podman run -d --name "$name" --cap-add NET_ADMIN \
        --network "$net:ip=$ip" \
        -v "$lib:/home/koyomi/lanrurugi/content:Z" \
        -e RUST_LOG="${RUST_LOG:-info,lanrurugi_api::share_net=debug}" \
        "$@" \
        --entrypoint sh "$SHARE_NET_IMAGE" -c "
            until [ -e /tmp/net-ready ]; do sleep 0.2; done
            # Test CAs (the node's relay, a TLS-inspecting proxy) go into the system trust store,
            # which is what sharing trusts.
            if ls /certs/*.pem >/dev/null 2>&1; then
                for pem in /certs/*.pem; do cp \"\$pem\" /usr/local/share/ca-certificates/\$(basename \"\$pem\" .pem).crt; done
                update-ca-certificates >/dev/null 2>&1
            fi
            redis-server /etc/lanrurugi/redis.conf --daemonize yes
            until redis-cli -p 16379 ping >/dev/null 2>&1; do sleep 0.1; done
            exec lanrurugi-server serve --bind 0.0.0.0:3001 --log-dir /tmp --disable-update-check
        " >/dev/null
    _containers+=("$name")
    # The server image has no iproute2: routes are set from a tools container in the same network
    # namespace, before the server starts. mDNS needs a multicast route even with no gateway.
    ensure_tools_image
    podman run --rm --cap-add NET_ADMIN --network "container:$name" "$SHARE_NET_TOOLS" sh -c "
        ip route replace default via $gw 2>/dev/null || true
        dev=\$(ip -o addr show | grep -F ' $ip/' | awk '{print \$2}' | head -1)
        ip route add 224.0.0.0/4 dev \$dev 2>/dev/null || true
        ip -6 route add ff00::/8 dev \$dev 2>/dev/null || true
        # IPv6-only cases: podman adds an IPv4 subnet to every network, so drop it here.
        [ -z '${SHARE_NET_V6ONLY:-}' ] || ip -4 addr flush dev \$dev
    "
    podman exec "$name" touch /tmp/net-ready
}

# api NAME METHOD PATH [JSON] — an authenticated API call; prints the body.
api() {
    local name="$SHARE_NET_PREFIX-$1" method="$2" path="$3" body="${4:-}"
    if [ -n "$body" ]; then
        podman exec "$name" curl --noproxy "*" -fsS -b /tmp/jar -c /tmp/jar -X "$method" \
            -H 'content-type: application/json' --data "$body" "http://127.0.0.1:3001$path"
    else
        podman exec "$name" curl --noproxy "*" -fsS -b /tmp/jar -c /tmp/jar -X "$method" "http://127.0.0.1:3001$path"
    fi
}

# Waits for the server and logs in; for the sharer, also waits for its archive to be catalogued.
ready() {
    local name="$SHARE_NET_PREFIX-$1"
    # Not /health: it reports optional subsystems (offline model downloads) that sharing never uses.
    for _ in $(seq 1 120); do
        podman exec "$name" curl --noproxy "*" -fsS -c /tmp/jar -o /dev/null \
            --data-urlencode "password=$SHARE_NET_PASSWORD" http://127.0.0.1:3001/api/login 2>/dev/null && break
        sleep 1
    done
    [ "$1" = a ] || return 0
    for _ in $(seq 1 60); do
        [ "$(api "$1" GET /api/archives | jq 'length')" -gt 0 ] && return 0
        sleep 1
    done
    log "$1: the fixture archive was never catalogued"
    return 1
}

settings() { api "$1" PUT /api/shares/settings "$2" >/dev/null; }

# share NAME — shares the instance's first archive; prints the code's text form.
share() {
    local id
    id=$(api "$1" GET /api/archives | jq -r '.[0].arcid')
    api "$1" POST /api/shares "{\"items\":[{\"kind\":\"archive\",\"id\":\"$id\"}]}" | jq -r '.code.text_form'
}

# import NAME CODE [TIMEOUT] — imports a code and waits; prints done/failed/timeout and the seconds
# until the first byte arrived.
import() {
    local name="$1" code="$2" timeout="${3:-180}"
    local id started first="" state=""
    started=$(date +%s)
    id=$(api "$name" POST /api/shares/imports "{\"code\":\"$code\"}" | jq -r '.import_id') || {
        echo "failed -"
        return
    }
    while [ $(( $(date +%s) - started )) -lt "$timeout" ]; do
        local status
        status=$(api "$name" GET "/api/shares/imports/$id" 2>/dev/null || echo '{}')
        state=$(jq -r '.state // ""' <<<"$status")
        if [ -z "$first" ] && [ "$(jq '[.archives[]?.bytes_done] | add // 0' <<<"$status")" -gt 0 ]; then
            first=$(( $(date +%s) - started ))
        fi
        case "$state" in done | failed) break ;; esac
        sleep 1
    done
    [ -z "$state" ] || [ "$state" = transferring ] || [ "$state" = rebuilding ] && state=timeout
    # Finished between two polls: the first byte came no later than the end.
    [ "$state" = done ] && [ -z "$first" ] && first=$(( $(date +%s) - started ))
    echo "$state ${first:--}"
}

# The routes the sharer logged for incoming connections (direct, relay), deduplicated.
routes() {
    podman logs "$SHARE_NET_PREFIX-$1" 2>&1 | sed 's/\x1b\[[0-9;]*m//g' \
        | grep -o 'route="\?[a-z]*' | sed 's/.*["=]//' | sort -u | paste -sd, -
}

# The endpoint's node string, for configuring friends.
node_string() { api "$1" GET /api/shares/settings | jq -r '.endpoint.node_string // empty'; }

# capture NAME FILE — starts tcpdump in the container's network namespace; stop with capture_stop.
capture() {
    ensure_tools_image
    local target="$SHARE_NET_PREFIX-$1" cap="$SHARE_NET_PREFIX-cap-$1"
    podman run -d --rm --name "$cap" --cap-add NET_ADMIN --cap-add NET_RAW --network "container:$target" \
        -v "$SHARE_NET_WORK:/out:Z" "$SHARE_NET_TOOLS" pcap-capture -Z root -i eth0 -U -w "/out/$2" >/dev/null
    _containers+=("$cap")
}

# tcpdump runs as PID 1 and ignores SIGTERM; SIGINT makes it flush and exit.
capture_stop() {
    podman kill -s INT "$SHARE_NET_PREFIX-cap-$1" >/dev/null 2>&1 || true
    podman wait "$SHARE_NET_PREFIX-cap-$1" >/dev/null 2>&1 || true
}

# A self-signed CA and a server certificate for HOST under DIR (relay role "files" mode).
make_certs() {
    local dir="$1" host="$2"
    ensure_tools_image
    mkdir -p "$dir"
    podman run --rm -v "$dir:/c:Z" "$SHARE_NET_TOOLS" sh -c "
        cd /c
        openssl req -x509 -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -days 2 \
            -subj /CN=test-ca -keyout ca.key -out ca.pem 2>/dev/null
        openssl req -newkey ec -pkeyopt ec_paramgen_curve:P-256 -nodes -subj /CN=$host \
            -addext subjectAltName=DNS:$host -keyout server.key -out server.csr 2>/dev/null
        printf 'subjectAltName=DNS:$host\n' > ext
        openssl x509 -req -in server.csr -CA ca.pem -CAkey ca.key -CAcreateserial -days 2 \
            -extfile ext -out server.pem 2>/dev/null
        chmod 644 *
    "
}
