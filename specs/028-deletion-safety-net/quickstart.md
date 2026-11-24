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

## S3 备份（全量 RDB）与导出（JSON）

先记住两类东西的区别：`kind=backup` 是 Redis 的全量 RDB（只能运维替换还原），`kind=export` 是可在 app 内恢复的 JSON。

1. 重启实例后检查默认计划（**开箱只有每周一 03:00 一次，且计划生成的是 RDB 快照**）：

```bash
curl -fsS -b $J "$BASE/api/backups" | python3 -m json.tool | head -50
```

**期望**：`schedule.cron` 为 `0 0 3 * * 1`、`preset` 为 `weekly`，`timezone` 形如 `Asia/Tokyo (+09:00)`，
`next_runs` 有 10 个秒级 ISO 时刻，第一个是下一个周一 03:00:00。默认没有 `last_fire_at`（还没到过命中点），
也不会因为重启就补发一份（计划改用 cron 后不再「启动即生成」，见 `S5` 第 6 步）。
之后当 cron 命中时会生成 `kind=backup`、`reason=scheduled`、`snapshot-<时间戳>-scheduled.rdb`，
`summary` 为 `null`。若 Redis 在别的机器上（或托管实例禁用了 `CONFIG`），计划生成会失败——日志里是明确的
「cannot be backed up this way」，不会产生空文件；此时请改用 `POST /api/backups/export` 或在 Redis 主机上做 RDB。

2. 立刻手动生成一次，确认两类都能落盘（快照 = 全量 RDB，导出 = JSON）。两个端点都是**异步**的：返回 `job_id`，
进度走 SSE：

```bash
JOB=$(curl -fsS -b $J -X POST "$BASE/api/backups" | python3 -c 'import json,sys;print(json.load(sys.stdin)["job_id"])')
curl -N -sS -b $J "$BASE/api/backups/jobs/$JOB/events"      # event: progress … event: done
curl -fsS -b $J "$BASE/api/backups/jobs/$JOB" | python3 -m json.tool   # 兜底轮询：state/percent/entry

EJOB=$(curl -fsS -b $J -X POST "$BASE/api/backups/export" | python3 -c 'import json,sys;print(json.load(sys.stdin)["job_id"])')
curl -N -sS -b $J "$BASE/api/backups/jobs/$EJOB/events"
```

**期望**：SSE 先收到若干 `event: progress`（`percent` 单调不减、带 `bytes_written/bytes_expected/elapsed_ms`），
最后是 `event: done`（`percent=100`，带 `entry`），随后连接关闭；RDB 的 entry 是
`backup snapshot-<时间戳>-manual.rdb`，导出的是 `export export-<时间戳>-manual.json`。
两者都能被 `GET /api/backups/<name>` 原样下载（导出可加 `?format=zip`，得到只含该 JSON 的 zip；`.rdb` 即使带
`format=zip` 也返回单文件）。空闲时流里会出现 `: keep-alive` 注释行。

3. **快照不能一键还原、导出可以**（恢复也是异步任务，进度按「已应用档案数 / 总数」）：

```bash
curl -sS -b $J -o /dev/null -w '%{http_code}\n' -X POST "$BASE/api/backups/<snapshot-name>/restore"   # 409
RJOB=$(curl -fsS -b $J -X POST "$BASE/api/backups/<export-name>/restore" | python3 -c 'import json,sys;print(json.load(sys.stdin)["job_id"])')
curl -N -sS -b $J "$BASE/api/backups/jobs/$RJOB/events"   # progress … done(result.restored/skipped)
```

**期望**：第一条 `409`（`reason=full_backup_not_restorable`，提示「停 Redis → 替换 RDB → 启动」）；第二条
返回 `job_id` + `events`，流里 `progress.percent` 按「已应用档案数 / 总数」递增，最后 `done` 的 `result`
带 `restored`/`skipped` 等计数，并额外生成一份 `pre-restore` **导出**。
真正还原快照由运维执行：下载 `.rdb`、停 Redis、用它替换 `CONFIG GET dir` + `dbfilename` 指向的文件、启动 Redis。

4. pin：`curl -fsS -b $J -X POST "$BASE/api/backups/<name>/pin"` → `{"pinned":true}`，列表里该条 `pinned=true`；
`-X DELETE .../pin` 取消。pin 之后无论保留策略怎么算都不会删它（下一步可验证）。

5. 保留策略：导出类造 10 份日期不同的 `export-*-daily.json`，跑一次保留清理 → 只剩最近 **7** 份；快照类造 5 份
`snapshot-*.rdb` → 只剩最近 **3** 份，且最新一份一定在。被 pin 的那份无论如何都在。

