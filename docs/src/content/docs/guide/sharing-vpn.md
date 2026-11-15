---
title: 让分享走 VPN
description: 用 ExpressVPN + gluetun 让分享传输从 VPN 出口出去，站点本身照旧在局域网；含可直接用的 compose 示例和验证方法。
---

分享是唯一会**主动向外发起连接**的功能：它要和别人的实例建立 QUIC 连接、可能经过中继、也可能被对方反向连过来。
如果不想让这些流量暴露家里的宽带 IP，可以让它单独走一条 VPN 线路，而书库、网页、手机访问仍然留在局域网。

这里用 **ExpressVPN + gluetun** 举例，其他 OpenVPN / WireGuard 服务商只要换掉 gluetun 的几行环境变量即可。

## 两种做法，先选一种

| 做法 | 谁在 VPN 里 | 适合 |
|---|---|---|
| **A. 旁车容器**（本文示例） | 整个 LANrurugi 容器 | 最省心，绝不会漏出真实 IP；代价是别人访问你的站点也要经 VPN 出口 |
| **B. 宿主机 VPN + 绑定接口** | 只有分享端点 | 站点仍从本地网卡提供服务，只有共享流量走 VPN；需要宿主机本身有 `tun0` |

做法 B 不需要额外容器：在宿主机（或普通容器所在的主机）把 VPN 连起来，然后在
**设置 → 分享 → 网络 → 网络接口** 选 `tun0` 即可 —— 共享端点只绑定这张网卡，导入、协助分发、中继转发都走隧道，
站点本身照旧监听局域网。选错接口（比如 VPN 断开、接口名变了）时共享会**直接关闭并在日志里说明原因**，而不是退回全部接口把共享流量漏到物理网卡上；接口回来之后端点会自动重建。

下面的 compose 是做法 A。

## 第一步：拿到 ExpressVPN 的 OpenVPN 凭据

1. 登录 <https://portal.expressvpn.com/setup>，左侧 **VPN → Set up your devices → Manual configuration → OpenVPN**。
2. 页面上直接给出这一组专用 **Username / Password**。它**不是**你登录网站的邮箱密码，忘了可以在这个页面重置。
3. 在同一页选一个国家/城市，下载对应的 `.ovpn` 文件 —— 用 gluetun 时不需要它（gluetun 会自己挑服务器），
   但留一份在本地，方便直接用 OpenVPN 客户端验证线路是否可用。

## 第二步：compose 文件

把下面存成 `compose.yaml`，把两个 `OPENVPN_USER` / `OPENVPN_PASSWORD` 换成第一步拿到的值，然后 `docker compose up -d`：

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
      # 换掉这两个：ExpressVPN 手动配置页给出的那组，不是登录密码
      - OPENVPN_USER=你的用户名
      - OPENVPN_PASSWORD=你的密码
      # 想固定出口国家就取消注释（值用 ISO 名，例如 Japan / Netherlands）
      # - SERVER_COUNTRIES=Japan
      # 内置防火墙：除下面 publish 的端口外，所有进出流量都被挡住
      - FIREWALL=on
      - HEALTH_VPN_DURATION_INITIAL=60s
    ports:
      # 站点的网页与 API
      - "3000:3000"
      - "3001:3001"
      # 分享的 UDP 端口（设置 → 分享 → 连接 里能改，两边要一致）
      - "11204:11204/udp"
      # 只有把本实例当中继节点用时才需要
      # - "11205:11205/tcp"
    restart: unless-stopped

  lanrurugi:
    image: ghcr.io/thelastfantasy/lanrurugi:beta
    container_name: lanrurugi
    # 关键一行：与 vpn 容器共享同一个网络命名空间，出站流量因此只能走隧道
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

两点容易踩的地方：

- **端口要写在 `vpn` 服务上**。`network_mode: "service:vpn"` 之后，应用容器自己没有独立的网络栈，
  它的 `ports:` 会被忽略；gluetun 的 `FIREWALL=on` 也只会放行它自己 publish 的端口。
