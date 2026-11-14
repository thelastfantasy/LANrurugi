# Quickstart: 入库可见性门禁

**Feature**: [spec.md](./spec.md) | **Plan**: [plan.md](./plan.md) | **Contract**: [contracts/visibility-contract.md](./contracts/visibility-contract.md)

两段验收：

- **A（读侧）**：直接写 pending 键，验证搜索/阅读页在所有入口都拒绝该档案——这一段在当前代码上会失败，正是 D1–D3 的复现。
- **B（写侧）**：用一个会睡 30 秒的 metadata 插件跑一次真实上传，确认标记在 cataloguing 之前写入、在插件结束后才删除。

环境：本机 dev 容器（`mise run dev-up`），前端 `http://127.0.0.1:3000`，后端 `http://127.0.0.1:3001`，Redis 由 `lrr-dev` 以 host network 访问。

```sh
# 确认端口（dev 的 Redis 端口以容器 env 为准；本机历史值是 16379）
docker inspect lrr-dev --format '{{range .Config.Env}}{{println .}}{{end}}' | grep LANRURUGI_REDIS_URL
REDIS_PORT=16379
curl -s http://127.0.0.1:3001/api/info | jq '{version, version_name, total_archives}'
```

> 没有登录态时 `/api/search` 走 `guest_visitor`（dev 的 `LRR_CONFIG guestmode=1`）。本 quickstart 的两组断言都要在**未登录**与**登录管理员**下各跑一次。

## A. 读侧：pending 档案必须不可见、不可读

1. 选一个当前能被搜到的档案（下面用 `source:` 过滤，任意稳定的过滤都行）：

```sh
ID=$(curl -s 'http://127.0.0.1:3001/api/search?filter=source:e-hentai.org/g/4141895/6b1b9778ca&start=0' | jq -r '.data[0].arcid')
echo "$ID"
curl -s "http://127.0.0.1:3001/api/archives/$ID/metadata" | jq '{arcid, title, pagecount}'
```

2. 在 **DB 0**（archive 逻辑库）写标记，模拟「已 cataloguing、插件未结束」：

```sh
docker exec lrr-test-redis redis-cli -p "$REDIS_PORT" -n 0 \
  SADD LANRURUGI_PENDING_VISIBILITY "$ID"
docker exec lrr-test-redis redis-cli -p "$REDIS_PORT" -n 0 \
  SISMEMBER LANRURUGI_PENDING_VISIBILITY "$ID"   # 期望 1
```

3. 断言隐藏（**当前代码在 3a 会失败**，这正是缺陷）：

```sh
# 3a. 未登录 / 管理员，分组与未分组都要为空
for g in true false; do
  curl -s "http://127.0.0.1:3001/api/search?filter=source:e-hentai.org/g/4141895/6b1b9778ca&start=0&groupby_tanks=$g" \
    | jq --arg id "$ID" '{hits: ([.data[].arcid] | map(select(. == $id)) | length), filtered: .recordsFiltered}'
done
curl -s "http://127.0.0.1:3001/api/search/archives?filter=source:e-hentai.org/g/4141895/6b1b9778ca" | jq '.data | length'

# 3b. 阅读页三个入口都必须是 404
for p in metadata files "page?path=_1.jpg"; do
  curl -s -o /dev/null -w "$p -> %{http_code}\n" "http://127.0.0.1:3001/api/archives/$ID/$p"
done
```

期望：3a 全部 `hits: 0`；3b 三行都是 `404`（当前代码是 200/200/200）。

4. 删除标记，确认恢复（发布即时性）：

```sh
docker exec lrr-test-redis redis-cli -p "$REDIS_PORT" -n 0 SREM LANRURUGI_PENDING_VISIBILITY "$ID"
curl -s "http://127.0.0.1:3001/api/search?filter=source:e-hentai.org/g/4141895/6b1b9778ca" | jq --arg id "$ID" '[.data[].arcid] | index($id)'
curl -s -o /dev/null -w '%{http_code}\n' "http://127.0.0.1:3001/api/archives/$ID/metadata"   # 期望 200
```

