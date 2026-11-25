# Contract: 资源接口

两条路由，都在 `/api` 挂载，都只对管理员会话开放。

| 方法 | 路径 | 用途 | 观察者 |
|---|---|---|---|
| GET | `/api/resources` | 一次性快照（导出、反馈包、curl 排查） | 不注册 |
| GET | `/api/resources/stream` | SSE：首帧完整快照，之后每秒一帧样本 | 注册，断开即注销 |

## 权限

`crates/lanrurugi-api/policy/route_policy.csv` 新增：

```
# 030: process/memory detail — a real admin login only, never an API token or a guest.
p, token_admin, /api/resources/*, *, deny
p, token_guest, /api/resources/*, *, deny
p, guest_visitor, /api/resources/*, *, deny
```

理由与 018 的 `/api/diagnostics/*` 一致：这类数据能反推出宿主结构，不能给可长期保存的 API token，也不能给访客。

## GET /api/resources

200：

```json
{
  "snapshot": { "…": "见 resources-snapshot.md" }
}
```

503 只在采集整体失败（`/proc` 不可读且有 `portable` 也拿不到）时返回，body `{"error": "resources unavailable: …"}`。

## GET /api/resources/stream

`Content-Type: text/event-stream`。帧类型：

```
event: snapshot
data: {"…完整 ResourceSnapshot…"}

event: sample
data: {"…Sample…"}

: keep-alive
```

- 首帧在订阅后 ≤ 200 ms 内发出（若采样任务刚启动则是首帧立即采的样本）。
- 之后每 `interval_ms`（默认 1000，允许 500–5000）一帧；客户端可用 `?interval_ms=` 覆盖。
- 客户端断开 → 服务端在 ≤ 2 s 内注销观察者；归零则终止采样任务（`contract` 的可测点）。
- 服务端每 15 s 发一次 `KeepAlive` 注释帧。

## 不会发生的事

- 不会因为轮询而累积多个采样任务：观察者计数为原子的，`attach` 只在 0→1 时 spawn。
- 不会在租约外采样：`GET /api/resources` 只在请求期间采集一次，不写历史。
- 不会把租约与用户会话绑定：租约是连接级的，页面刷新即断开重建。