- **不要再给 `lanrurugi` 加 `ports:`**，同上。

## 第三步：应用里要改的设置

打开 **设置 → 分享**：

- **网络** → **网络接口**：整机走 VPN 时容器里通常只剩 `tun0` 一条默认路由，留「全部接口」也可以；
  想更保险就明确选 `tun0`，这样即使哪天 VPN 掉了、`eth0` 又有路由，共享端点也不会从 `eth0` 出去。
- **网络** → **在路由器上自动映射端口**：建议关掉。你控制不了 VPN 出口那一侧的 NAT，UPnP 只会白跑一趟；
  走中继本来就能连上（速度取决于中继）。
- **连接** → **UDP 端口**：和 compose 里 `11204:11204/udp` 保持一致。

改完保存会重建共享端点，正在进行的导入会短暂中断，之后自动从断点继续。

## 第四步：验证它真的在 VPN 里

```sh
# 出口 IP：应当是 VPN 服务器的地址，不是你家宽带的地址
docker exec lanrurugi curl -s https://api.ipify.org; echo
# 隧道接口存在
docker exec lanrurugi ip -brief addr show tun0
```

再看 **设置 → 分享 → 连接**：显示「运行中 · 直连地址 …」说明端点起来了；
直连地址里出现的是 VPN 出口地址、或者干脆只有中继，都是正常的。

## 会有什么副作用

- **速度**：分享传输（导入、协助分发）全部经 VPN 出口，受 VPN 线路带宽限制；网页浏览因为整机走 VPN 也一样。
- **IP 限制**：对方看到的来源是你**VPN 出口**的地址，不是你家的地址。给分享设 `ip_allow` 时要按出口地址写，
  或者干脆别在有 IP 限制的分享上走 VPN。
- **中继角色**：如果你在 VPN 容器里开「本实例中继」，`11205/tcp` 要 publish 出来，并在
  **中继 → 本中继的公开地址** 里填 VPN 出口的 `https://…`，否则导入方拿不到正确的入口。

## 穿透率是怎么测的

「内网穿透覆盖率 ≥99%」是一个**测出来的数**，不是「配置齐全」的推断。测法固定，才能在不同版本之间比较：

- **拓扑**：`scripts/share-network-matrix.sh` 在本机用 podman 起两个实例、两个路由器容器和一块虚拟 WAN，
  逐个跑 15 种网络环境（同一局域网、NAT 后、双 NAT、对称 NAT、只放行中继、IPv6-only、WAN 不通、
  HTTP 代理、丢包……），每种环境都把一份档案从 A 分享、在 B 导入。
- **成功**：导入方把档案**完整收录**（页数齐全、能打开），不是「连上了」或「握手成功」。
  每一轮跑 3 次，3 次都收录才算这种环境通过。
- **记录**：脚本结尾输出 `N/15` 与 `summary.jsonl`（每行一个环境：编号、环境名、结果、走的路径、
  首字节耗时）。报告的比率就是这个文件里 `landed` 的计数除以总数。
- **基线**：本仓库目前还没有记录基线的真实数字。在真实硬件上跑一次，写明日期、机器、容器版本，
  那个比率才能当结论引用。

## 顺便：用这套东西测中继

同一台机器上可以同时跑「本地实例」和「VPN 实例」，互相导入来验证直连/中继两条路：

- 本地实例照常 `3000`，VPN 实例把网页端口换成别的（例如 `4100:3000`、`4101:3001`），
  否则两边会抢同一个端口；
- 两个实例各用一份数据目录（`./data-lan`、`./data-vpn`），库里有同一批文件才能看出导入走的是哪条路；
- 想测「中继节点」这条链路，再起第三个实例当作中继：在它上面打开 **中继节点**，
  把它的节点码（`lrrg-node:…`）填进另外两台的 **中继节点 → 节点码** 输入框；
- 传输过程中在任一台的日志里能看到走的是直连还是中继，断开中继容器就能看到降级行为。
