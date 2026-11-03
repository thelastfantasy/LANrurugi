#!/usr/bin/env bash
# Sharing network matrix (017 quickstart Scenario 9): for each environment, two throwaway instances
# behind emulated routers share and import the fixture archive, and the route the sharer saw is
# recorded. Everything runs as rootless podman containers on internal networks; nothing reaches the
# real internet, so public services are off and a `share-node` container on the emulated WAN is
# the rendezvous and relay.
#
# Usage: share-network-matrix.sh [case…]   (default: every case; cases are 1–15, see below)
# Prints one line per case: number, name, result, route, seconds to first byte.
# Captures land in $SHARE_NET_WORK/<case>-{a,b}.pcap when SHARE_NET_CAPTURE is set, for
# share-traffic-check.sh.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
# shellcheck source=share-net-lib.sh
. "$here/share-net-lib.sh"
trap net_cleanup EXIT

WAN=$SHARE_NET_NET.0
NODE_IP=$WAN.10
NODE_HOST=relay.test
results=()

# ---------------------------------------------------------------------------------------------
# Topology pieces

# The emulated internet with the share node (rendezvous + relay, TLS from a private CA).
wan_and_node() {
    network wan "$WAN.0/24"
    make_certs "$SHARE_NET_WORK/certs" "$NODE_HOST"
    podman run -d --name "$SHARE_NET_PREFIX-node" --network "$SHARE_NET_PREFIX-wan:ip=$NODE_IP" \
        --add-host "$NODE_HOST:$NODE_IP" -v "$SHARE_NET_WORK/certs:/certs:Z" \
        --entrypoint lanrurugi-server "$SHARE_NET_IMAGE" \
        share-node --data /tmp/node --udp-port 11204 --public-host "$NODE_HOST" \
        --relay --relay-port 443 --relay-tls files --ca-file /certs/ca.pem \
        --relay-cert /certs/server.pem --relay-key /certs/server.key >/dev/null
    _containers+=("$SHARE_NET_PREFIX-node")
    for _ in $(seq 1 30); do
        NODE=$(podman logs "$SHARE_NET_PREFIX-node" 2>/dev/null | grep -o '^lrrg-node:[^ ]*' | head -1 || true)
        [ -n "$NODE" ] && break
        sleep 1
    done
    [ -n "$NODE" ] || { log "share-node printed no node string"; return 1; }
}

# side LETTER NAT-MODE [instance args…] — a LAN with a router onto the WAN and one instance.
side() {
    local s="$1" mode="$2"
    shift 2
    local n=$([ "$s" = a ] && echo 1 || echo 2)
    network "lan-$s" "$SHARE_NET_NET.$n.0/24"
    router "r$s" "lan-$s" "$SHARE_NET_NET.$n.254" wan "$WAN.$((n + 1))" "$mode"
    instance "$s" "lan-$s" "$SHARE_NET_NET.$n.10" "$SHARE_NET_NET.$n.254" \
        --add-host "$NODE_HOST:$NODE_IP" -v "$SHARE_NET_WORK/certs:/certs:Z" "$@"
    ready "$s"
}

# The settings both instances get unless a case overrides them.
base_settings() {
    settings "$1" "{\"share_public_services\":false,\"share_nodes\":\"$NODE\"${2:+,$2}}"
}

# Port-forward 11204/udp on a router to its LAN instance (manual forward, published container port).
forward() {
    local s="$1" n=$([ "$1" = a ] && echo 1 || echo 2)
    router_rule "r$s" -t nat -A PREROUTING -p udp --dport 11204 -j DNAT --to-destination "$SHARE_NET_NET.$n.10:11204"
}

# run CASE NAME [code-field] — share from a, import on b, record the outcome.
run() {
    local num="$1" name="$2" field="${3:-text_form}"
    ready a && ready b || { results+=("$num|$name|setup-failed|-|-"); return; }
    [ -n "${SHARE_NET_CAPTURE:-}" ] && { capture a "$num-a.pcap"; capture b "$num-b.pcap"; }
    local id code outcome
    id=$(api a GET /api/archives | jq -r '.[0].arcid')
    code=$(api a POST /api/shares "{\"items\":[{\"kind\":\"archive\",\"id\":\"$id\"}]}" | jq -r ".code.$field")
    outcome=$(import b "$code" "${IMPORT_TIMEOUT:-180}")
    [ -n "${SHARE_NET_CAPTURE:-}" ] && { capture_stop a; capture_stop b; }
    results+=("$num|$name|${outcome% *}|$(routes a)|${outcome#* }")
    log "case $num ($name): ${outcome% *} via $(routes a)"
}

