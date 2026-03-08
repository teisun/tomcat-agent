# 工作目录与数据布局

本文为 [Architecture](../Architecture.md) 中「10. 工作目录与数据布局」的详细设计，总览见主文档。参考 [openclaw 多 agent 目录约定]

## 1. 默认工作根目录（work_dir）

- **默认值**：当前可执行文件所在目录下的 `.pi_wasm`，即 `{可执行文件目录}/.pi_wasm`。在进程启动时通过 `std::env::current_exe()` 的父目录得到 `exe_dir`，默认 `work_dir = exe_dir.join(".pi_wasm")`。
- **可配置**：支持通过配置文件（如 `storage.work_dir` 或 `app.work_dir`）和环境变量（如 `PI_AWSM__STORAGE__WORK_DIR`）覆盖。路径支持 `~` 与相对路径，经现有 `normalize_path` 展开。

## 2. 多 agent 子目录约定

均相对于 work_dir，与 openclaw 的 `agents/<agentId>/` 思路一致：

| 路径 | 说明 |
|------|------|
| `agents/<agentId>/sessions/` | 该 agent 的会话与 transcript（sessions.json、JSONL 等） |
| `agents/<agentId>/plugins/` | 该 agent 的插件目录 |
| `agents/<agentId>/tmp/` | 该 agent 的临时文件（如 run_script 写入的 script.js） |
| `agents/<agentId>/logs/` | 该 agent 的日志（per-agent，若写文件则用此路径） |
| `agents/<agentId>/wasm/` | 该 agent 的 wasm 目录（如 quickjs 缓存、该 agent 专用 wasm 资源） |
| `wasm/` | **全局** wasm/quickjs 缓存（共享的 wasmedge_quickjs.wasm 等） |

- **当前仅一个 agent**：agentId 固定为 `default`，即使用 `work_dir/agents/default/` 下各子目录。
- **与现有配置的兼容**：若已显式配置 `sessions_dir`、`plugins_dir` 等，则优先使用（可视为单 agent 或兼容模式）；否则按 work_dir + 上述多 agent 布局推导。设计文档与实现中需统一约定兼容规则。

## 3. 启动时创建目录

- **时机**：load_config 后或 CLI/服务入口启动时。
- **行为**：创建 work_dir 及本约定中的全部子目录；若目录已存在则跳过。
- **当前**：仅支持一个 agent（agentId=`default`），至少创建：
  - `work_dir/agents/default/sessions`
  - `work_dir/agents/default/plugins`
  - `work_dir/agents/default/tmp`
  - `work_dir/agents/default/logs`
  - `work_dir/agents/default/wasm`
  - `work_dir/wasm`（全局 wasm）

## 4. run_script 与临时文件

- run_script(code) 写入的 script.js 放在**当前 agent 的 tmp**，当前即 `work_dir/agents/default/tmp/`。未指定 agent 时使用 `default`。
- work_dir 未初始化或不可用时，可回退到系统 temp（如 `std::env::temp_dir()`）。

## 5. 引用本设计

- [Architecture](../Architecture.md) 详细设计索引已包含本文档。
- 涉及会话路径、存储根目录、配置与数据布局的文档（如 host-core-layer、session-storage、01-infrastructure、design、02-wasm-runtime-and-plugin 等）应在相应小节引用本文档。
