---
name: /bump-cli-ext-version
id: bump-cli-ext-version
category: Workflow
description: 使用项目脚本升级 CLI 与插件版本，默认 patch +1，同步内置 CLI；只改版本和校验，不构建、不提交、不发布
---

# 升级 CLI 与插件版本号

执行本 command 时，使用仓库现有的 `scripts/release-version.mjs`，默认将 CLI 和插件的 **patch 版本各加 1**，同时让插件内置 CLI 版本指向升级后的 CLI。

这不是完整发版：不要自动调用 `/release-cli-ext` 或 `/commit-with-status`。

```text
读取当前版本与改动
        |
        v
版本一致性检查 -> 升级一次 -> 再检查 -> 汇报前后版本
        |
        +-- 失败：停止，报告原因，不继续递增
```

## 1. 确认仓库与当前状态

- 所有命令在仓库根目录执行，不依赖固定的本机路径。确认根目录包含 `scripts/release-version.mjs` 和 `release-versions.json`。
- 查看当前分支与改动，保留用户已有的工作，不要求切换到 `develop`，不暂存、不回滚、不清理文件：

```sh
git status --short
git branch --show-current
git diff -- release-versions.json tomcat/Cargo.toml tomcat/Cargo.lock tomcat-vscode-ext/package.json tomcat-vscode-ext/package-lock.json
git diff --cached -- release-versions.json tomcat/Cargo.toml tomcat/Cargo.lock tomcat-vscode-ext/package.json tomcat-vscode-ext/package-lock.json
```

- 读取 `release-versions.json`，记录升级前的 `cli`、`extension.version` 和 `extension.bundledCli`，不要硬编码版本号。
- 与版本无关的未提交改动不阻塞本操作。如果版本文件已经有未提交的升级，先判断是否就是本次请求已经完成的结果；如果不清楚用户是否要再升一次，先确认，避免重复递增。
- 默认使用 `patch`。只有用户明确指定 `minor` 或 `major` 时，才替换下方命令中的升级级别。

## 2. 使用项目脚本升级一次

按以下顺序执行，前一步成功才继续：

```sh
node scripts/release-version.mjs check &&
node scripts/release-version.mjs bump --all patch &&
node scripts/release-version.mjs check
```

- `--all` 同时升级 CLI 和插件，并同步 `extension.bundledCli`；不要拆成两次独立升级。
- 只让项目脚本更新版本，不手改 JSON/TOML，不运行 `npm version`，不重新生成依赖锁文件。
- 初始 `check` 失败时停止并报告不一致的文件，不自动执行 `sync` 覆盖用户正在编辑的版本。
- 升级或后续检查失败时，先查看当前文件与脚本输出；即使要重试校验，也不要重新运行 `bump`。它会再次增加版本号，不是可安全重复的检查命令。

## 3. 核对结果

```sh
git diff -- release-versions.json tomcat/Cargo.toml tomcat/Cargo.lock tomcat-vscode-ext/package.json tomcat-vscode-ext/package-lock.json
git diff --check -- release-versions.json tomcat/Cargo.toml tomcat/Cargo.lock tomcat-vscode-ext/package.json tomcat-vscode-ext/package-lock.json
git status --short
```

相对执行前的状态，本次只应修改以下五个文件的版本字段：

- `release-versions.json`：CLI、插件、内置 CLI 三个版本。
- `tomcat/Cargo.toml`：`[package].version`。
- `tomcat/Cargo.lock`：`tomcat` 根包版本。
- `tomcat-vscode-ext/package.json`：`version` 与 `tomcat.bundledCliVersion`。
- `tomcat-vscode-ext/package-lock.json`：顶层与根包版本。

依赖版本、业务代码和私有 GUI 的版本字段不属于本次修改范围。发现本操作新增了其他变化时，停止调查，不自动回滚用户的改动。

## 4. 汇报并结束

汇报 CLI、插件、内置 CLI 的「旧版本 -> 新版本」、实际修改文件，以及版本检查和空白检查的结果。失败或未运行的步骤如实说明，不把升级版本号等同于安装或发布成功。

默认到此结束：**不跑测试套件，不编译、不打包、不安装、不重启、不暂存、不提交、不推送、不打 tag、不创建 release**。如需这些操作，由用户另行明确要求。
