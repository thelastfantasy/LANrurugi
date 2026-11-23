# Quickstart: 删除安全网验收脚本

## 前置

- 一个**数据库副本**上的实例（绝不在生产/dev 实例上验证）：`<data>` 为副本目录，端口自选。
- 已登录的 cookie：`curl -c jar -d 'password=kamimamita' $BASE/api/login`。
- 库目录可写（快照要落盘）。

```bash
BASE=http://127.0.0.1:3600
J=/tmp/qa-jar; rm -f $J
curl -fsS -c $J -o /dev/null -d 'password=kamimamita' $BASE/api/login
```

## S1 删除台账：删了还能找回来

1. 取一个档案 id 并记下它的标签与进度：

```bash
ID=$(curl -fsS -b $J "$BASE/api/search?start=0&count=1&filter=" | python3 -c 'import json,sys;print(json.load(sys.stdin)["data"][0]["arcid"])')
curl -fsS -b $J "$BASE/api/archives/$ID/metadata" | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d["tags"]);print(d.get("progress"))'
```

2. 删除它（会进回收站，同时写台账）：`curl -fsS -b $J -X DELETE "$BASE/api/archives/$ID"`

3. 台账里应出现该条：

```bash
curl -fsS -b $J "$BASE/api/deletions" | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d["total"], [e["id"] for e in d["entries"]][:3])'
```

**期望**：`total` ≥ 1，包含 `$ID`，`file_exists` 反映磁盘现状。

4. 恢复：`curl -fsS -b $J -X POST "$BASE/api/deletions/$ID/restore"`

**期望**：`success:1`；随后 `GET /api/archives/$ID/metadata` 返回 200，`tags` 与第 1 步一致，处于回收站的文件已回到库目录。

5. 再恢复一次 → **期望** 404（恢复成功后台账条目已被消费）；409 `already_exists` 只在「记录已存在而条目尚未消费」时出现。

## S2 批量删除守卫

1. 把阈值调小，构造一次超阈值的清理：

```bash
curl -fsS -b $J -X PUT -H 'Content-Type: application/json' \
  -d '{"deletion_guard_percent":1,"deletion_guard_min":1}' $BASE/api/settings >/dev/null
curl -sS -b $J -o /tmp/clean1.json -w '%{http_code}\n' -X POST "$BASE/api/database/clean"
cat /tmp/clean1.json
```

**期望**：`409`，`success:0`，带 `planned / total / percent / guard_percent / guard_min`，且库里记录数**未变**。

2. 带确认重试：`curl -sS -b $J -X POST "$BASE/api/database/clean?force=1"`

**期望**：`200`，`deleted` 与 409 里的 `planned` 一致，`journaled` 等于本次删除数。

3. 把阈值调回默认，确认常规清理不再被拦。

## S3 自动快照

1. 重启实例后检查当天是否生成：

```bash
curl -fsS -b $J "$BASE/api/backups" | python3 -m json.tool | head -30
```

**期望**：至少一条 `reason=daily`，`usable=true`；`<library>/.lanrurugi-backups/` 下能看到同名文件，以及同一份
`<文件名>.summary.json`（内容含 `archives / archives_with_tags / categories / tankoubons / stamps / bookmarks / bytes`）。
同一响应里的 `schedule` 应给出 `enabled / daily_at / weekly_day / last_* / next_*`，且 `next_daily_at` 是「下一个
`snapshot_daily_at` 本地时刻」对应的 UTC 瞬时。刷新一次列表：`last_daily_at` 已记录，`next_daily_at` 因此变成明天
—— 重启进程后**不应**再出现第二份同槽位 `daily`。

2. 手动生成：`curl -fsS -b $J -X POST "$BASE/api/backups"`

**期望**：返回新条目 `reason=manual`，磁盘出现该文件，且能被 `GET /api/backups/<name>` 原样下载。

3. 用快照恢复：`curl -fsS -b $J -X POST "$BASE/api/backups/<daily-name>/restore"`

**期望**：`success:1`，并额外生成一份 `pre-restore` 快照。

