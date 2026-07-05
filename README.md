# tomcat

基于 Rust 的轻量 AI Agent 运行时与 VS Code 扩展，为作者学习 Agent 开发而作的实践项目：[`tomcat/`](tomcat/) 提供运行时、CLI 与 `tomcat serve --stdio`，[`tomcat-vscode-ext/`](tomcat-vscode-ext/) 提供 VS Code 里的 Tomcat Agent Box。

![Tomcat Agent Box screenshot](assets/tomcat-agent-box.png)

## 特性

- **Tomcat Agent Box**：主推 VS Code 二级侧边栏里的对话面板，支持多会话切换、`Chat/Plan` 模式、模型切换、附件与上下文水位；装好 bundled `.vsix` 就能直接使用。
- **自主 Agent 循环**：三层嵌套循环（对话管理 -> 容错重试 -> 思考-行动），支持 Steering / FollowUp / Abort，长对话自动做上下文压缩（Compaction）与限流退避。
- **稳健的代码读写**：`read` / `write` / `edit` / `list_dir` 等原语之外，还提供 `hashline_edit` 行锚点编辑；`search_files` 同时支持系统 `rg` / `fd` 与进程内回退实现，统一遵循忽略规则。
- **命令执行与后台任务**：`bash` 走权限门控；长任务可后台运行，再用 `task_output` / `task_stop` / `task_list` 跨轮驱动，不必把整个会话卡死在单条命令上。
- **联网检索**：`web_search` 归一化多个搜索后端，`web_fetch` 抓取网页并转成 Markdown，同时对私网 / 环回 / 带凭据 URL 做前置拦截。
- **Plan、Todos 与澄清问题**：`create_plan` / `update_plan` / `todos` / `ask_question` 让长任务可以先出计划、再执行、再追踪。
- **Skills 与插件扩展**：支持按名加载 Skills；插件系统使用进程内 `rquickjs`，敏感能力统一走 `pi.*` hostcall，可同时扩展 LLM 工具与宿主扩展点。
- **多模型与安全审计**：支持 OpenAI Chat Completions、OpenAI Responses 等管线；`models.toml` 管理模型目录与凭据；PermissionGate、Checkpoint 影子 Git、JSONL transcript 与审计日志保证可控可回溯。
- **终端 CLI**：无子命令默认进入 `chat`，覆盖 `init` / `doctor` / 会话 / 配置 / 工作区 / 审计等完整工作流。

## 快速开始

优先入口：

- **VS Code 插件（推荐）**：从 GitHub Release 下载平台对应的 bundled `.vsix`，安装并 Reload VS Code。然后按 `Cmd/Ctrl+Shift+P` 运行 `Tomcat: Focus Agent Box` 打开 **Tomcat Agent Box**；也可以先打开右侧二级侧边栏（Secondary Side Bar）再点击 Tomcat Agent Box 图标。首次使用若看到提示，点击 `Start Setup` 让 VS Code 帮你跑 `tomcat init`。包怎么选、怎么装，见 [`tomcat-vscode-ext/README.md`](tomcat-vscode-ext/README.md)。
- **CLI**：见 **[用户使用说明](tomcat/docs/user-guide.md)**，覆盖前置依赖、构建、`init` / `doctor` / `chat`、会话与工作区、配置、审计及集成测试等完整步骤与示例输出。

通用前提：需要一个 OpenAI 兼容 API 密钥；无论通过 VS Code 插件还是 CLI 使用，首次运行都可通过 `tomcat init`（或在 VS Code 里点击 `Start Setup`）完成配置，Key 会写入 `~/.tomcat/assets/.env` 并在启动时自动加载。运行态数据默认在 `~/.tomcat/`，目录布局见 [工作目录与数据布局](tomcat/docs/architecture/work-dir-and-data-layout.md)。

仅从源码编译时才需要 Rust stable 1.70+；仓库内的 `tomcat/.env` 仅用于本地/CI 测试夹具，不是终端用户的配置路径。

## 项目结构

| 组件 | 作用 | 主要文档 |
| --- | --- | --- |
| [`tomcat/`](tomcat/) | Rust Agent 运行时、CLI 与 `tomcat serve --stdio` | [用户使用说明](tomcat/docs/user-guide.md)、[tomcat/src/README.md](tomcat/src/README.md) |
| [`tomcat-vscode-ext/`](tomcat-vscode-ext/) | VS Code 扩展与 Tomcat Agent Box | [tomcat-vscode-ext/README.md](tomcat-vscode-ext/README.md) |

```text
tomcat/
├── src/
│   ├── api/              # CLI、serve --stdio、config、session、plugin、audit …
│   ├── core/             # Agent Loop、LLM、会话、工具、权限、Checkpoint、Plan
│   ├── ext/              # 插件与扩展能力
│   └── infra/            # 配置、日志、审计、事件总线、错误、平台 IO
└── docs/
    ├── openspec/         # 宪法、架构索引、开发与测试规范
    ├── agents/           # 角色卡、任务看板、计划模板
    └── architecture/     # 运行时、工作目录与子系统设计
```

```text
tomcat-vscode-ext/
├── src/                  # 扩展宿主：serve 桥接、webview provider、typed 协议
├── gui/                  # Tomcat Agent Box 前端（React + Vite）
└── docs/architecture/    # 扩展架构设计
```

模块级说明见 [tomcat/src/README.md](tomcat/src/README.md)；扩展安装与使用见 [tomcat-vscode-ext/README.md](tomcat-vscode-ext/README.md)。

## 架构

自下而上单向依赖，宏观分层与 [Architecture.md](tomcat/docs/openspec/specs/Architecture.md) 一致：

```text
基础设施层 (infra)
    ↑
宿主核心能力层 (core) — 会话、LLM、Agent Loop、Compaction、工具、权限、Checkpoint、Plan
    ↑
交互层 (api) — CLI `chat` + `serve --stdio`（供 VS Code Tomcat Agent Box）
```

两条主要入口共用同一套运行时内核，差别只在交互层：

```text
VS Code Tomcat Agent Box
    -> tomcat serve --stdio
    -> AgentLoop
    -> LlmProvider
    -> 工具执行 / transcript / 审计

CLI chat
    -> SessionManager
    -> AgentLoop
    -> LlmProvider
    -> 工具执行 / transcript / 审计
```

全貌见 [项目全貌](tomcat/docs/architecture/project-overview-panorama.md)。

## 文档入口

- [tomcat/docs/README.md](tomcat/docs/README.md) — 文档地图
- [tomcat-vscode-ext/README.md](tomcat-vscode-ext/README.md) — VS Code 扩展（Tomcat Agent Box）安装与使用
- [tomcat/src/README.md](tomcat/src/README.md) — `src/` 模块索引与分层图

## 许可

本项目采用 [MIT License](LICENSE)。
