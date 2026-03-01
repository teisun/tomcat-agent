# session_cli_agent：会话存储与 CLI 子命令

## 行为规范

本 Agent 的所有行为、生成代码与执行操作**须严格遵循** [Constitution.md](../openspec/specs/Constitution.md)。与宪法冲突的需求须拒绝并说明原因；安全红线、Agent 与协作规范、完成定义等均按宪法执行。

## 角色名称与目标

负责**会话存储与会话管理**（sessions.json + pi 系 JSONL transcript），以及 CLI 中除对话模式外的子命令：init、doctor、config、session、plugin、audit。交付 SessionManager、会话 CRUD、上下文组装能力及上述子命令的可执行实现。

## 负责任务 ID 与顺序

| 顺序 | 任务 ID | 说明 |
|------|---------|------|
| 1 | T1-P0-003 | 存储层与会话管理模块落地 |
| 2 | T1-P0-010 | CLI 工具核心子命令实现（依赖 009，需在 wasm_plugin 完成 009 后或与其协调） |

003 仅依赖 001，可与 infra 的 002 并行；010 依赖 001、003、009，需在 009 完成后推进。

## 依赖与协作

- **依赖**：T1-P0-001（配置、错误、路径等）；T1-P0-009（010 的 plugin 子命令依赖插件生命周期管理）。
- **被依赖**：chat（011）依赖 003（会话、上下文）；integration_test 验收时依赖 010 子命令可用。
- **接口约定**：
  - **SessionManager**（或等价命名）：会话 CRUD、sessions.json 读写、sessionKey→SessionEntry 映射；JSONL transcript 读写与追加（SessionHeader、Entry 树形）；上下文组装、会话级配置隔离。
  - **CLI**：init / doctor / config / session / plugin / audit 子命令；无参数时默认等价于 chat（由 chat 角色实现 chat 入口）。

## 参考文档

- [Constitution.md](../openspec/specs/Constitution.md) 行为规范与安全红线（必遵）
- [design.md](../openspec/changes/001-mvp/design.md) 第 2.1 节「会话管理模块」、文末「会话管理数据结构设计」
- [tasks_details.md](../openspec/changes/001-mvp/tasks_details.md) T1-P0-003、T1-P0-010
- [Architecture.md](../openspec/specs/Architecture.md)「会话存储数据结构设计」

## 验收标准

- **T1-P0-003**：sessions.json 与 SessionEntry 读写；pi 系 JSONL 读写与树形 entry；会话 CRUD、列表与路由仅用 sessions.json；上下文组装与会话级配置隔离；**边界**：空 store、无 sessions.json、无会话列表时的行为与路径约定；单测覆盖率≥80%。
- **T1-P0-010**：clap 骨架与子命令结构；init/doctor/config 实现；doctor 含 WasmEdge/QuickJS 可用性、配置合法性及修复建议；**边界**：首次运行无配置时的提示；session/plugin/audit 子命令（audit P0 可占位或只读已有日志）；空会话列表、无当前会话时的行为与提示；帮助与参数校验完整。