fresh() { net_cleanup; }

# ---------------------------------------------------------------------------------------------
# Cases (quickstart Scenario 9)

case_1() { # VPS whose public address is not on its interface (1:1 NAT) + share_external_addrs
    wan_and_node; side a none; side b cone; forward a
    router_rule ra -t nat -A POSTROUTING -o "$(wan_if ra)" -j SNAT --to-source "$WAN.2"
    base_settings a "\"share_external_addrs\":\"$WAN.2:11204\""; base_settings b
    run 1 "vps-1to1-nat"
}

case_2() { # home NAT, no UPnP: hole punching
    wan_and_node; side a cone; side b cone
    base_settings a "\"share_portmap\":false"; base_settings b "\"share_portmap\":false"
    run 2 "home-nat-holepunch"
}

case_3() { # double NAT (CGNAT) on both sides: relay through the node only
    wan_and_node
    for s in a b; do
        local n=$([ "$s" = a ] && echo 1 || echo 2)
        network "isp-$s" "$SHARE_NET_NET.1$n.0/24"
        router "c$s" "isp-$s" "$SHARE_NET_NET.1$n.254" wan "$WAN.1$n" symmetric
        network "lan-$s" "$SHARE_NET_NET.$n.0/24"
        router "r$s" "lan-$s" "$SHARE_NET_NET.$n.254" "isp-$s" "$SHARE_NET_NET.1$n.2" symmetric
        podman exec "$SHARE_NET_PREFIX-r$s" ip route replace default via "$SHARE_NET_NET.1$n.254"
        instance "$s" "lan-$s" "$SHARE_NET_NET.$n.10" "$SHARE_NET_NET.$n.254" \
            --add-host "$NODE_HOST:$NODE_IP" -v "$SHARE_NET_WORK/certs:/certs:Z"
        ready "$s"
    done
    base_settings a; base_settings b
    run 3 "double-nat"
}

case_4() { # UDP blocked on the importer's side: relay over TCP 443
    wan_and_node; side a cone; side b cone
    router_rule rb -I FORWARD -p udp -j DROP
    base_settings a; base_settings b
    run 4 "udp-blocked"
}

case_5() { # only an HTTP proxy gets out, set as the system proxy (HTTPS_PROXY)
    wan_and_node; side a cone
    side b cone -e "HTTPS_PROXY=http://$WAN.20:3128"
    podman run -d --name "$SHARE_NET_PREFIX-squid" --network "$SHARE_NET_PREFIX-wan:ip=$WAN.20" \
        --add-host "$NODE_HOST:$NODE_IP" docker.io/ubuntu/squid:latest >/dev/null
    _containers+=("$SHARE_NET_PREFIX-squid")
    router_rule rb -I FORWARD -j DROP
    router_rule rb -I FORWARD -d "$WAN.20" -p tcp --dport 3128 -j ACCEPT
    router_rule rb -I FORWARD -m state --state ESTABLISHED,RELATED -j ACCEPT
    base_settings a; base_settings b
    run 5 "http-proxy-only"
}

case_6() { # only a SOCKS5 proxy gets out, set as the system proxy (ALL_PROXY)
    wan_and_node; side a cone
    side b cone -e "ALL_PROXY=socks5h://lrr:pw@$WAN.21:1080"
    podman run -d --name "$SHARE_NET_PREFIX-socks" --network "$SHARE_NET_PREFIX-wan:ip=$WAN.21" \
        --add-host "$NODE_HOST:$NODE_IP" -e PROXY_USER=lrr -e PROXY_PASSWORD=pw docker.io/serjs/go-socks5-proxy:latest >/dev/null
    _containers+=("$SHARE_NET_PREFIX-socks")
    router_rule rb -I FORWARD -j DROP
    router_rule rb -I FORWARD -d "$WAN.21" -p tcp --dport 1080 -j ACCEPT
    router_rule rb -I FORWARD -m state --state ESTABLISHED,RELATED -j ACCEPT
    base_settings a; base_settings b
    run 6 "socks5-only"
}