**负向对照**：确认 3a 的空结果不是「过滤本身没命中」——先删掉标记跑同一查询，必须命中；否则这次对照无效（调查中 ID2/ID3 就是这样被误读的）。

## B. 写侧：真实上传 + 慢插件

1. 放一个临时 metadata 插件（dev 的 `plugins/` 是 bind mount，保存即生效；**不要提交**）：

```ts
// plugins/metadata/slowgate.ts（namespace 由相对路径决定，即 metadata/slowgate）
export function pluginInfo() {
  return {
    namespace: "slowgate",
    type: "metadata" as const,
    parameters: [],
    declared_permissions: { net: [], read: false, write: false },
    name: "Slow Gate Fixture",
    author: "test",
    description: "Sleeps 30s, then adds a tag.",
    version: "0.0.1",
  };
}

export async function execMetadata(_hostArgs: Record<string, unknown>) {
  await new Promise((r) => setTimeout(r, 30_000));
  return { tags: "slowgate:done" };
}
```

2. 启用它（插件启用状态在 config 库的 hash 里；namespace 以 `GET /api/plugins` 返回的为准）：

```sh
docker exec lrr-test-redis redis-cli -p "$REDIS_PORT" -n 2 \
  HSET "LRR_PLUGIN_METADATA/SLOWGATE" enabled 1   # key = LRR_PLUGIN_ + namespace 全大写；以 GET /api/plugins 的 namespace 为准
```

3. 在网页上传一个测试 zip（或 `curl -F file=@fixture.zip http://127.0.0.1:3001/api/upload`），请求会挂 30 秒以上。**请求返回前**，在另一个终端：

```sh
# 标记必须已经存在，且档案还搜不到
docker exec lrr-test-redis redis-cli -p "$REDIS_PORT" -n 0 SMEMBERS LANRURUGI_PENDING_VISIBILITY
curl -s "http://127.0.0.1:3001/api/search?filter=title:<上传文件名片段>" | jq '.data | length'   # 0
```

4. 请求返回后（响应体里的 `id` 就是 archive id）：

```sh
curl -s "http://127.0.0.1:3001/api/archives/$ID/metadata" | jq '{title, tags}'   # tags 含 slowgate:done
curl -s "http://127.0.0.1:3001/api/search?filter=title:<片段>" | jq '.data | length'  # ≥1
docker exec lrr-test-redis redis-cli -p "$REDIS_PORT" -n 0 SISMEMBER LANRURUGI_PENDING_VISIBILITY "$ID"  # 0
```

5. 清理：删除 fixture、`HDEL LRR_PLUGIN_METADATA/SLOWGATE enabled`、删除残留键。

## C. 崩溃恢复

在 B 的 30 秒窗口内 `docker kill -s KILL lrr-dev`（或 `kill -9` server），重启后：

```sh
docker exec lrr-test-redis redis-cli -p "$REDIS_PORT" -n 0 SMEMBERS LANRURUGI_PENDING_VISIBILITY  # 空
```

并在 60 秒内确认该档案可搜、metadata 200（带崩溃前已有的标签）。

## D. 自动化测试

```sh
# 单 crate，带真实 Redis（lrr-test-redis 映射在 16380）
LANRURUGI_TEST_REDIS_URL=redis://127.0.0.1:16380 mise run check-crate -- lanrurugi-search
LANRURUGI_TEST_REDIS_URL=redis://127.0.0.1:16380 mise run check-crate -- lanrurugi-api
```

完整测试按仓库约定以脱离 agent 进程树的方式跑：

```sh
setsid nohup mise run test > /tmp/ingest-visibility-test.log 2>&1 < /dev/null &
```

## 验收清单

- [ ] A3a：管理员/访客、分组/未分组、`/api/search/archives` 全部 0 命中
- [ ] A3b：metadata/files/page 全部 404
- [ ] A4：删除标记后立即恢复
- [ ] B3：上传请求进行中标记已存在、搜索 0 命中
- [ ] B4：上传返回后标签含 `slowgate:done`、搜索命中、标记已删
- [ ] C：崩溃重启后 60 秒内无残留标记、档案可见
- [ ] D：`lanrurugi-search` 与 `lanrurugi-api` 相关测试通过
