# Quickstart: 资源监视的真实验收

全部在 `lrr-dev` 容器上跑（`mise run dev-rebuild` 之后），不靠单测代替。登录 cookie：

```sh
curl -s -X POST localhost:3000/api/login -d 'password=kamimamita' -c /tmp/lrr-cookies.txt
```

## 场景 1 — 一次性快照（US2 的最小验证）

```sh
curl -s -b /tmp/lrr-cookies.txt localhost:3000/api/resources | jq '.snapshot | {at, build, server, cgroup, runtime, processes, models, redis, watch}'
```

期望：`server.rss_bytes > 0`、`cgroup.anon_bytes < cgroup.memory_current_bytes`（证明拆开了）、`runtime.cpu_cores >= 1`、`build.profile == "debug"`（dev 容器；发布镜像里是 `release`）、`processes.tree` 里有 `depth: 0` 的本进程、`comm` 里能看到 `deno`（先随便调一次插件）与 `lanrurugi-book`（先打开一本电子书）、`redis.used_memory_rss_bytes > 0`、`models.usage` 至少有当前配了的 feature、`watch.samples_taken == 0` 且 `watch.sampler_running == false`（一次性请求不应留下任务）。

## 场景 2 — SSE 首帧与帧率（US2）

```sh
timeout 5 curl -sN -b /tmp/lrr-cookies.txt localhost:3000/api/resources/stream | head -20
```

期望：`event: snapshot` 先到，随后 1 s 一帧 `event: sample`；5 秒内 ≥ 3 帧 `sample`。

## 场景 3 — 关掉即停（FR-001/FR-002，最关键的一条）

```sh
# 1) 开一个流并立刻断开
timeout 3 curl -sN -b /tmp/lrr-cookies.txt localhost:3000/api/resources/stream >/dev/null
sleep 3
# 2) 两次快照之间 samples_taken 不应增长
curl -s -b /tmp/lrr-cookies.txt localhost:3000/api/resources | jq '.snapshot.watch'
sleep 10
curl -s -b /tmp/lrr-cookies.txt localhost:3000/api/resources | jq '.snapshot.watch'
```

期望：两次 `samples_taken` 相同、`sampler_running == false`、`observers == 0`。同时 `ps -o pid,comm -C deno` 的进程数不应因这 10 秒而变化。

## 场景 4 — 两个观察者共享一个任务（US2 边界）

```sh
timeout 6 curl -sN -b /tmp/lrr-cookies.txt localhost:3000/api/resources/stream >/tmp/a.sse &
timeout 6 curl -sN -b /tmp/lrr-cookies.txt localhost:3000/api/resources/stream >/tmp/b.sse &
sleep 2
curl -s -b /tmp/lrr-cookies.txt localhost:3000/api/resources | jq '.snapshot.watch'
wait
curl -s -b /tmp/lrr-cookies.txt localhost:3000/api/resources | jq '.snapshot.watch'
```

期望：中间那次 `observers == 2`（一次性快照本身不计数）；结束后 `observers == 0`、`sampler_running == false`。

## 场景 5 — 反馈包里的 resources.json（US1）

```sh
curl -s -b /tmp/lrr-cookies.txt -H 'content-type: application/json' \
  -d "{\"from\": $(( $(date +%s%3N) - 600000 )), \"to\": $(date +%s%3N), \"levels\": [\"error\"], \"categories\": {\"server_logs\": false, \"frontend_events\": false, \"environment\": true, \"config\": true, \"resources\": true}}" \
  localhost:3000/api/diagnostics/bundles -o /tmp/bundle.zip
unzip -l /tmp/bundle.zip
unzip -p /tmp/bundle.zip resources.json | jq 'keys'
unzip -p /tmp/bundle.zip diagnostics.json | jq 'keys'      # 不应为空文件
unzip -p /tmp/bundle.zip config.redacted.json | head -3
```

期望：三个 JSON 都有内容；`manifest.json` 的 `files[]` 列出它们且字节数 > 2；`resources.json` 的 `build.profile` 是 `debug`，里面出现的进程名带 `(debug)`。

## 场景 6 — 不勾资源就不含（US1 边界）

用同样的请求把 `resources` 设为 `false`，`unzip -l` 里不应出现 `resources.json`，其余文件不变。

## 场景 7 — 降级（Edge cases）

在容器里临时让 PSI 不可读（`sudo mount --bind /dev/null /proc/pressure` 或改采集路径的单元测试替身）：

期望：`cgroup.psi_memory` 为 `null` 且同级出现 `unavailable` 说明，页面显示 `—`，其它字段照常，接口不 500。

## 场景 8 — 错误事件带资源上下文（US4）

```sh
curl -s -b /tmp/lrr-cookies.txt -X POST localhost:3000/api/diagnostics/envelope/ \
  -d '{"message":"resource context probe","level":"error"}'
curl -s -b /tmp/lrr-cookies.txt 'localhost:3000/api/diagnostics/issues?limit=1' | jq '.issues[0].contexts.resources'
```

期望：最近一次 error 事件带 6 个数字与采样时刻；若从未打开过面板，则**不**带（说明只在有效采样时附加）。

## 场景 9 — 阈值告警与去重（US4）

在容器内制造内存压力（`stress-ng --vm 1 --vm-bytes 2G` 或分批下载）直到 `memory.events` 的 `oom` 增长：

```sh
podman exec lrr-dev cat /sys/fs/cgroup/memory.events
```

期望：`GET /api/diagnostics/issues` 出现一条 `resources.*` warning，且 10 分钟内不重复出现第二条。

## 场景 10 — 前端面板（US2/US3/US5）

浏览器打开 `设置 → 资源`：表格在 1 s 内出现并每秒刷新；「复制为文本」的内容含 version/release/platform 三行；「导出 CSV」每行一个时间点。关闭面板后按场景 3 的方式确认服务端停止采样。

## 场景 11 — 采样不拖慢下载（SC-004）

同一批 3 个链接，分两次加入队列：第一次**不开**资源面板，第二次面板一直开着；两次都从 `POST /api/download_queue/start_all` 记到全部 `done`。

```sh
curl -s -b /tmp/lrr-cookies.txt 'localhost:3000/api/download_queue' | jq '[.items[] | {state, average_bytes_per_sec}]'
```

期望：两次的平均速率之比 ≥ 0.90，且开启采样期间 `GET /api/resources` 的 `server.cpu_percent_1s` 始终 < 100（不到一个核）。同时记录 `collect()` 的 p95（见 tasks.md T009）——超过 50 ms 即视为不达标。

一次只跑一批，不要在两次之间改并发上限或换链接，否则数字不可比。