case_7() { # IPv6-only
    network v6 "2a0e:99::/64" --ipv6
    make_certs "$SHARE_NET_WORK/certs" "$NODE_HOST"
    local v6node="2a0e:99::10"
    podman run -d --name "$SHARE_NET_PREFIX-node" --network "$SHARE_NET_PREFIX-v6:ip=$v6node" \
        --add-host "$NODE_HOST:$v6node" -v "$SHARE_NET_WORK/certs:/certs:Z" \
        --entrypoint lanrurugi-server "$SHARE_NET_IMAGE" \
        share-node --data /tmp/node --public-host "$NODE_HOST" --relay --relay-tls files --ca-file /certs/ca.pem \
        --relay-cert /certs/server.pem --relay-key /certs/server.key >/dev/null
    _containers+=("$SHARE_NET_PREFIX-node")
    sleep 3
    NODE=$(podman logs "$SHARE_NET_PREFIX-node" | grep -o '^lrrg-node:[^@]*')"@[$v6node]:11204?relay=https://$NODE_HOST/"
    for s in a b; do
        SHARE_NET_V6ONLY=1 instance "$s" v6 "2a0e:99::$([ "$s" = a ] && echo 21 || echo 22)" "2a0e:99::1" \
            --add-host "$NODE_HOST:$v6node" -v "$SHARE_NET_WORK/certs:/certs:Z"
        ready "$s"
    done
    base_settings a; base_settings b
    run 7 "ipv6-only"
}

case_8() { # container bridge with the UDP port published (DNAT), address learnt from the relay
    wan_and_node; side a cone; side b cone; forward a
    base_settings a; base_settings b
    run 8 "published-port"
}

case_9() { # TLS-inspecting system proxy whose CA the system trusts
    wan_and_node
    podman run -d --name "$SHARE_NET_PREFIX-mitm" --network "$SHARE_NET_PREFIX-wan:ip=$WAN.22" \
        --add-host "$NODE_HOST:$NODE_IP" -v "$SHARE_NET_WORK/certs:/certs:Z" \
        docker.io/mitmproxy/mitmproxy:latest \
        mitmdump --set ssl_verify_upstream_trusted_ca=/certs/ca.pem >/dev/null
    _containers+=("$SHARE_NET_PREFIX-mitm")
    sleep 3
    podman exec "$SHARE_NET_PREFIX-mitm" cat /home/mitmproxy/.mitmproxy/mitmproxy-ca-cert.pem \
        >"$SHARE_NET_WORK/certs/mitm-ca.pem"
    side a cone
    side b cone -e "HTTPS_PROXY=http://$WAN.22:8080"
    router_rule rb -I FORWARD -j DROP
    router_rule rb -I FORWARD -d "$WAN.22" -p tcp --dport 8080 -j ACCEPT
    router_rule rb -I FORWARD -m state --state ESTABLISHED,RELATED -j ACCEPT
    base_settings a; base_settings b
    run 9 "tls-intercepting-proxy"
}

case_10() { # isolated LAN, public services off, no nodes: mDNS
    network lan "$SHARE_NET_NET.1.0/24"
    instance a lan $SHARE_NET_NET.1.10 $SHARE_NET_NET.1.1
    instance b lan $SHARE_NET_NET.1.11 $SHARE_NET_NET.1.1
    ready a && ready b
    settings a '{"share_public_services":false}'; settings b '{"share_public_services":false}'
    run 10 "isolated-lan-mdns" code
}

case_11() { # WireGuard pair: direct over the tunnel addresses
    wan_and_node; side a cone; side b cone
    router_rule ra -I FORWARD -p udp ! --dport 51820 -j DROP
    router_rule rb -I FORWARD -p udp ! --dport 51820 -j DROP
    local ka kb
    ka=$(podman run --rm "$SHARE_NET_TOOLS" wg genkey); kb=$(podman run --rm "$SHARE_NET_TOOLS" wg genkey)
    forward_wg() { router_rule "r$1" -t nat -A PREROUTING -p udp --dport 51820 -j DNAT --to-destination "$2:51820"; }
    forward_wg a $SHARE_NET_NET.1.10
    for s in a b; do
        local key peerkey addr peer
        [ "$s" = a ] && { key=$ka; peerkey=$kb; addr=10.77.0.1; peer="$WAN.3"; } \
                     || { key=$kb; peerkey=$ka; addr=10.77.0.2; peer="$WAN.2"; }
        # Kernel WireGuard in the instance's network namespace; the key goes in on a pipe because
        # a host AppArmor profile for `wg` (Ubuntu) refuses it reading a key file by path.
        podman run -d --name "$SHARE_NET_PREFIX-wg-$s" --cap-add NET_ADMIN \
            --network "container:$SHARE_NET_PREFIX-$s" "$SHARE_NET_TOOLS" sh -c "
                ip link add wg0 type wireguard
                echo '$key' | wg set wg0 listen-port 51820 private-key /proc/self/fd/0 \
                  peer \$(echo '$peerkey' | wg pubkey) allowed-ips 10.77.0.0/24 endpoint $peer:51820 persistent-keepalive 5
                ip addr add $addr/24 dev wg0; ip link set wg0 up; sleep infinity" >/dev/null
        _containers+=("$SHARE_NET_PREFIX-wg-$s")
    done
    ready a && ready b
    settings a '{"share_public_services":false,"share_external_addrs":"10.77.0.1:11204"}'
    settings b '{"share_public_services":false}'
    run 11 "wireguard-pair" text_form
}

