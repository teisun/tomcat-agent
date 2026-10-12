---
name: /release-cli-ext
id: release-cli-ext
category: Workflow
description: 递增 CLI/EXT 版本，推 develop/main/master，先发 CLI 再发 EXT
---

# CLI / EXT 发版

这个 command 用于当前仓库的一次标准发版。目标顺序固定：

```text
改版本
  ->
提交并推 develop
  ->
同步 main / master
  ->
打 cli tag，等 CLI release 资产就绪
  ->
打 ext tag，等 EXT release 资产就绪
```

## 核心规则

- `release-versions.json` 是唯一可以手改的版本来源，记录三个独立事实：
  - `cli`：CLI 发布版本
  - `extension.version`：EXT 发布版本
  - `extension.bundledCli`：该 EXT 实际内置的 CLI 版本
- `tomcat/Cargo.toml`、`tomcat/Cargo.lock`、EXT `package.json` / `package-lock.json` 都是命令生成的镜像，禁止逐个手改。
- 私有 GUI 不独立发布，因此 GUI manifest 和 lockfile 没有 release version。
- 默认标准发版执行 **CLI + EXT patch +1**，并让新 EXT pin 新 CLI；仍然坚持 **先发布 CLI、资产就绪后再发布 EXT**。
- `develop -> main`、`develop -> master` 只能 **fast-forward**。
- 发现非预期脏文件时先停下来问用户。

```text
release-versions.json
├─ cli ───────────────> Cargo.toml + Cargo.lock
├─ extension.version ─> EXT package.json + package-lock.json
└─ extension.bundledCli
                        └> EXT package.json tomcat.bundledCliVersion
```

## 1. 起手检查

先执行：

```bash
git status --short
git branch --show-current
git tag --list | tail -n 40
node scripts/release-version.mjs check
```

要求：

- 当前分支是 `develop`
- 工作区只有本次 release 相关改动
- 当前所有版本镜像一致
- 目标 tag 还不存在：
  - `cli-v<new_cli_version>`
  - `ext-v<new_ext_version>`

## 2. 用一条命令改版本

标准 CLI + EXT patch 发版只执行：

```bash
node scripts/release-version.mjs bump --all patch
```

它会同时修改根版本源以及 Cargo/npm 必要镜像，并把新 EXT 的 bundled CLI pin 指向新 CLI。不要再运行 `npm version`，也不要手改 Cargo/npm 镜像。

少数独立发版场景使用：

```bash
node scripts/release-version.mjs bump --cli patch
node scripts/release-version.mjs bump --extension patch
```

CLI-only 和 EXT-only 都会保留当前 `extension.bundledCli`，不会偷偷改变已经定义好的 VSIX 内容。需要指定精确值时使用：

```bash
node scripts/release-version.mjs set \
  --cli <new_cli_version> \
  --extension <new_ext_version> \
  --bundled-cli <cli_version_bundled_by_the_new_ext>
```

改变 bundled CLI 会改变 VSIX 内容，因此同一次操作必须提供一个新的 EXT 版本。

## 3. 审核生成结果并做只读检查

执行：

```bash
git diff -- release-versions.json tomcat/Cargo.toml tomcat/Cargo.lock \
  tomcat-vscode-ext/package.json tomcat-vscode-ext/package-lock.json
node scripts/release-version.mjs check
```

预期 diff 只能包含三个业务版本及其必要镜像。Cargo.lock 只能改变唯一 `tomcat` 根包版本，EXT lock 只能改变顶层和根包版本；出现依赖变化就停止调查。

如果维护者选择直接编辑 `release-versions.json`，随后运行：

```bash
node scripts/release-version.mjs sync
node scripts/release-version.mjs check
```

`sync` 会先验证所有输入、在内存中计算全部目标，再写文件；输入错误时不会写任何目标。修正根清单或坏镜像后重新运行即可，不要用手改多个镜像“恢复”。

## 4. 再跑发版 guard

```bash
node .github/scripts/release/check-cli-tag.mjs . cli-v<new_cli_version>
node .github/scripts/release/check-ext-tag.mjs . ext-v<new_ext_version>
```

guard 会再次先检查根清单、Cargo/npm 镜像和 bundled CLI pin，再检查 tag；失败就回到版本命令修复，不要继续。

## 5. 提交并推 develop

把所有 release 文件一起提交。

推荐 commit message：

```text
chore(release): bump cli to <new_cli_version> and ext to <new_ext_version>
```

然后：

```bash
git push origin develop
```

## 6. 同步 main 和 master

只允许 fast-forward：

```bash
git switch main
git merge --ff-only origin/develop
git push origin main

git switch master
git merge --ff-only origin/develop
git push origin master

git switch develop
```

如果不能 FF，停止并问用户，不要自己改成 merge commit 或 force push。

## 7. 先打 CLI tag

```bash
git tag cli-v<new_cli_version>
git push origin cli-v<new_cli_version>
```

然后等 GitHub release 资产真正出来。至少要看到：

- `SHA256SUMS`
- `tomcat-cli-v<new_cli_version>-aarch64-apple-darwin.tar.gz`
- `tomcat-cli-v<new_cli_version>-x86_64-apple-darwin.tar.gz`
- `tomcat-cli-v<new_cli_version>-x86_64-unknown-linux-gnu.tar.gz`

推荐检查：

```bash
gh release view cli-v<new_cli_version> --repo teisun/tomcat-agent --json url,assets
```

CLI 资产没出来之前，**禁止**打 EXT tag。

## 8. 再打 EXT tag

确认 CLI 资产就绪后再执行：

```bash
git tag ext-v<new_ext_version>
git push origin ext-v<new_ext_version>
```

然后等 EXT release 资产就绪。至少要看到：

- `SHA256SUMS`
- `tomcat-vscode-ext-<new_ext_version>-darwin-arm64.vsix`
- `tomcat-vscode-ext-<new_ext_version>-darwin-x64.vsix`
- `tomcat-vscode-ext-<new_ext_version>-linux-x64.vsix`
- `tomcat-vscode-ext-<new_ext_version>.vsix`

推荐检查：

```bash
gh release view ext-v<new_ext_version> --repo teisun/tomcat-agent --json url,assets
```

## 9. 收尾汇报

最后至少汇报：

- release commit SHA
- `develop` / `main` / `master` 已推送
- CLI release URL
- CLI 资产列表
- EXT release URL
- EXT 资产列表

## 不要做的事

- 不要 force push
- 不要在 CLI release 资产没出来前先打 EXT tag
- 不要手改 Cargo/npm 版本镜像；只操作根清单或版本命令
- 不要漏提交命令生成的 Cargo.lock / package-lock 版本变化
- 不要把本地打包成功当成 GitHub release 完成
