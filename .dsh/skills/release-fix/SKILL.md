---
name: "release-fix"
description: "Fix a shipped release: read the user's bug report plus rustrak/Sentry data, land every fix in one PR on a branch off the latest dev, bump the version and docs, merge dev into main, then re-release and verify the artifacts."
metadata:
  author: "LANrurugi"
  source: "session lessons — beta.5 shipped with the wrong image tag"
---

# release-fix

把「用户报的线上问题 + rustrak 里的真实错误」变成一次可验证的发版。所有修复集中在
一个 PR 里，版本号必须跟着改，最后一定验证产物真的存在。

## 0. 先读这些

- `CLAUDE.md`：中文回答、`mise run check` 才能提交、`--no-verify` 禁止、重构建用
  `setsid nohup … & disown`。
- `~/.dsh/notes/lanrurugi-ops.md`：NAS / rustrak 的连接方式与踩坑记录。
- `.github/workflows/release.yml`：**镜像 tag 取自 `Cargo.toml` 的 `version`，不是 git tag**。
  这是本 skill 存在的原因（beta.5 的镜像被发成 `1.0.0-beta.4`，还覆盖了 beta.4 的镜像）。

## 1. 收集要修什么

1. 用户这条消息里点名的问题（`$ARGUMENTS`）。
2. rustrak：`list_projects` → `list_issues`（按 `filter=open`、最近的 issue 优先）→
   `get_issue` / `list_events` 看堆栈。只把**能复现、有堆栈或明确路径**的收进本轮，
   猜测性的修复写进回复而不是塞进 PR。
3. 本地复现：`127.0.0.1:3000` 是 dev 容器（HMR，前端改动即时生效；Rust 改动要
   `mise run dev-rebuild`）。

## 2. 开分支（必须在最新 dev 上）

```sh
git fetch origin --quiet
git checkout -q dev && git pull -q --ff-only origin dev
git checkout -b fix/<slug>
```

共享工作区里可能有别人未提交的改动——**先 `git status`**，不要顺手把它们的文件提交进去
（曾经把提交误落到 `feat/telemetry-install-detail` 分支上）。

## 3. 修

- 一个 PR、一个分支，所有修复一起做（用户明确要求省 CI 时间）。
- Rust 改完先 `mise run check-crate -- <crate>`，前端改完 `pnpm -C apps/frontend run lint`。
- 修完跑 `mise run check`（rust-check + frontend-lint），失败就修，绝不 `--no-verify`。
- 测试对旧语义的假设也要一起改：beta.3 的自愈修复让扫描会剪掉「映射指向不存在档案」的
  条目，旧测试用全零占位 id 就必然失败（PR #254）。

## 4. 版本号与文档（容易被忘的一步）

1. `Cargo.toml` 的 `[workspace.package] version`（第 33 行附近）改成新版本，
   `cargo check`/`mise run check-crate` 会顺带刷新 `Cargo.lock`（26 处引用）。
2. 只在**真的要发版**时改；改完再跑一遍 check。
3. 文档：`README.md` 的功能条目、`docs/comparison-with-lanraragi.md`、
   `docs/` 里版本相关页面。没有用户可感知的变化就不用动。

## 5. 合到 dev，再合到 main

```sh
git push origin fix/<slug>
gh pr create --base dev --head fix/<slug> --title "..." --body "...Closes #N..."
gh pr checks <pr>            # 全绿再合
gh pr merge <pr> --merge

git checkout dev && git pull --ff-only origin dev
gh pr create --base main --head dev --title "release: <version>" --body "..."
gh pr merge <pr> --merge      # 这一步会跑完整测试
```

`mergeStateStatus: BLOCKED` 而所有 check 都 pass 时（`Docs (build + publish all three)` 这类
只在 tag 上跑的 job 被 SKIPPED 会让 GitHub 认为必需检查未满足），可以用 `--admin` 合并；
本仓库此前多次这样做过。CI 卡在 `Install libarchive build dependencies` 的 apt 步骤时也一样。

合并 main 之后 **必须**把 dev 快进到 main：

```sh
git checkout dev && git merge --ff-only origin/main && git push origin dev
```

## 6. 发版

```sh
git tag -a v<version> -m "LANrurugi <version>" <main-sha>
git push origin v<version>
sleep 60 && gh run list --workflow=release.yml --limit 1 \
  --json databaseId,status,headSha --jq '.[] | "\(.databaseId) \(.status) \(.headSha[0:7])"'
```

运行约 45 分钟，jobs：`docs`、`frontend, docs, plugins`、`linux-x86_64`、`linux-aarch64`、
`macos-arm64`、`freebsd-x86_64`、`docker-amd64`、`docker-arm64`、`amend-builds`。

## 7. 验证（不验证就等于没发）

```sh
gh release view v<version> --json assets --jq '.assets[] | "\(.name) \(.size)"'
scripts/verify-release.sh <version>

for t in <version> beta <version>-<short-sha>; do
  printf "%-22s " "$t"
  podman manifest inspect ghcr.io/thelastfantasy/lanrurugi:$t >/dev/null 2>&1 && echo OK || echo missing
done
podman manifest inspect ghcr.io/thelastfantasy/lanrurugi:<version> \
  | python3 -c "import json,sys; d=json.load(sys.stdin); print([(m['platform']['os'],m['platform']['architecture']) for m in d['manifests']])"

curl -s -o /dev/null -w "%{http_code}\n" https://docs.lrrg.dev/v<version>/
```

必须全部成立：6 个产物（docs zip、4 个平台 tar.gz、SHA256SUMS）、`:latest` **不存在**
（预发布）、版本号镜像存在且是 amd64+arm64、docs 200。

### 镜像 tag 错了怎么救（本次真实事故）

镜像 tag 来自 `Cargo.toml`，所以版本号没 bump 时整批镜像会发到**上一个**版本号上（并覆盖它）：

```sh
gh release delete v<version> --yes --cleanup-tag
git push origin :refs/tags/v<version>
# 在 dev 上 bump Cargo.toml → PR → main → 快进 dev
git tag -a v<version> -m "..." <new-main-sha> && git push origin v<version>
```

被覆盖掉的旧版本镜像**无法恢复**，重跑一次发布即可（tag 可以重打，因为 Release 已删除）。

## 8. 收尾

- 更新 NAS（`app.image.op.check_update` → `app.pull_images` → job 报 "App redeployed
  successfully" → 启动应用），然后确认 filemap 没被清空、档案数没掉。
- 回复用户时给：改了什么、验证数据（版本号/平台列表/HTTP 状态）、以及**没做**的部分。
