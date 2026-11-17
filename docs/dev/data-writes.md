# 修改已存储的记录

宪章 Principle III「Changing a stored record」条款的实施说明。来源：#302。

## 问题

读出整条记录 → 修改 → 整条写回，在「读」和「写」之间如果有别的写入，写回时会把对方的修改覆盖掉。
#302 里阅读器打开档案时同时发出两个请求：清除 new 标记的请求先读到旧记录，保存进度的请求写入了第 1 页，
随后清除 new 标记时整条写回，把进度改回了 0。同样的写法当时存在于约 40 处：元数据插件、全库缩略图重建、
订阅检查结束、分类和单行本的成员增删等。耗时越长的操作（插件、全库任务、订阅翻页）越容易撞上。

## 规则

| 场景 | 用法 |
|---|---|
| 修改已有的档案 / 分类 / 单行本 / 订阅 | `repo.update(id, \|r\| { … })` |
| 修改后还要同步搜索索引等派生数据 | `repo.update_then(id, \|r\| { … }, \|old, new\| async { … })` |
| 只写一个独立字段 | 专用方法：`set_progress`、`set_isnew`、`set_blake3` |
| 新建记录，或刻意整条替换（回收站恢复、旧版导入的覆盖模式） | `repo.create_or_replace(&record)` |

- `update` 在该记录的锁内重新读取最新记录，执行闭包，只写回实际变化的字段（档案），或在锁内整条写回（分类、单行本、订阅这类整条存储的记录）。
  记录不存在或已被删除时返回 `None`，不会把它写回来。
- 同一条记录的写入排队执行，不同记录之间互不等待。锁是进程内的（`lanrurugi_storage::record_lock`，基于 `lockable`）：本服务是这些记录唯一的写入方。
- **慢操作放在 `update` 之前**：网络请求、渲染、LLM 调用、订阅翻页都在锁外完成，闭包里只做「把结果合并进最新记录」。
  闭包是同步的，本身就不能 `await`。
- 闭包里的判断要基于闭包收到的最新记录，不要基于之前读到的副本。例如「只在标签为空时自动填充」要在闭包里检查 `r.tags.is_empty()`。
- 搜索索引是按「旧值 → 新值」的差量更新的，必须放在 `update_then` 的第二个参数里，在锁内按写入顺序执行。
  放在锁外的话，两次修改的索引更新可能以相反的顺序执行，索引就和记录不一致了。档案用 `archives::reindex_title_and_tags`，单行本用 `reindex_tankoubon`。

## 例子

```rust
// 错误：插件运行了几秒，期间用户改的标签会被这次写回覆盖
let mut archive = repo.get(&id).await?.unwrap();
let data = run_plugin(&archive).await?;
archive.tags = merge(&archive.tags, &data.tags);
repo.create_or_replace(&archive).await?;

// 正确：慢操作在锁外，合并基于锁内读到的最新记录，索引在锁内跟着更新
let data = run_plugin(&snapshot).await?;
repo.update_then(
    &id,
    |a| a.tags = merge(&a.tags, &data.tags),
    |old, new| reindex_title_and_tags(&state, old, new),
)
.await?;
```

## 新增一种存储记录时

在第一个写入方合并之前，先给它的仓库加上 `update`（以及需要时的 `update_then`），并让 `delete` 也拿同一把锁。

## 相关

- 宪章：`.specify/memory/constitution.md` Principle III
- 实现：`crates/lanrurugi-storage/src/record_lock.rs`，`repository.rs` 中各仓库的 `update` / `update_then`
- Redis 分库沿用 LANraragi 的布局（宪章 Principle I），跨库操作做不到原子性，所以派生数据要靠 `update_then` 在锁内保证顺序。
