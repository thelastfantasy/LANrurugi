#!/usr/bin/env bash
# Sharing interface pin check (022 quickstart A, US1): with `share_bind_interface` naming a second
# interface (the stand-in for a VPN tunnel), sharing traffic must leave only through it — never
# through the host's physical link, which keeps the default route — and must not fall back to the
# physical link when the tunnel goes down.
#
# Topology (share-network-matrix.sh's pieces): the share node on the emulated WAN; the importer
# behind a cone NAT; the sharer with two LANs, each with its own router onto the WAN — `phys` holds
# the default route, `tun` is the pinned interface with a lower-priority default route of its own.
#
# Steps:
#   1. pin, share, import from the other side, capturing both of the sharer's interfaces;
#   2. take the tunnel down and import again, capturing the physical interface.
# Fails when the first import does not finish, when the tunnel saw no sharing traffic, or when the
# physical interface carried any packet in either step other than the kernel's own (ARP, ICMPv6
# neighbour/router messages) and DNS. DNS is listed but not failed: names are resolved through the
# system resolver, which follows the host's own DNS setup (022 T009's recorded boundary).
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=share-net-lib.sh
. "$here/share-net-lib.sh"
trap net_cleanup EXIT

# Reuse the matrix's topology helpers without running its cases.
eval "$(sed -n '/^# Topology pieces/,/^# Cases/p' "$here/share-network-matrix.sh")"
WAN=$SHARE_NET_NET.0 NODE_IP=$SHARE_NET_NET.0.10 NODE_HOST=relay.test
NET=$SHARE_NET_NET
fail=0

# capture_if NAME IFACE FILE — like `capture`, on a named interface.
capture_if() {
    ensure_tools_image
    local target="$SHARE_NET_PREFIX-$1" cap="$SHARE_NET_PREFIX-cap-$1-$2"
    podman run -d --rm --name "$cap" --cap-add NET_ADMIN --cap-add NET_RAW --network "container:$target" \
        -v "$SHARE_NET_WORK:/out:Z" "$SHARE_NET_TOOLS" pcap-capture -Z root -i "$2" -U -w "/out/$3" >/dev/null
    _containers+=("$cap")
}
capture_if_stop() {
    podman kill -s INT "$SHARE_NET_PREFIX-cap-$1-$2" >/dev/null 2>&1 || true
    podman wait "$SHARE_NET_PREFIX-cap-$1-$2" >/dev/null 2>&1 || true
}
# The packets in a capture, one line each, minus the kernel's own link housekeeping (ARP, ICMPv6
# neighbour and router messages, multicast listener reports) and minus DNS unless the second
# argument is `dns`.
packets() {
    local filter="not arp and not icmp6 and not (ip6 multicast and not udp) and not port 53"
    [ "${2:-}" = dns ] && filter="port 53"
    podman run --rm -v "$SHARE_NET_WORK:/out:Z" "$SHARE_NET_TOOLS" \
        sh -c "pcap-capture -r /out/$1 -nn '$filter' 2>/dev/null" || true
}
# dns_note FILE STEP — lists DNS queries on the physical interface without failing on them.
dns_note() {
    local dns
    dns=$(packets "$1" dns)
    [ -z "$dns" ] || echo "NOTE step $2: $(grep -c . <<<"$dns") DNS packets on the physical interface (system resolver)"
}
# netns CONTAINER COMMAND — runs a command with iproute2 in a container's network namespace.
netns() {
    podman run --rm --cap-add NET_ADMIN --network "container:$SHARE_NET_PREFIX-$1" "$SHARE_NET_TOOLS" sh -c "$2"
}

wan_and_node
side b cone

network phys "$NET.1.0/24"
router ra phys "$NET.1.254" wan "$WAN.2" cone
network tun "$NET.3.0/24"
router rt tun "$NET.3.254" wan "$WAN.4" cone
instance a phys "$NET.1.10" "$NET.1.254" --network "$SHARE_NET_PREFIX-tun:ip=$NET.3.10" \
    --add-host "$NODE_HOST:$NODE_IP" -v "$SHARE_NET_WORK/certs:/certs:Z"
phys_if=$(netns a "ip -o addr show | grep -F ' $NET.1.10/' | awk '{print \$2}'")
tun_if=$(netns a "ip -o addr show | grep -F ' $NET.3.10/' | awk '{print \$2}'")
# A VPN client adds a route through the tunnel; the physical default route stays the preferred one,
# so anything not pinned would leave through `phys`.
netns a "ip route add default via $NET.3.254 dev $tun_if metric 200"
ready a
log "sharer: physical $phys_if, tunnel $tun_if"

base_settings a "\"share_bind_interface\":\"$tun_if\""
base_settings b
for _ in $(seq 1 30); do
    binding=$(api a GET /api/shares/settings | jq -r '"\(.endpoint.binding) \(.endpoint.bound_interface)"')
    [ "$binding" = "pinned $tun_if" ] && break
    sleep 1
done
log "endpoint: $binding"
[ "$binding" = "pinned $tun_if" ] || { echo "FAIL the endpoint never reported pinned to $tun_if"; fail=1; }

# Step 1: a transfer while pinned.
capture_if a "$phys_if" 1-phys.pcap
capture_if a "$tun_if" 1-tun.pcap
code=$(share a)
outcome=$(import b "$code" "${IMPORT_TIMEOUT:-180}")
capture_if_stop a "$phys_if"
capture_if_stop a "$tun_if"
log "pinned import: $outcome via $(routes a)"
[ "${outcome% *}" = done ] || { echo "FAIL the pinned import ended $outcome"; fail=1; }

phys1=$(packets 1-phys.pcap)
tun1=$(packets 1-tun.pcap)
echo "=== step 1: $(wc -l <<<"$tun1") packets on $tun_if, $(grep -c . <<<"$phys1" || true) on $phys_if"
[ "$(grep -c . <<<"$tun1" || true)" -gt 0 ] || { echo "FAIL no sharing traffic on the tunnel"; fail=1; }
if [ -n "$phys1" ]; then
    echo "FAIL packets on the physical interface while pinned:"
    head -20 <<<"$phys1"
    fail=1
fi
dns_note 1-phys.pcap 1

# Step 2: the tunnel goes down; nothing may move to the physical link.
api b GET /api/archives | jq -r '.[].arcid' | while read -r id; do
    api b DELETE "/api/archives/$id" >/dev/null 2>&1 || true
done
netns a "ip link set $tun_if down"
capture_if a "$phys_if" 2-phys.pcap
outcome=$(import b "$code" "${DOWN_TIMEOUT:-45}")
capture_if_stop a "$phys_if"
log "import with the tunnel down: $outcome"
[ "${outcome% *}" = done ] && { echo "FAIL the import finished with the tunnel down"; fail=1; }
phys2=$(packets 2-phys.pcap)
echo "=== step 2: $(grep -c . <<<"$phys2" || true) packets on $phys_if with $tun_if down"
if [ -n "$phys2" ]; then
    echo "FAIL packets on the physical interface with the tunnel down:"
    head -20 <<<"$phys2"
    fail=1
fi
dns_note 2-phys.pcap 2

[ "$fail" = 0 ] && echo "PASS sharing stayed on $tun_if"
exit "$fail"
