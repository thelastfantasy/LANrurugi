# Phase 0 Research: 删除安全网

每个决策给出「选了什么 / 为什么 / 考虑过什么」。所有条目都已定案，无 NEEDS CLARIFICATION。

## D1. 台账挂在哪一层

**Decision**: 挂在 `crates/lanrurugi-api/src/archives.rs::purge_one_archive` —— 它是所有永久删除的唯一出口（`DeleteOneOutcome` 的调用方包括 Clean Database、单条删除、修订版取代、拆分原始档）。

**Rationale**: 逐个入口去加保护，漏一个就等于没保护；而 `purge_one_archive` 在删除前已经 `get` 到了完整记录，正好是要留档的内容。这也避免改动每个调用方的签名。

**Alternatives**: 在 `ArchiveRepository::delete` 里做（更底层，但 storage 层不该知道 backup/journal 语义，且它与 register 的重建路径耦合）；在 API 层每个 handler 里做（重复、易漏）。

## D2. 台账存储与保留

**Decision**: Redis hash `LANRURUGI_DELETION_JOURNAL`（archive DB），field = archive id，value = 紧凑 JSON `{record, deleted_at_ms, source, path}`；保留 30 天，由既有周期性 sweep 清理；另设单键上限 20,000 条（超限按 `deleted_at_ms` 从旧到新淘汰）。

**Rationale**: 删除路径上只多一次 HSET（微秒级，与删除本身同一连接），不需要新文件目录、不需要容器新挂载；记录级恢复要的是「原样记录」，JSON 直接就是。上限用来防「一次清空 10 万条」把 Redis 撑爆。

**Alternatives**: 文件式台账（每条一个 json）——大库时 inode 爆炸、扫描式列表慢；把台账写进备份文档——那要等下一次快照，救不了「刚删完就发现」。

## D3. 快照放哪

**Decision**: `<library>/.lanrurugi-backups/`，与既有 `<library>/.lanrurugi-recycle/` 同一套约定：库内点目录、扫描与 watcher 忽略、`fsutil::in_dir` 写入、临时文件 + rename 落盘。

**Rationale**: 容器部署已经挂了库目录且可写（库扫描会写 `_byname/` 与 sidecar），不需要新增卷；与回收站一致的心智模型；备份跟着库走，换机器时一起搬。

**Alternatives**: 新增 `<data>/backups`（容器需要新挂载，老部署会静默丢备份）；放进 Redis（大库几十 MB，且要跟 AOF 抢空间）。

## D4. 什么时候生成快照

**Decision**: (a) 每天首次启动时，若 `<library>/.lanrurugi-backups/` 里没有今天的 `daily` 快照；(b) 每个破坏性操作前（clean / restore / rebuild-index / 台账批量清理），在操作开始前同步写一份 `pre-*`；(c) 提供手动触发端点。

**Rationale**: 启动点覆盖「一整天没重启」以外的所有情况；操作前快照是「事后能回滚」的最后一道。同步写（在 job 线程里 `spawn_blocking`）保证操作开始时快照已落盘。

**Alternatives**: 纯定时器（进程可能整天不重启也可能频繁重启，两种都难判断「今天有没有」）；只在操作前（日常就没有基线）。

## D5. 快照保留策略

**Decision**: 7 份 `daily` + 4 份 `weekly`。每天的快照按本地日期去重（同一天只留最新一份）；`weekly` 按 ISO 周分组，每组留最新一份，最多 4 组。`pre-*` 不受这两条限制，但按最老优先只保留最近 5 份。任何情况下都不删除最新一份可用快照。

**Rationale**: 「每日 7 + 每周 4」是业界常见默认，覆盖「一周内」与「一个月内」两个典型发现窗口；按日期去重避免一天多次重启产生一堆同质快照。

**Alternatives**: 只按数量（最近 N 份）——发现得晚（>N 天）就没有基线；只按时间——数量不可控。

## D6. 删除守卫放在哪

**Decision**: 在 `clean_database` 里，**真正删除那一遍**开始前先算出计划删除量；`planned > guard_min` 且 `planned * 100 >= total * guard_percent` 时返回 409（`success: 0`，附 `planned / total / percent / threshold`），除非请求带 `force=1`。`file` 清空那一遍不受守卫限制（可逆，且记录仍在）。

**Rationale**: 守卫要拦的是「一次性带走大比例数据」，不是「删了一条」；`file` 清空是可逆的软删除，拦它反而会让正常清理走不下去。

**Alternatives**: 在 `purge_one_archive` 里拦（单条删除也会被拦，且它看不到「本次操作总量」）；只在 UI 上加确认（API 调用绕过）。

## D7. 库目录诊断能拿到什么

**Decision**: 输出四项：`scanned_files`、`records_total`、`overlap_percent`（记录中 `file` 存在的比例，逐条 `stat`，在扫描已有池里做）、`library_source`（Linux 读 `/proc/self/mountinfo` 中库路径对应挂载的 root 字段；拿不到或非 Linux 记 `unknown`）。全部只写日志与 job 结果，**不据此拒绝任何操作**。

**Rationale**: 运行时差异太大（原生安装上 mountinfo 的 root 恒为 `/`，Docker Desktop/gVisor 里可能是合成路径），把它当安全判据会既误报又漏报；作为诊断信息能显著缩短定位时间。安全判据交给 D1/D2 的台账与 D6 的守卫。

**Alternatives**: 用指纹（dev+ino）拒绝操作——跨容器/网络文件系统不稳定，会误拦正常部署。

## D8. 恢复语义

**Decision**: 恢复 = 把台账里的记录原样写回（`HSET` 全字段）；若同 id 已存在则跳过并报告；若 `file` 指向的文件不存在，记录仍然恢复，并在返回结果里标 `file_missing: true`（下次扫描会把它纳入「文件缺失」的处理，而不会删记录）。恢复后清掉台账条目。

**Rationale**: 事故现场最常见的形态就是「文件在、记录没了」或「文件也不在但稍后会挂回来」；两种都该让记录先回来。跳过已存在 id 是为了不覆盖用户这期间新建/改过的记录。

**Alternatives**: 恢复时重新扫描该文件以重建 id——会把用户改过的标签冲掉；不恢复缺失文件的记录——正好丢掉最需要救的那批。

## D9. 失败与并发

**Decision**: 台账写入失败时删除照常进行，但 `purge_one_archive` 的返回值/日志带 `journal_failed`，且 `clean_database` 的结果里计数；快照生成失败只 warning，不阻塞启动或操作，同一自然日内不重复尝试；台账到期清理与快照保留都放在既有的 sweep 里串行执行，避免与正在进行的恢复/生成竞争。

**Rationale**: 安全网本身不能成为可用性风险；但「这次删除无法撤销」必须让人看见。

**Alternatives**: 台账写失败就中止删除——用户会看到一个删不掉的档案，比丢数据更糟；重试到成功——删除路径被存储故障拖死。

## D10. 设置项

**Decision**: 新增 4 个数字设置（沿用 `NUMBER_FIELDS` 模式，默认值写在 `settings.rs`）：
`deletion_journal_days`（30）、`deletion_guard_percent`（25）、`deletion_guard_min`（50）、`snapshot_week_keep`（4）。每日保留固定 7（`snapshot_daily_keep` 也一并暴露，默认 7）。全部即时生效，无需重启。

**Rationale**: 与既有设置项一致；默认值让「不配置也安全」。
