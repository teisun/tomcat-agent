# 工作目录与数据布局

本文为 [Architecture](../openspec/specs/Architecture.md) 中「8. 工作目录与数据布局」的详细设计，总览见主文档。参考 [openclaw 多 agent 目录约定]

## 1. 默认工作根目录（work_dir）

- **默认值**：`~/.tomcat`。
- **可配置**：支持通过主配置文件 `tomcat.config.toml` 的 `[storage].work_dir` 与环境变量 `TOMCAT__STORAGE__WORK_DIR` 覆盖。路径支持 `~` 与相对路径，经现有 `normalize_path` 展开。

## 2. 多 agent 子目录约定

均相对于 work_dir，与 openclaw 的 `agents/<agentId>/` 思路一致：

| 路径 | 说明 |
|------|------|
| `agents/<agentId>/agent/` | 该 agent 的身份与凭据目录（可配置覆盖） |
| `agents/<agentId>/sessions/` | 该 agent 的会话与 transcript（sessions.json、JSONL 等） |
| `agents/<agentId>/todos/` | 轻量待办文件 `<session_id>.todo.md`；每个 session 固定一份，不再依赖 `sessions.json.activeTodosId` |
| `agents/<agentId>/logs/` | 该 agent 的日志（per-agent，若写文件则用此路径） |
| `agents/<agentId>/audit/` | 该 agent 的审计日志（JSONL） |
| `agents/<agentId>/checkpoints/` | 该 agent 的 checkpoint 影子 Git 根目录；按 `sha256(agent_workspace_dir)` 分桶存放元数据与 git 对象库 |
| `workspace-<agentId>/` | 该 agent 的工作区（AGENTS.md 等设计态文件，根级目录，可配置覆盖） |
| `memory/` | 向量检索索引 |
| `credentials/` | OAuth 凭据 |
| `media/` | 媒体文件 |
| `subagents/` | 子 agent 注册表 |
| `plugins/` | **全局**共享插件目录（所有 agent / scope 均可发现） |
| `skills/` | **全局**共享 skill 目录（Managed 层） |
| `packages/` | **全局** package ledger 命名空间；只存 `packages/registry.json`，不存 plugin/skill 正文 |
| `assets/js/` | **全局** JS runtime/shim 资源（如 `pi_bridge.js`、`pi_crypto_shim.js`、`pi_node_shim.js` 等） |
| `assets/.env` | 敏感配置（API Key 等），`tomcat init` 生成，权限 0600，`run_cli` 启动时 dotenvy 加载 |
| `assets/.versions.json` | 内嵌资源版本记录（SHA-256 + extracted_at），`ensure_embedded_assets` 写入 |
| `assets/.lock` | 并发写入保护文件锁（fs2 exclusive lock），`ensure_embedded_assets` 使用 |

- **全局额外授权目录**：除每个 agent 默认可写的 `agent_definition_dir`（`[agent].workspace` 指向的 `workspace-{id}/` 设计态目录）外，额外允许访问的外部目录根由主配置文件 **`tomcat.config.toml` 的 `[workspace] workspace_roots`** 列出（**所有 agent 共用**，不按 agent 分文件）。启动 `tomcat chat` 时的 shell 当前目录 `agent_workspace_dir` 不会自动进入授权根；需要通过 `tomcat workspace add --cwd`、cwd lazy prompt、`/path <路径>` 命令或本会话授权显式加入。由 `tomcat workspace add/list/remove` 或手编 TOML 维护。

- **当前仅一个 agent**：agentId 固定为 `main`，即使用 `work_dir/agents/main/` 下各子目录。
- **可配置覆盖**：`agent_dir` 和 `workspace` 可通过 `[agent]` 配置节覆盖；`sessions/logs/audit/checkpoints` 始终从 `work_dir/agents/{id}/` 独立推导，不受 `agent_dir` 配置影响。

### 2.1 PackageManager 三层安装布局

`PackageManager` 只负责把资源放进既有三层根，并按层写账本；runtime 继续复用 `plugin_roots()` / `skill_roots()` 发现磁盘内容，不会把 `packages/registry.json` 当作第四套发现源。

| 可见层 | plugin 正文目录 | skill 正文目录 | package 账本 | plugin 管理账本 | 说明 |
|------|-----------------|---------------|-------------|----------------|------|
| `global` | `{work_dir}/plugins/` | `{work_dir}/skills/` | `{work_dir}/packages/registry.json` | `{work_dir}/plugins/registry.json` | 所有 scope 的兜底层 |
| `agent` | `{work_dir}/agents/<agentId>/plugins/` | `{work_dir}/agents/<agentId>/skills/` | `{work_dir}/agents/<agentId>/packages/registry.json` | `{work_dir}/agents/<agentId>/plugins/registry.json` | 当前 agent 私有层 |
| `scope` | `<scope_root>/.tomcat/plugins/` | `<scope_root>/.tomcat/skills/` | `<scope_root>/.tomcat/packages/registry.json` | `<scope_root>/.tomcat/plugins/registry.json` | 当前 project 私有层；`scope_root` 必须先 canonicalize |

补充约束：

- `packages/` 只存安装账本，不承载 plugin/skill 正文。
- `packages/registry.json` 的当前账本 schema 固定为 `tomcat.package.registry.v1`；字段定义与示例以 [package-manager.md](./package-manager.md) §5.3 为准。
- package 安装清单使用 `package.json` 顶层 `tomcat` 块，schema 默认 `tomcat.package.v1`；版本统一来自外层 `package.json.version`。
- `scope` 层目录由 install 时按需创建；`global` 与 `agent` 层目录由 `ensure_work_dir_structure()` 预创建。
- code/claw 会话里 `/install` 成功后，只刷新当前进程内 `SkillSet` 与 plugin catalog/static tool 清单；**不会**在安装路径执行插件代码，也不会热替换已加载 plugin 实例。

## 3. 启动时创建目录

- **时机**：load_config 后或 CLI/服务入口启动时。
- **行为**：创建 work_dir 及本约定中的全部子目录；若目录已存在则跳过。
- **当前**：仅支持一个 agent（agentId=`main`），至少创建：
  - `work_dir/agents/main/agent`（身份与凭据，可配置覆盖）
  - `work_dir/agents/main/sessions`
  - `work_dir/agents/main/todos`
  - `work_dir/agents/main/logs`
  - `work_dir/agents/main/audit`
  - `work_dir/agents/main/checkpoints`
  - `work_dir/agents/main/plugins`
  - `work_dir/agents/main/skills`
  - `work_dir/agents/main/packages`
  - `work_dir/workspace-main`（根级工作区，可配置覆盖）
  - `work_dir/memory`、`credentials`、`media`、`subagents`
  - `work_dir/plugins`、`work_dir/skills`、`work_dir/packages`
  - `work_dir/assets/js`（全局 runtime/shim 资源）

## 4. run_script 与临时文件

- run_script(code) 写入的 script.js 放在**当前 agent 的 tmp**，当前即 `work_dir/agents/main/tmp/`（tmp 目录保留签名兼容但不在启动时创建）。未指定 agent 时使用 `main`。
- work_dir 未初始化或不可用时，可回退到系统 temp（如 `std::env::temp_dir()`）。

## 5. 引用本设计

- [Architecture](../openspec/specs/Architecture.md) 详细设计索引已包含本文档。
- 涉及会话路径、存储根目录、配置与数据布局的文档（如 host-core-layer、session-storage、[src/infra/README.md](../../../src/infra/README.md)、design、[src/ext/README.md](../../../src/ext/README.md) 等）应在相应小节引用本文档。
