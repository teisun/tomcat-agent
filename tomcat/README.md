# tomcat

基于 Rust 的轻量 AI Agent 运行时

## 特性

- **Agent 循环**：三层嵌套循环（对话管理 → 容错重试 → 思考-行动），支持 Steering / FollowUp / Abort、上下文压缩（Compaction）与限流退避
- **多模型 LLM**：OpenAI Chat Completions、OpenAI Responses 等管线；`models.toml` 目录与凭据解析；流式输出与 Thinking 展示
- **内置工具**：`read` / `write` / `edit` / `bash` / `search_files` 等原语，以及 plan、todos、checkpoint、web 等扩展工具；权限门控（PermissionGate）与路径规则
- **会话与状态**：`sessions.json` + pi-mono 相容 JSONL transcript；Checkpoint 影子 Git；PLAN 模式与 plan runtime
- **CLI 对话**：无子命令默认进入 `chat`；会话 / 配置 / 工作区 / 审计 / 插件管理
- **插件（可选）**：WasmEdge 沙箱、Hostcall 分发、长生命周期 VM Actor；对齐 pi-mono ExtensionAPI（需 `wasmedge` feature）

## 快速开始

### 前置依赖

- Rust 1.70+（推荐 stable）
- OpenAI 兼容 API 密钥（复制 `.env.example` 为 `.env` 并填写 `OPENAI_API_KEY`）
- WasmEdge C 库 0.13.5（仅 `--features wasmedge` 时需要）

### 安装 WasmEdge（可选，真实 Wasm 模式）

```bash
bash scripts/install-wasmedge.sh -y
source $HOME/.wasmedge/env
```

### 构建与运行

```bash
cargo build --release                              # 默认：无 Wasm，宿主 + CLI
# cargo build --release --features wasmedge        # 需预装 WasmEdge C 库
# cargo build --release --features standalone      # 构建时自动拉取并链接 WasmEdge

./target/release/tomcat init                     # 初始化配置与工作目录
./target/release/tomcat doctor                   # 环境自检
./target/release/tomcat chat                     # 交互对话（无子命令时同此）
./target/release/tomcat session list             # 会话列表
```

工作区与运行态数据默认落在 `~/.tomcat/`（会话、日志、审计、插件等），见 [工作目录与数据布局](docs/architecture/work-dir-and-data-layout.md)。

### 运行测试

```bash
# 需已配置 .env
RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh integration

# Wasm / JS API 对齐（需 wasmedge feature 构建）
# cargo build --release --features wasmedge
# RUST_LOG=tomcat=debug,info ./scripts/run-integration-tests.sh integration-wasm
```

## CLI 子命令

| 命令 | 说明 |
|------|------|
| `chat` | 交互式对话（默认入口） |
| `init` | 生成配置、引导模型与安全策略 |
| `doctor` | 环境与配置检查 |
| `config` | `get` / `set` / `edit` 配置项 |
| `session` | `list` / `new` / `switch` / `delete` / `archive` / `search` |
| `workspace` | 授权工作区 `add` / `list` / `remove` |
| `pathrules` | 路径规则 `add` / `list` |
| `plugin` | 插件 `list` / `load` / `unload` / `enable` / `disable` / `info`（Wasm 模式） |
| `audit` | 审计 `list` / `show` / `export` |

## 项目结构

```text
src/
├── api/              # CLI：init、doctor、config、session、chat、plugin、audit …
├── core/             # 宿主核心（仅可信 Rust）
│   ├── agent_loop/   # Agent 循环、工具分发、流式与错误分类
│   ├── llm/          # 多 Provider、模型目录、系统提示、流事件
│   ├── session/      # SessionManager、JSONL transcript、上下文指标
│   ├── compaction/   # 上下文压缩与预热
│   ├── checkpoint/   # 影子 Git checkpoint / restore
│   ├── plan_runtime/ # PLAN 模式与 todos 落盘
│   ├── tools/        # 内置工具 catalog、原语执行器、plan 工具
│   └── permission/   # PermissionGate、path_rule
├── ext/              # Wasm 引擎、Hostcall、插件管理（wasmedge feature）
└── infra/            # 配置、日志、审计、事件总线、错误、平台 IO

docs/
├── openspec/         # 宪法、架构索引、开发与测试规范
├── agents/           # 角色卡、任务看板、计划模板
├── architecture/     # 各子系统设计（Agent Loop、会话、插件、工具 …）
└── …                 # 进度 status、报告、用户指南
```

模块级说明见 [src/README.md](src/README.md)。

## 架构

自下而上单向依赖，宏观分层与 [Architecture.md](docs/openspec/specs/Architecture.md) 一致：

```text
基础设施层 (infra)
    ↑
宿主核心能力层 (core) — 会话、LLM、Agent Loop、Compaction、工具、权限、Checkpoint、Plan
    ↑
宿主 API 层 — Hostcall / ExtensionAPI（ext，Wasm 插件唯一入口）
    ↑
WasmEdge 运行时 + 沙箱执行层（可选，插件 JS/TS）
    ↑
交互层 (api) — CLI
```

一次对话的主路径：**CLI `chat`** → **SessionManager** 加载 transcript → **AgentLoop** 调用 **LlmProvider** 流式推理 → 按需执行内置工具或插件 Hostcall → 写回 transcript / 审计。全貌见 [项目全貌](docs/architecture/project-overview-panorama.md)。

## 文档入口

- [docs/README.md](docs/README.md) — 文档地图
- [src/README.md](src/README.md) — `src/` 模块索引与分层图
- [docs/INTEGRATION.md](docs/INTEGRATION.md) — 集成进度看板
- [docs/user-guide.md](docs/user-guide.md) — 使用说明

## 规范文档

- [Constitution.md](docs/openspec/specs/Constitution.md) — 项目宪法
- [Product_Brief.md](docs/openspec/specs/Product_Brief.md) — 定位与路线图（P0–P9）
- [编码与架构规范](docs/openspec/specs/guides/coding/Codeing&Architecture_Spec.md)
- [单元测试规范](docs/openspec/specs/guides/testing/UNIT_TEST_SPEC.md)
- [集成测试规范](docs/openspec/specs/guides/testing/INTEGRATION_TEST_SPEC.md)

## 许可

本项目采用 [MIT License](LICENSE)。
