# 案例：CLI 子命令补完（TASK-02 / T1-P0-010-completion）

以下是一份符合 [PLAN_SPEC.md](./PLAN_SPEC.md) 的**完整开发计划**（历史输出），形态为「多子命令、占位渐进替换」——**同类新任务请复制结构、替换任务名与路径即可**，不必保留 `pi-wasm` 等旧二进制名：当前仓库 CLI 入口以 crate 配置为准（常见为 `pi` / `cargo run -p pi_wasm -- …`）。

- **更小任务**：优先用 [PLAN_SKELETON.md](./PLAN_SKELETON.md) 一屏骨架。  
- **TASK-02 已在看板 DONE**：本文仅作章节与粒度示范，子项编号以当时 TASK_BOARD 为准。

### 规范映射

| 规范条目 | 案例中对应章节 |
| :--- | :--- |
| 1. 待完成子项清单 | 一、待完成子项清单 |
| 2. 目标与验收 / 用户故事 | 二、目标与验收 + 各步骤的用户故事 |
| 3. 每子项详细（文件/思路/接口/测试） | 三、各子项详细计划 |
| 4. 实施顺序与依赖 | 四、实施顺序与依赖关系 |
| 5. 风险点 | 五、风险点与可能的阻塞项 |
| 7. Todo 总表 + 写后复核 | **〇、Todo 与章节映射**（[PLAN_SPEC.md](./PLAN_SPEC.md) 一.7、第六节） |

---

## 〇、Todo 与章节映射（与计划描述一致）

| Todo / 子项 | 类型 | 对应计划章节 |
| :--- | :--- | :--- |
| `ops-claim`～`develop` 同步、`feature/...` | 流程 | Dispatcher；本案例省略细节，真实任务须写明 |
| 10.3 doctor | 实施 | 三、子项 1 |
| 10.4 config | 实施 | 三、子项 2 |
| 10.6 plugin | 实施 | 三、子项 3 |
| 10.7 audit | 实施 | 三、子项 4 |
| 10.8 帮助与校验 | 实施 | 三、子项 5 |
| 每子项完成 → status / commit / push | 流程 | Dispatcher §5；与 TASK-20 类计划中 `ops-after-each-phase` 同级 |
| 收尾 → 门禁 → PENDING_INTEGRATION | 流程 | Dispatcher §7、INTEGRATION_MERGE_AND_ACCEPTANCE |

**写后复核**：确认上表每一行在「三、各子项详细计划」中有展开；「一」中清单与上表实施类一一对应。

---

## 一、待完成子项清单

对照 [TASK_BOARD.md](../TASK_BOARD.md) 中 TASK-02 的子项清单：

- 10.1 CLI 骨架（clap 子命令结构） — 已完成
- 10.2 `pi-wasm init`：引导 LLM 配置、生成配置文件 — 已完成
- **10.3 `pi-wasm doctor`**：补全 WasmEdge/QuickJS 可用性检测（当前为占位 `println`）
- **10.4 `pi-wasm config`**：补全 `get key`、`set`、`edit`（当前为占位 `println`）
- 10.5 `pi-wasm session`：list/new/switch/delete/archive/search — 已完成
- **10.6 `pi-wasm plugin`**：list/load/unload/enable/disable/info（当前 6 个子命令全为占位）
- **10.7 `pi-wasm audit`**：list/show/export（当前 3 个子命令全为占位）
- **10.8 完善帮助文档与参数校验**

## 二、目标与验收

**要做出什么**：将 `pi-wasm` CLI 中仍为占位的 5 组子命令（doctor 检测、config set/edit、plugin 6 个、audit 3 个、帮助文档）补充为真实可执行的实现，使用户可通过命令行完成环境诊断、配置修改、插件管理、审计日志查看的全部操作。

**验收标准**：

- `pi-wasm doctor` 能实际检测 WasmEdge/QuickJS 可用性并输出修复建议
- `pi-wasm config set/edit` 能真实修改配置文件
- `pi-wasm plugin list/load/unload/enable/disable/info` 能实际对接 PluginManager
- `pi-wasm audit list/show/export` 能读取已有审计日志
- 所有子命令帮助文档完整、参数校验正确
- 首次运行无配置时的提示友好
- `cargo test -j 1 --all -- --test-threads=1` 全部通过，门禁（rustfmt/clippy/单测）通过