4. 保留策略：手工在目录里造 10 份日期不同的 `daily` 文件，运行一次保留清理（重启或等 sweep），**期望**只剩最近 7 份，且最新一份一定在。

## S4 库目录诊断

```bash
curl -fsS -b $J -o /dev/null -X POST "$BASE/api/shinobu/rescan"; sleep 20
curl -fsS -b $J "$BASE/api/jobs" | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d["jobs"][0]["result"])'
```

**期望**：结果里含 `scanned_files`、`records_total`、`overlap_percent`、`library_source`；非 Linux 或拿不到挂载信息时 `library_source_known=false` 且其它字段照常。

## S5 计划可配置：改完一分钟内生效

1. 把每日时间改到「一分钟后就到」的本地时刻，并把星期改到两天后（用来确认 weekly 不会跟着立刻生成）：

```bash
AT=$(date -d '+2 minutes' +%H:%M 2>/dev/null || date -v+2m +%H:%M)
WD=$(( ($(date +%w) + 2) % 7 ))
curl -fsS -b $J -X PUT -H 'Content-Type: application/json' \
  -d "{\"snapshot_daily_at\":\"$AT\",\"snapshot_weekly_day\":$WD}" $BASE/api/settings >/dev/null
curl -fsS -b $J "$BASE/api/backups" | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d["schedule"])'
```

**期望**：`schedule.daily_at` 已经是 `$AT`、`weekly_day` 是 `$WD`，`next_daily_at` 指向今天 `$AT`（未到则今天，已过则明天）。

2. 等过那个时刻（最多 1 分钟 tick + 生成耗时），再查一次列表：

```bash
sleep 90
curl -fsS -b $J "$BASE/api/backups" | python3 -c 'import json,sys;d=json.load(sys.stdin);s=d["schedule"];print(s["last_daily_at"],s["next_daily_at"],[e["name"] for e in d["entries"][:3]])'
```

**期望**：`last_daily_at` 变成刚才那次生成的瞬时（不为 null），`next_daily_at` 变成明天同一时刻；列表里出现一条新的
`reason=daily`，且它的 `summary.archives` 等于库里档案数。**不需要重启**。

3. 关闭计划：

```bash
curl -fsS -b $J -X PUT -H 'Content-Type: application/json' -d '{"snapshot_schedule_enabled":false}' $BASE/api/settings >/dev/null
curl -fsS -b $J "$BASE/api/backups" | python3 -c 'import json,sys;s=json.load(sys.stdin)["schedule"];print(s["enabled"],s["next_daily_at"],s["next_weekly_at"])'
```

**期望**：`False None None`（两个 next 都是 null，`last_*` 保留）；此后等两分钟不再出现新的计划快照。
手动 `POST /api/backups` 仍然能生成 `manual`，`POST /api/database/clean` 仍然会先落 `pre-clean`。

4. 非法值必须被拒：

```bash
curl -sS -b $J -o /dev/null -w '%{http_code}\n' -X PUT -H 'Content-Type: application/json' \
  -d '{"snapshot_daily_at":"25:00"}' $BASE/api/settings
curl -sS -b $J -o /dev/null -w '%{http_code}\n' -X PUT -H 'Content-Type: application/json' \
  -d '{"snapshot_weekly_day":7}' $BASE/api/settings
```

**期望**：两次都是 `400`（`snapshot_daily_at` 必须是 `HH:MM` 且 00:00–23:59；`snapshot_weekly_day` 必须 0–6，0 = 周日）。

## S6 台账保留期

1. 把 `deletion_journal_days` 设为 0，触发一次删除 → 台账**不**记录（关闭台账）。
2. 设为 1，删除一条，手工把该条的 `deleted_at_ms` 改到 2 天前，运行 sweep → **期望**该条被清除且日志里有计数。

## 回归

```bash
mise run test-crate -- lanrurugi-api lanrurugi-storage lanrurugi-scanner
mise run clippy && mise run fmt-check
cd apps/frontend && npx tsc -b --force && npx eslint src/pages/Settings/Backup.tsx
```
