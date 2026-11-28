# 发布新版本

`dev` 是日常开发分支，`main` 是发布分支。发布就是把 `dev` 合进 `main`，再在 `main` 上打 tag。

## 步骤

1. **冻结 `dev`**：这次要发布的功能 PR 都合进 `dev`。
2. **处理 Dependabot**：把 `dev` 上开着的 dependabot PR 处理掉。
   - 例行检查每周一次（`.github/dependabot.yml` 的 `interval: weekly`），实际落在周五 10:20 UTC 左右。发布前想拿到最新结果，可以在仓库 Insights → Dependency graph → Dependabot 里对各生态点 “Check for updates”。
   - 改动 `dependabot.yml` 并合进 `dev` 后，几分钟内会重新扫描一次，可能马上开出新 PR。
   - 主版本升级单独成 PR，合并前先读它的 breaking changes，确认项目没有用到被删的选项或规则。
   - 多个 PR 改同一个 lockfile 时依次合并；冲突的那个留言 `@dependabot rebase` 让它自己重建。
3. **改版本号**：开一个 PR 到 `dev`，改三处：
   - `Cargo.toml` 的 `[workspace.package] version`；
   - `Cargo.lock` 里所有 `lanrurugi-*` 包的版本；
   - `apps/docs-site/public/versions.json` 开头加一条（`version`、`tag`、`date`、`prerelease`）。文档站构建时会按 git tag 重新生成这个文件，这里改的是仓库里的副本。

   改完版本号后不再往 `dev` 合并别的东西，否则就要回到第 2 步。
4. **`dev` → `main` 的 PR**：标题 `release: <版本号>`，正文列出这次合入 `dev` 的 PR。CI 全部通过后合并。
   - 推送到 `main` 时 `ci.yml` 会发布三个 Cloudflare Pages 站点：docs（`docs.lrrg.dev`）、API（`api.lrrg.dev`）、SDK（`sdk.lrrg.dev`）。docs 的最新版本放在根路径，`/v<版本号>/` 301 到根路径的同一页；旧版本在 `/v<旧版本号>/` 下，最近几个是静态文件，更早的由 Pages Function 从 R2 里的发布 zip 读出（#305）。
   - 这时还没有 tag，但构建时会把正在构建的版本放进版本列表，「版本历史」页和版本选择器都已经有新版本。
5. **打 tag**：在 `main` 的合并提交上打 `v<版本号>` 并推送：

   ```sh
   git fetch origin
   git tag v1.2.3 origin/main
   git push origin v1.2.3
   ```

   `release.yml` 会先检查 tag 和 `Cargo.toml` 版本一致，不一致直接失败。之后构建镜像和各平台安装包，全部成功后才发布 Release。带预发布后缀（如 `-beta.7`）的版本推到镜像的 `beta` tag，Release 标为 pre-release；正式版推到 `latest`。
6. **发布后检查**：
   - GitHub Release：`SHA256SUMS`、`docs-<版本号>.zip`、四个平台的 `lanrurugi-<版本号>-*.tar.gz` 都在（`release.yml` 最后一步也会核对）；
   - 镜像：`ghcr.io/thelastfantasy/lanrurugi` 和 Docker Hub 上有 `<版本号>` 和 `beta`/`latest`；
   - **第一个正式版（1.0）**：`latest` 这时才第一次出现。在 `latest` 能拉取之后，把文档里暂时写成 `:beta` 的示例改回 `:latest`：三份 README 的「快速开始」（并删掉旁边的注释）、文档站中英文 `guide/install.mdx`（取消 stable 那段的注释，放回 beta 前面作为默认页）和 `guide/reverse-proxy.md`；
   - 文档站：`docs.lrrg.dev/` 是新版本（版本选择器选中它），`/v<新版本号>/` 跳到根路径，上一个版本在 `/v<上一版本号>/` 可以打开；API 和 SDK 站点显示新版本号。

## 出问题时

- **release 失败**：失败前什么都不会公开。修好后（如需改代码，走 `dev` → `main`），删掉 tag 重新在新的 `main` 提交上打：

  ```sh
  git push origin :refs/tags/v1.2.3
  git tag -f v1.2.3 origin/main
  git push origin v1.2.3
  ```

- **忘了改版本号就打了 tag**：`release.yml` 第一步会失败。删 tag，补第 3 步，再从第 4 步继续。