### 各步骤的用户故事、作用与意义

**10.3 doctor — WasmEdge/QuickJS 检测**

- **用户场景**：用户初次安装 pi-wasm 或换机后执行 `pi-wasm doctor`，想确认运行环境是否齐备。
- **作用**：尝试初始化 WasmEngine 并检查 QuickJS wasm 文件路径，输出可用/不可用状态与修复建议。
- **意义**：若不做，用户在 `plugin load` 时才会遇到 WasmEdge 错误，报错信息缺乏引导，排查成本高。

**10.4 config set/edit — 配置修改能力**

- **用户场景**：用户想快速改一个配置项（如 `log.level` 改为 `debug`），不想手动找文件路径和编辑 TOML 语法。
- **作用**：`set` 按 key 路径定位并写入新值后持久化；`edit` 启动系统编辑器打开配置文件。
- **意义**：若不做，用户只能手动定位配置文件并编辑，易出格式错误且无校验。

**10.6 plugin — 插件管理**

- **用户场景**：用户下载了一个插件，想通过 `pi-wasm plugin load ./my-plugin` 加载并验证。
- **作用**：将 6 个占位命令对接 PluginManager 的 API。
- **意义**：若不做，插件系统无法通过 CLI 操作，用户没有入口加载和管理插件。

**10.7 audit — 审计日志查看**

- **用户场景**：运维或用户想追溯"上次 bash 执行了什么命令"，通过 `pi-wasm audit list` 查看近期审计记录。
- **作用**：读取 tracing 日志中的审计记录行，解析并格式化输出；支持导出为 JSON。
- **意义**：若不做，审计记录只在 tracing 日志中，用户需自行在庞大日志中人肉筛选，实用性为零。

**10.8 帮助文档与参数校验**

- **用户场景**：用户输入 `pi-wasm plugin --help` 或误输入参数时，需要看到清晰的帮助信息。
- **作用**：补全所有子命令的 clap 文档注释和参数校验逻辑。
- **意义**：若不做，用户面对简陋的帮助文本和无校验的输入，体验差、易犯错。

## 三、各子项详细计划

> 以下展示**子项 1（10.3 doctor）** 的完整写法作为模板，其余子项结构相同。

### 子项 1：10.3 doctor — 补全 WasmEdge/QuickJS 检测（完整示例）

#### 涉及的文件与模块

- **改动**：`src/api/cli.rs` — `run_doctor` 函数
- **依赖模块**：`ext::WasmEngine`（`WasmEngine::global`）、`infra::config`（`WasmConfig`）
- **参考文档**：`openspec/changes/001-mvp/tasks_details.md` T1-P0-010 第 10.3 项

#### 实现思路（调用链路）

```
run_doctor
  ├─ [已有] load_config → validate_config → ensure_work_dir_structure
  ├─ [新增] check_wasmedge():
  │    └─ WasmEngine::global(Some(WasmEngineConfig { quickjs_path: cfg.wasm.quickjs_path.clone(), ..default }))
  │         ├─ Ok(_) → 输出 "✓ WasmEdge 运行时：可用"
  │         └─ Err(e) → 输出 "✗ WasmEdge 运行时：不可用 ({e})"
  │                      + "修复建议：请安装 WasmEdge，参考 https://wasmedge.org/docs/start/install"
  └─ [新增] check_quickjs():
       └─ 从 cfg.wasm.quickjs_path 或 std::env::var("WASMEDGE_QUICKJS_PATH") 取路径
            ├─ Some(p) 且 Path::new(p).exists() → 输出 "✓ QuickJS 运行时：可用 ({p})"
            ├─ Some(p) 但不存在 → "✗ QuickJS wasm 文件不存在: {p}"
            └─ None → "✗ QuickJS 路径未配置" + 修复建议
```

#### 依赖的现有接口 / 需新建的接口

- **已有 API**：
  - `WasmEngine::global(config: Option<WasmEngineConfig>) -> Result<Arc<Self>, AppError>`
  - `WasmEngineConfig { quickjs_path: Option<String>, .. }`
  - `load_config`, `validate_config`, `ensure_work_dir_structure`
- **需新建**：无新 pub API。仅在 `run_doctor` 内部新增 `check_wasmedge` 和 `check_quickjs` 两个私有辅助函数。

