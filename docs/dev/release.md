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
   - `docs/public/versions.json` 开头加一条（`version`、`tag`、`date`、`prerelease`）。文档站构建时会按 git tag 重新生成这个文件，这里改的是仓库里的副本。

   改完版本号后不再往 `dev` 合并别的东西，否则就要回到第 2 步。
4. **`dev` → `main` 的 PR**：标题 `release: <版本号>`，正文列出这次合入 `dev` 的 PR。CI 全部通过后合并。
   - 推送到 `main` 时 `ci.yml` 会发布三个 Cloudflare Pages 站点：docs（`docs.lrrg.dev`，本版本放在 `/v<版本号>/`）、API（`api.lrrg.dev`）、SDK（`sdk.lrrg.dev`）。
   - 这时还没有 tag，所以 docs 的「版本历史」页还不会列出新版本（页头的版本选择器按已部署的目录生成，不受影响），第 6 步补上。
5. **打 tag**：在 `main` 的合并提交上打 `v<版本号>` 并推送：

   ```sh
   git fetch origin
   git tag v1.2.3 origin/main
   git push origin v1.2.3
   ```

   `release.yml` 会先检查 tag 和 `Cargo.toml` 版本一致，不一致直接失败。之后构建镜像和各平台安装包，全部成功后才发布 Release。带预发布后缀（如 `-beta.7`）的版本推到镜像的 `beta` tag，Release 标为 pre-release；正式版推到 `latest`。
6. **重新发布文档站**：release 完成后，在 Actions 里打开第 4 步合并到 `main` 那次的 CI，重跑 “Docs (build + publish all three)” job（`gh run rerun <run-id> --job <job-id>`）。重跑时会重新 checkout、拿到新 tag，「版本历史」页才会出现新版本。
7. **发布后检查**：
   - GitHub Release：`SHA256SUMS`、`docs-<版本号>.zip`、四个平台的 `lanrurugi-<版本号>-*.tar.gz` 都在（`release.yml` 最后一步也会核对）；
   - 镜像：`ghcr.io/thelastfantasy/lanrurugi` 和 Docker Hub 上有 `<版本号>` 和 `beta`/`latest`；
   - 文档站：docs 的版本选择器里有新版本、首页跳到正确的版本，API 和 SDK 站点显示新版本号。

## 出问题时

- **release 失败**：失败前什么都不会公开。修好后（如需改代码，走 `dev` → `main`），删掉 tag 重新在新的 `main` 提交上打：

  ```sh
  git push origin :refs/tags/v1.2.3
  git tag -f v1.2.3 origin/main
  git push origin v1.2.3
  ```

- **忘了改版本号就打了 tag**：`release.yml` 第一步会失败。删 tag，补第 3 步，再从第 4 步继续。
