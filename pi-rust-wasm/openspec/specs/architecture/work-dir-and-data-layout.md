# 工作目录与数据布局

本文为 [Architecture](../Architecture.md) 中「10. 工作目录与数据布局」的详细设计，总览见主文档。参考 [openclaw 多 agent 目录约定]

## 1. 默认工作根目录（work_dir）

- **默认值**：当前可执行文件所在目录下的 `.pi_`，即 `{可执行文件目录}/.pi_`。在进程启动时通过 `std::env::current_exe()` 的父目录得到 `exe_dir`，默认 `work_dir = exe_dir.join(".pi_")`。
- **可配置**：支持通过主配置文件 `pi.config.toml`（键如 `storage.work_dir` 或 `app.work_dir`）和环境变量（如 `PI_WASM__STORAGE__WORK_DIR`）覆盖。路径支持 `~` 与相对路径，经现有 `normalize_path` 展开。

## 2. 多 agent 子目录约定

均相对于 work_dir，与 openclaw 的 `agents/<agentId>/` 思路一致：

| 路径 | 说明 |
|------|------|
| `agents/<agentId>/agent/` | 该 agent 的身份与凭据目录（可配置覆盖） |
| `agents/<agentId>/sessions/` | 该 agent 的会话与 transcript（sessions.json、JSONL 等） |
| `agents/<agentId>/logs/` | 该 agent 的日志（per-agent，若写文件则用此路径） |
| `agents/<agentId>/audit/` | 该 agent 的审计日志（JSONL） |
| `workspace-<agentId>/` | 该 agent 的工作区（AGENTS.md 等设计态文件，根级目录，可配置覆盖） |
| `memory/` | 向量检索索引 |
| `credentials/` | OAuth 凭据 |
| `media/` | 媒体文件 |
| `subagents/` | 子 agent 注册表 |
| `plugins/` | **全局**共享插件目录（所有 agent 均可加载） |
| `assets/wasm/` | **全局** wasm 运行时引擎（共享的 wasmedge_quickjs.wasm 等） |
| `assets/modules/` | **全局** JS 兼容模块 |

- **当前仅一个 agent**：agentId 固定为 `main`，即使用 `work_dir/agents/main/` 下各子目录。
- **可配置覆盖**：`agent_dir` 和 `workspace` 可通过 `[agent]` 配置节覆盖；`sessions/logs/audit` 始终从 `work_dir/agents/{id}/` 独立推导，不受 `agent_dir` 配置影响。

## 3. 启动时创建目录

- **时机**：load_config 后或 CLI/服务入口启动时。
- **行为**：创建 work_dir 及本约定中的全部子目录；若目录已存在则跳过。
- **当前**：仅支持一个 agent（agentId=`main`），至少创建：
  - `work_dir/agents/main/agent`（身份与凭据，可配置覆盖）
  - `work_dir/agents/main/sessions`
  - `work_dir/agents/main/logs`
  - `work_dir/agents/main/audit`
  - `work_dir/workspace-main`（根级工作区，可配置覆盖）
  - `work_dir/memory`、`credentials`、`media`、`subagents`
  - `work_dir/plugins`（全局共享插件）
  - `work_dir/assets/wasm`、`assets/modules`（全局资源）

## 4. run_script 与临时文件

- run_script(code) 写入的 script.js 放在**当前 agent 的 tmp**，当前即 `work_dir/agents/main/tmp/`（tmp 目录保留签名兼容但不在启动时创建）。未指定 agent 时使用 `main`。
- work_dir 未初始化或不可用时，可回退到系统 temp（如 `std::env::temp_dir()`）。

## 5. 引用本设计

- [Architecture](../Architecture.md) 详细设计索引已包含本文档。
- 涉及会话路径、存储根目录、配置与数据布局的文档（如 host-core-layer、session-storage、[docs/technical/01-infrastructure.md](../../../docs/technical/01-infrastructure.md)、design、[docs/technical/02-wasm-runtime-and-plugin.md](../../../docs/technical/02-wasm-runtime-and-plugin.md) 等）应在相应小节引用本文档。
