#!/usr/bin/env bash
# Sharing traffic-profile check (017 research §15, FR-045, SC-016): captures both instances'
# traffic through a scripted lifecycle and fails on anything a DPI classifier or a plaintext match
# could single out.
#
# Lifecycle (on the share-network-matrix.sh topology, share node on the emulated WAN):
#   1. create a share, resolve it, import it directly (home NAT, hole punching);
#   2. import it again with UDP blocked on the importer's side (relay over TCP 443);
#   3. when the server supports helpers, a third instance imports with "help deliver" on and a
#      fourth imports while it helps.
# Each capture goes through nDPI's ndpiReader. The check fails on:
#   - any flow nDPI classifies as BitTorrent or another P2P protocol;
#   - any QUIC/TLS ALPN or SNI naming lanrurugi, lrrg or iroh;
#   - any packet carrying `lanrurugi`, `lrrg` or `iroh` in plaintext.
# Prints the classifier summary per capture, for the Implementation Record.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
export SHARE_NET_CAPTURE=1
# shellcheck source=share-net-lib.sh
. "$here/share-net-lib.sh"
trap net_cleanup EXIT

# Reuse the matrix's topology helpers without running its cases.
eval "$(sed -n '/^# Topology pieces/,/^# Cases/p' "$here/share-network-matrix.sh")"
eval "$(sed -n '/^wan_if()/,/^}/p' "$here/share-network-matrix.sh")"
WAN=$SHARE_NET_NET.0 NODE_IP=$SHARE_NET_NET.0.10 NODE_HOST=relay.test
results=()

wan_and_node
side a cone
side b cone
base_settings a
base_settings b
capture node node.pcap
run 1 direct
router_rule rb -I FORWARD -p udp -j DROP
# The importer must not already hold the archive, or the second import is a reuse with no transfer.
api b GET /api/archives | jq -r '.[].arcid' | while read -r id; do
    api b DELETE "/api/archives/$id" >/dev/null 2>&1 || true
done
run 2 udp-blocked

if api a GET /api/shares/settings | jq -e '.helpers_supported == true' >/dev/null 2>&1; then
    instance c lan-b $SHARE_NET_NET.2.11 $SHARE_NET_NET.2.254 --add-host "$NODE_HOST:$NODE_IP" -v "$SHARE_NET_WORK/certs:/certs:Z"
    instance d lan-b $SHARE_NET_NET.2.12 $SHARE_NET_NET.2.254 --add-host "$NODE_HOST:$NODE_IP" -v "$SHARE_NET_WORK/certs:/certs:Z"
    ready c && ready d
    base_settings c; base_settings d
    capture c 3-c.pcap; capture d 3-d.pcap
    id=$(api a GET /api/archives | jq -r '.[0].arcid')
    code=$(api a POST /api/shares "{\"items\":[{\"kind\":\"archive\",\"id\":\"$id\"}],\"allow_helpers\":true}" | jq -r .code.text_form)
    api c POST /api/shares/imports "{\"code\":\"$code\",\"help_deliver\":true}" >/dev/null
    sleep 10
    import d "$code" >/dev/null
    capture_stop c; capture_stop d
else
    log "helpers not supported by this build; skipping the helping step"
fi
capture_stop node

fail=0
for pcap in "$SHARE_NET_WORK"/*.pcap; do
    name=$(basename "$pcap")
    report=$(podman run --rm -v "$SHARE_NET_WORK:/out:Z" "$SHARE_NET_TOOLS" \
        ndpiReader -i "/out/$name" -v 2 2>/dev/null || true)
    echo "=== $name"
    protocols=$(sed -n '/Detected protocols:/,/^$/p' <<<"$report")
    printf '%s\n' "$protocols"
    if grep -Eiq 'BitTorrent|eDonkey|Gnutella|Kazaa|DirectConnect|Soulseek|Skype|P2P' <<<"$protocols"; then
        echo "FAIL $name: P2P classification"
        fail=1
    fi
    names=$(grep -Eio '(ALPN|SNI|Hostname)[^]]*' <<<"$report" | sort -u || true)
    [ -n "$names" ] && printf '%s\n' "$names"
    if grep -Eiq 'lanrurugi|lrrg|iroh' <<<"$names"; then
        echo "FAIL $name: a handshake names the software"
        fail=1
    fi
    if podman run --rm -v "$SHARE_NET_WORK:/out:Z" "$SHARE_NET_TOOLS" \
        sh -c "pcap-capture -r /out/$name -nn -A 'not port 3001' 2>/dev/null" | grep -Eaiq 'lanrurugi|lrrg|iroh'; then
        echo "FAIL $name: plaintext marker"
        fail=1
    fi
done

for r in "${results[@]}"; do echo "lifecycle: $r"; done
echo "captures: $SHARE_NET_WORK"
exit "$fail"
