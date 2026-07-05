# tomcat

基于 Rust 的轻量 AI Agent 运行时，为作者学习 Agent 开发而作的实践项目。源码与构建在 [`tomcat/`](tomcat/) 目录。

## 特性

- **Agent 循环**：三层嵌套循环（对话管理 → 容错重试 → 思考-行动），支持 Steering / FollowUp / Abort、上下文压缩（Compaction）与限流退避
- **多模型 LLM**：OpenAI Chat Completions、OpenAI Responses 等管线；`models.toml` 目录与凭据解析；流式输出与 Thinking 展示
- **内置工具**：`read` / `write` / `edit` / `bash` / `search_files` 等原语，以及 plan、todos、checkpoint、web 等扩展工具；权限门控（PermissionGate）与路径规则
- **会话与状态**：`sessions.json` + JSONL transcript；Checkpoint 影子 Git；PLAN 模式与 plan runtime
- **CLI 对话**：无子命令默认进入 `chat`；会话 / 配置 / 工作区 / 审计

## 快速开始

优先入口：

- **VS Code 插件（推荐）**：从 GitHub Release 下载平台对应的 bundled `.vsix`，安装后直接在 Chat 里用 `@tomcat`。包怎么选、怎么装，见 [`tomcat-vscode-ext/README.md`](tomcat-vscode-ext/README.md)。
- **CLI**：见 **[用户使用说明](tomcat/docs/user-guide.md)**，覆盖前置依赖、构建、`init` / `doctor` / `chat`、会话与工作区、配置、审计及集成测试等完整步骤与示例输出。

简要前提：Rust stable 1.70+；OpenAI 兼容 API 密钥（`tomcat/.env.example` → `tomcat/.env` 中的 `OPENAI_API_KEY`）。运行态数据默认在 `~/.tomcat/`，目录布局见 [工作目录与数据布局](tomcat/docs/architecture/work-dir-and-data-layout.md)。

## 项目结构

```text
tomcat/
├── src/
│   ├── api/              # CLI：init、doctor、config、session、chat、plugin、audit …
│   ├── core/             # 宿主核心（仅可信 Rust）
│   ├── ext/              # 扩展能力（建设中）
│   └── infra/            # 配置、日志、审计、事件总线、错误、平台 IO
└── docs/
    ├── openspec/         # 宪法、架构索引、开发与测试规范
    ├── agents/           # 角色卡、任务看板、计划模板
    └── architecture/     # 各子系统设计
```

模块级说明见 [tomcat/src/README.md](tomcat/src/README.md)。

## 架构

自下而上单向依赖，宏观分层与 [Architecture.md](tomcat/docs/openspec/specs/Architecture.md) 一致：

```text
基础设施层 (infra)
    ↑
宿主核心能力层 (core) — 会话、LLM、Agent Loop、Compaction、工具、权限、Checkpoint、Plan
    ↑
交互层 (api) — CLI
```

一次对话的主路径：**CLI `chat`** → **SessionManager** 加载 transcript → **AgentLoop** 调用 **LlmProvider** 流式推理 → 按需执行内置工具 → 写回 transcript / 审计。全貌见 [项目全貌](tomcat/docs/architecture/project-overview-panorama.md)。

## 文档入口

- [tomcat/docs/README.md](tomcat/docs/README.md) — 文档地图
- [tomcat/src/README.md](tomcat/src/README.md) — `src/` 模块索引与分层图

## 许可

本项目采用 [MIT License](LICENSE)。