case_12() { # UPnP router (miniupnpd) on the sharer's side
    wan_and_node; side a cone; side b cone
    local wif
    wif=$(wan_if ra)
    podman exec -d "$SHARE_NET_PREFIX-ra" sh -c "
        iptables -t nat -N MINIUPNPD; iptables -t nat -A PREROUTING -i $wif -j MINIUPNPD
        iptables -N MINIUPNPD; iptables -I FORWARD -i $wif -j MINIUPNPD
        miniupnpd -d -i $wif -a $SHARE_NET_NET.1.254 -N -U -P /tmp/miniupnpd.pid"
    base_settings a "\"share_portmap\":true"; base_settings b
    run 12 "upnp-router"
    results[-1]+="|$(api a GET /api/shares/settings | jq -c '.endpoint.direct_addresses')"
}

case_13() { # frp UDP proxy on a VPS, its address in share_external_addrs
    wan_and_node; side a cone; side b cone
    podman run -d --name "$SHARE_NET_PREFIX-frps" --network "$SHARE_NET_PREFIX-wan:ip=$WAN.23" \
        docker.io/snowdreamtech/frps:latest >/dev/null
    _containers+=("$SHARE_NET_PREFIX-frps")
    printf 'serverAddr = "%s"\nserverPort = 7000\n[[proxies]]\nname = "share"\ntype = "udp"\nlocalIP = "127.0.0.1"\nlocalPort = 11204\nremotePort = 7001\n' \
        "$WAN.23" >"$SHARE_NET_WORK/frpc-udp.toml"
    podman run -d --name "$SHARE_NET_PREFIX-frpc" --network "container:$SHARE_NET_PREFIX-a" \
        -v "$SHARE_NET_WORK/frpc-udp.toml:/etc/frp/frpc.toml:Z" docker.io/snowdreamtech/frpc:latest >/dev/null
    _containers+=("$SHARE_NET_PREFIX-frpc")
    base_settings a "\"share_external_addrs\":\"$WAN.23:7001\",\"share_portmap\":false"; base_settings b
    run 13 "frp-udp"
}

case_14() { # own relay in proxy mode behind an frp HTTPS tunnel, no other relay or node
    network wan "$WAN.0/24"
    make_certs "$SHARE_NET_WORK/certs" relay-a.test
    side a cone; side b cone
    printf 'bindPort = 7000\nvhostHTTPSPort = 443\n' >"$SHARE_NET_WORK/frps-https.toml"
    podman run -d --name "$SHARE_NET_PREFIX-frps" --network "$SHARE_NET_PREFIX-wan:ip=$WAN.23" \
        -v "$SHARE_NET_WORK/frps-https.toml:/etc/frp/frps.toml:Z" docker.io/snowdreamtech/frps:latest >/dev/null
    _containers+=("$SHARE_NET_PREFIX-frps")
    printf 'serverAddr = "%s"\nserverPort = 7000\n[[proxies]]\nname = "relay"\ntype = "https"\ncustomDomains = ["relay-a.test"]\n[proxies.plugin]\ntype = "https2http"\nlocalAddr = "127.0.0.1:11205"\ncrtPath = "/certs/server.pem"\nkeyPath = "/certs/server.key"\n' \
        "$WAN.23" >"$SHARE_NET_WORK/frpc-https.toml"
    podman run -d --name "$SHARE_NET_PREFIX-frpc" --network "container:$SHARE_NET_PREFIX-a" \
        -v "$SHARE_NET_WORK/frpc-https.toml:/etc/frp/frpc.toml:Z" -v "$SHARE_NET_WORK/certs:/certs:Z" \
        docker.io/snowdreamtech/frpc:latest >/dev/null
    _containers+=("$SHARE_NET_PREFIX-frpc")
    for s in a b; do podman exec "$SHARE_NET_PREFIX-$s" sh -c "echo '$WAN.23 relay-a.test' >> /etc/hosts"; done
    ready a && ready b
    settings a '{"share_public_services":false,"share_node_relay":true,"share_node_relay_tls":"proxy","share_self_relay_url":"https://relay-a.test"}'
    settings b '{"share_public_services":false}'
    router_rule rb -I FORWARD -p udp -j DROP
    run 14 "own-relay-behind-tunnel" text_form
}

