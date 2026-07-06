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

- 默认做 **patch +1**
  - CLI：`tomcat/Cargo.toml`
  - EXT：`tomcat-vscode-ext/package.json`
- EXT 必须同时更新：
  - `tomcat-vscode-ext/package-lock.json`
  - `tomcat-vscode-ext/gui/package.json`
  - `tomcat-vscode-ext/gui/package-lock.json`
  - `tomcat-vscode-ext/package.json` 里的 `tomcat.bundledCliVersion`
- **先 CLI，后 EXT**
- `develop -> main`、`develop -> master` 只能 **fast-forward**
- 发现非预期脏文件时先停下来问用户

## 1. 起手检查

先执行：

```bash
git status --short
git branch --show-current
git tag --list | tail -n 40
```

要求：

- 当前分支是 `develop`
- 工作区只有本次 release 相关改动
- 目标 tag 还不存在：
  - `cli-v<new_cli_version>`
  - `ext-v<new_ext_version>`

## 2. 改版本

默认把当前版本都做 patch `+1`。

需要修改：

- `tomcat/Cargo.toml`
- `tomcat-vscode-ext/package.json`
- `tomcat-vscode-ext/package-lock.json`
- `tomcat-vscode-ext/gui/package.json`
- `tomcat-vscode-ext/gui/package-lock.json`

EXT / GUI 推荐直接用：

```bash
cd tomcat-vscode-ext
npm version --no-git-tag-version <new_ext_version>
npm --prefix gui version --no-git-tag-version <new_ext_version>
```

然后手改：

- `tomcat/Cargo.toml` 的 CLI 版本
- `tomcat-vscode-ext/package.json` 的 `tomcat.bundledCliVersion`

## 3. 检查 Cargo.lock

CLI release workflow 用的是：

```bash
cargo build --locked --release
```

所以改完版本后必须看：

```bash
git diff -- tomcat/Cargo.lock
```

处理规则：

- 如果没变化：继续
- 如果只是根包版本跟着变了（例如 `0.1.8 -> 0.1.9`）：把它一起提交
- 如果出现其他超预期变化：停止并询问用户

## 4. 先跑 guard

```bash
node .github/scripts/release/check-cli-tag.mjs . cli-v<new_cli_version>
node .github/scripts/release/check-ext-tag.mjs . ext-v<new_ext_version>
```

guard 失败就先修版本文件，不要继续。

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
- 不要忽略 `Cargo.lock` 的合法版本漂移
- 不要把本地打包成功当成 GitHub release 完成