#### 预期的测试要点

- **正常路径**：提供合法配置 + WasmEngine 可初始化 → 函数返回 Ok，输出包含检测结果
- **边界 1：首次运行无配置文件**：输出"未找到配置文件。请先运行: pi-wasm init"，不崩溃
- **边界 2：WasmEdge 不可用**：输出包含修复建议 URL，不 panic
- **边界 3：quickjs_path 配置了但文件不存在**：输出"不存在"提示和修复建议
- **边界 4：quickjs_path 和环境变量均未设置**：输出"未配置"提示
- **期望错误表现**：所有检测失败均输出清晰建议，函数始终返回 Ok（doctor 是诊断工具）

---

### 子项 2：10.4 config — 补全 get(key) / set / edit

- **涉及文件**：`src/api/cli.rs` — `run_config` 函数
- **实现思路**：`get` 按 "." 路径逐层取 TOML 值；`set` 按路径写入并做类型推断（原值类型优先）→ validate_config → write_file_atomic；`edit` 启动 `$EDITOR` 或回退 vi/notepad
- **需新建**：`resolve_toml_key`、`set_toml_key`、`config_file_path`（cli.rs 内私有）
- **测试要点**：正常读写、不存在的 key、非法值被 validate_config 拒绝、配置文件不存在时提示 init、edit 后配置不合法时输出警告

### 子项 3：10.6 plugin — 6 个子命令对接 PluginManager

- **涉及文件**：`src/api/cli.rs` — `run_plugin` 函数
- **实现思路**：新增 `build_plugin_context()` 构建 PluginManager + EventBus + WasmEngine + ToolRegistry；list/load/unload/enable/disable/info 分别调用 PluginManager 对应 API；load 成功后立即输出插件详情
- **需新建**：`PluginContext`、`build_plugin_context`、`cli_confirm_permissions`、`format_plugin_info`
- **测试要点**：load 成功/路径不存在/WasmEngine 不可用/重复加载/不存在的 ID/用户拒绝权限

### 子项 4：10.7 audit — 对接现有审计日志

- **涉及文件**：`src/api/cli.rs` — `run_audit` 函数
- **实现思路**：读取 tracing 日志文件，按 "audit primitive"/"audit tool_call"/"audit hostcall" 关键字过滤，解析为 `AuditDisplayEntry`；list 取最近 N 条、show 按行号、export 为 JSON
- **需新建**：`AuditDisplayEntry`、`parse_audit_line`、`read_audit_entries`
- **测试要点**：file_enabled=false 提示、日志不存在、无审计行、export 写入与反序列化

### 子项 5：10.8 完善帮助文档与参数校验

- **涉及文件**：`src/api/cli.rs` — 所有 clap 结构体的 `///` 注释和 `#[arg]` 属性
- **实现思路**：为各子命令补全 clap 文档注释和参数 help，移除"占位"字样
- **测试要点**：`--help` 输出完整、现有解析测试不受影响

## 四、实施顺序与依赖关系

```
子项1（doctor）──────────────┐
                              ├─→ 子项3（plugin）──┐
子项2（config）──────────────────────────────────────┼─→ 子项5（帮助文档）
                                                     │
子项4（audit）───────────────────────────────────────┘
```

- 子项 1（doctor）与子项 2（config）可并行
- 子项 3（plugin）在子项 1 之后（复用 WasmEngine 初始化经验）
- 子项 4（audit）可独立进行
- 子项 5（帮助文档）在 1-4 全部完成后

每个子项完成后独立提交。

## 五、风险点与可能的阻塞项

1. **WasmEdge 环境依赖** — 开发机可能未安装 WasmEdge C 库。**降级**：doctor/plugin load 输出安装指引；测试仅测 Err 路径。
2. **PluginManager 无跨进程持久化** — CLI 每次调用是独立进程。**降级**：load 成功后立即输出详情；其余子命令提示"持久化管理将在对话模式中支持"。
3. **审计日志依赖 tracing 文件输出** — P0 阶段审计记录混在通用日志中。**降级**：file_enabled=false 时给出明确提示；解析做宽松匹配；T1-P1-001 后可平滑替换。
4. **config set 类型推断** — 嵌套表结构与值类型可能不准确。**降级**：基于原值类型推断，写入前经 validate_config 校验，不合法则拒绝。