case_15() { # the importer's own proxy from its environment, no LANrurugi proxy setting
    local variant
    for variant in all_proxy https_proxy; do
        fresh
        wan_and_node; side a cone
        podman run -d --name "$SHARE_NET_PREFIX-socks" --network "$SHARE_NET_PREFIX-wan:ip=$WAN.21" \
            --add-host "$NODE_HOST:$NODE_IP" -e PROXY_USER=lrr -e PROXY_PASSWORD=pw docker.io/serjs/go-socks5-proxy:latest >/dev/null
        podman run -d --name "$SHARE_NET_PREFIX-squid" --network "$SHARE_NET_PREFIX-wan:ip=$WAN.20" \
            --add-host "$NODE_HOST:$NODE_IP" docker.io/ubuntu/squid:latest >/dev/null
        _containers+=("$SHARE_NET_PREFIX-socks" "$SHARE_NET_PREFIX-squid")
        if [ "$variant" = all_proxy ]; then
            side b cone -e "ALL_PROXY=socks5h://lrr:pw@$WAN.21:1080"
        else
            side b cone -e "HTTPS_PROXY=http://$WAN.20:3128"
        fi
        router_rule rb -I FORWARD -j DROP
        router_rule rb -I FORWARD -d "$WAN.20,$WAN.21" -p tcp -j ACCEPT
        router_rule rb -I FORWARD -m state --state ESTABLISHED,RELATED -j ACCEPT
        base_settings a; base_settings b
        run 15 "own-proxy-$variant"
        [ "${results[-1]##*|}" != - ] && [ "${results[-1]##*|}" -gt 30 ] && results[-1]+="|SLOW(>30s)"
    done
    # mihomo TUN with fake-IP DNS, its line relaying UDP and then dropping it.
    local udp
    for udp in true false; do
        fresh
        wan_and_node; side a cone
        podman run -d --name "$SHARE_NET_PREFIX-socks" --network "$SHARE_NET_PREFIX-wan:ip=$WAN.21" \
            --add-host "$NODE_HOST:$NODE_IP" -e PROXY_USER=lrr -e PROXY_PASSWORD=pw docker.io/serjs/go-socks5-proxy:latest >/dev/null
        _containers+=("$SHARE_NET_PREFIX-socks")
        side b cone
        cat >"$SHARE_NET_WORK/mihomo.yaml" <<EOF
mode: rule
dns: {enable: true, enhanced-mode: fake-ip, nameserver: [127.0.0.11], hosts: {$NODE_HOST: $NODE_IP}}
tun: {enable: true, stack: system, auto-route: true, dns-hijack: [any:53]}
proxies: [{name: line, type: socks5, server: $WAN.21, port: 1080, username: lrr, password: pw, udp: $udp}]
rules: ["IP-CIDR,$WAN.21/32,DIRECT", "MATCH,line"]
EOF
        podman run -d --name "$SHARE_NET_PREFIX-mihomo" --cap-add NET_ADMIN --device /dev/net/tun \
            --network "container:$SHARE_NET_PREFIX-b" -v "$SHARE_NET_WORK/mihomo.yaml:/root/.config/mihomo/config.yaml:Z" \
            docker.io/metacubex/mihomo:latest >/dev/null
        _containers+=("$SHARE_NET_PREFIX-mihomo")
        router_rule rb -I FORWARD -j DROP
        router_rule rb -I FORWARD -d "$WAN.21" -j ACCEPT
        router_rule rb -I FORWARD -m state --state ESTABLISHED,RELATED -j ACCEPT
        base_settings a; base_settings b
        run 15 "own-proxy-mihomo-tun-udp-$udp"
    done
}

# The WAN-side interface name of a router container.
wan_if() {
    podman exec "$SHARE_NET_PREFIX-$1" sh -c "ip -o -4 route get $NODE_IP | sed -n 's/.* dev \([^ ]*\).*/\1/p'"
}

# ---------------------------------------------------------------------------------------------

cases=("$@")
[ ${#cases[@]} -eq 0 ] && cases=($(seq 1 15))
for c in "${cases[@]}"; do
    fresh
    log "case $c"
    "case_$c" || results+=("$c|case_$c|error|-|-")
done

printf '\n%-4s %-32s %-10s %-14s %s\n' '#' environment result route first-byte-s
for r in "${results[@]}"; do
    IFS='|' read -r num name res route first extra <<<"$r"
    printf '%-4s %-32s %-10s %-14s %s %s\n' "$num" "$name" "$res" "${route:--}" "$first" "${extra:-}"
done
echo "work directory: $SHARE_NET_WORK"