## S4 库目录诊断

```bash
curl -fsS -b $J -o /dev/null -X POST "$BASE/api/shinobu/rescan"; sleep 20
curl -fsS -b $J "$BASE/api/jobs" | python3 -c 'import json,sys;d=json.load(sys.stdin);print(d["jobs"][0]["result"])'
```

**期望**：结果里含 `scanned_files`、`records_total`、`overlap_percent`、`library_source`；非 Linux 或拿不到挂载信息时 `library_source_known=false` 且其它字段照常。

## S5 计划可配置：cron 表达式，改完一个 tick 内生效

1. 先看默认计划（默认只有每周一 03:00:00）：

```bash
curl -fsS -b $J "$BASE/api/backups" | python3 -c 'import json,sys;print(json.load(sys.stdin)["schedule"])'
```

**期望**：`cron` 为 `0 0 3 * * 1`、`preset` 为 `weekly`、`timezone` 是形如 `Asia/Tokyo (+09:00)` 的字符串、
`next_runs` 是 10 个秒级 ISO 时刻（都带本地偏移），且第一个是「下一个周一 03:00:00」。

2. 保存前先预览（这一步不写设置）：

```bash
curl -fsS -b $J -X POST -H 'Content-Type: application/json' \
  -d '{"cron":"*/10 * * * * *"}' $BASE/api/backups/schedule/preview
```

**期望**：`{"valid": true, "next_runs": [...10 个]}`，第一个是 10 秒内的整十秒。
再试一个坏的：`-d '{"cron":"61 0 3 * * 1"}'` → `valid: false`，`error` 含 `seconds (segment 1)` 与 `0-59`；
`-d '{"cron":"0 3 * * 1"}'` → `valid: false`，`error` 说需要 6 或 7 段。

3. 换成一个「每分钟都跑」的表达式，确认一个 tick 内生效、并且真的按秒命中：

```bash
curl -fsS -b $J -X PUT -H 'Content-Type: application/json' \
  -d '{"snapshot_cron":"*/30 * * * * *","snapshot_cron_preset":"custom"}' $BASE/api/settings >/dev/null
sleep 40
curl -fsS -b $J "$BASE/api/backups" | python3 -c 'import json,sys;d=json.load(sys.stdin);s=d["schedule"];print(s["cron"],s["last_fire_at"],[e["name"] for e in d["entries"][:3]])'
```

**期望**：`cron` 已经是新表达式，`last_fire_at` 不为 null（40 秒内至少命中过一次），列表里出现 `kind=backup`、
`reason=scheduled`、`snapshot-<时间戳>-scheduled.rdb` 的文件。**不需要重启。**

4. 非法表达式保存必须被拒：

```bash
curl -sS -b $J -o /dev/null -w '%{http_code}\n' -X PUT -H 'Content-Type: application/json' \
  -d '{"snapshot_cron":"0 0 24 * * 1"}' $BASE/api/settings
```

**期望**：`400`，错误信息里能看出是**第三段（小时）**越界（`hours (segment 3)`）。

5. 关掉计划：

```bash
curl -fsS -b $J -X PUT -H 'Content-Type: application/json' -d '{"snapshot_schedule_enabled":false}' $BASE/api/settings >/dev/null
curl -fsS -b $J "$BASE/api/backups" | python3 -c 'import json,sys;s=json.load(sys.stdin)["schedule"];print(s["enabled"],s["next_runs"])'
```

**期望**：`False []`（`last_fire_at` 保留）；此后不再出现新的计划快照。
手动 `POST /api/backups`（RDB）与 `POST /api/backups/export`（JSON）仍然能用，`POST /api/database/clean` 仍然会先落 `pre-clean` 导出。

6. 重启与停机补一次（手工验证）：把 `last_cron_fire` 用 `redis-cli HSET` 改到一天前、表达式设成每分钟，重启进程 →
**期望** 启动后只多出**一份**计划快照，随后按 cron 继续；再重启一次且不改记录 → **不应**多出同一次命中的第二份。

## S6 台账保留期

1. 把 `deletion_journal_days` 设为 0，触发一次删除 → 台账**不**记录（关闭台账）。
2. 设为 1，删除一条，手工把该条的 `deleted_at_ms` 改到 2 天前，运行 sweep → **期望**该条被清除且日志里有计数。

## 回归

```bash
mise run test-crate -- lanrurugi-api lanrurugi-storage lanrurugi-scanner
mise run clippy && mise run fmt-check
cd apps/frontend && npx tsc -b --force && npx eslint src/pages/Settings/Backup.tsx
```
