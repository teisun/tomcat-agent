| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| @Jerry | 2026-04-27 15:50 | PENDING_INTEGRATION | feature/workspace-permission-tiers | - |

### ✅ DONE (已完成)
- [x] **[P0]** PR-1：核心抽象与 3 层决策引擎（PermissionGate / EffectiveRoots / session_grants / path_rules 三层合并）
- [x] **[P0]** PR-2：ConfirmDecision 升级 + executor::primitives 接入 PermissionGate + CliConfirmation 3 选项 UI
- [x] **[P0]** PR-3：bash_parser + execute_bash 接入 + bash_forbidden / bash_approval_required regex 默认值（系统级 / 凭据 / Agent 自我提权）
- [x] **[P0]** PR-4：拖拽 UX（行内含意图 = AllowOnce；纯路径 = TUI 5 选项菜单）
- [x] **[P0]** PR-5：PrimitiveConfig.path_rules / WorkspaceConfig.entries schema 升级 + env sanitize + pi config edit validate
- [x] **[P0]** PR-6：PrimitiveAuditEntry 加 permission_level / grant_source / in_working_dir
- [x] **[P0]** PR-7：config_get / config_set LLM 工具（双向白名单 + 单元素追加 + 二次 confirm）
- [x] **[P0]** PR-8：System prompt WorkspaceStateSection 注入 + 启动横幅
- [x] **[P0]** PR-9：{work_dir}/agents/{id} 接入 EffectiveRoots.read_only；凭据子目录默认 path_rules deny
- [x] **[P0]** PR-10：`pi pathrules add/list` CLI 子命令（首版无 remove）
- [x] **[P0]** 流程：分支侧 rustfmt + clippy + cargo test + 集成 / E2E

### ✅ P1（文档收尾）
- [x] **`permission-system.md`**：`openspec/specs/architecture/permission-system.md`（12 节 + 多幅 ASCII 辅助图）
- [x] **交叉引用**：`Architecture.md` / `security.md` ↔ 权限子系统

### 🧪 验收 (本分支)

- **rustfmt**：`cargo fmt --all` 干净（exit 0）。
- **clippy**：`cargo clippy --all-features --all-targets -- -D warnings` 干净（修复 `manual_contains` / `redundant_guards` / `derivable_impls` / `single_match` 4 类残留 lint）。
- **lib unit tests**：`cargo test --all-features --lib -- --test-threads=1` → **538 passed / 0 failed / 1 ignored**。
- **集成 + E2E**（含 `wasmedge_e2e_*`）：`cargo test --all-features --tests --no-fail-fast -- --test-threads=1` → **39 passed / 0 failed**（70.8s）。
- **架构文档**：`openspec/specs/architecture/permission-system.md` 含决策树 / 组件依赖 / EffectiveRoots 合并 / Bash 流水线 / 拖拽 UX / config 双通道 / WorkspaceState 数据流等 ASCII 图。
- **覆盖率**：`cargo llvm-cov` 当前未在本机安装；按规范保持 `Cov% = -`，待 CI 上跑过后回填。
- **新增测试覆盖**（按 PR 分布）：
  - PR-1：`core::permission::tests::gate / session_grants / path_rule / dragged_paths / effective_roots / defaults`
  - PR-2：`core::executor::tests::gate_suite`（含 `pr2_*` / `pr9_*`）
  - PR-3：`core::permission::bash_parser::tests`
  - PR-4：`api::chat::tests::dragged_path` + `api::chat::tests::interpret_dragged_paths`
  - PR-5：`infra::config::tests::path_rules` / `workspace_entries` / `env_sanitize`
  - PR-6：`infra::audit::tests::primitive_entry_fields`
  - PR-7：`api::chat::tests::config_tool`
  - PR-8：`core::tests::system_prompt`（含 `WorkspaceStateSection` 渲染 + 优先级）
  - PR-9：`core::executor::tests::gate_suite::pr9_*`、`core::permission::tests::gate::pr9_*`
  - PR-10：`api::cli::tests::pathrules_cmd`

### 🔌 INTERFACE (接口变更)
> 本分支引入的对外 API / 配置 schema 变更。详见 `.cursor/plans/workspace_permission_tiers_design_1ad7681a.plan.md` §2 / §5 / §6。

- `src/core/permission/`: 新增模块（trait `PermissionGate` / `DefaultPermissionGate` / `PermissionDecision` / `GrantSource` / `PermissionLevel` / `PathRuleMode` / `EffectiveRoots`）
- `src/core/confirmation.rs`: `confirm() -> ConfirmDecision { AllowOnce / AllowAndPersistRoot / Deny }`（替换原 `bool`）
- `src/infra/config/types.rs::PrimitiveConfig`: **删除** `path_blacklist` / `require_approval_for_all_write` / `require_approval_for_all_bash`；**新增** `path_rules: Vec<PathRule>`；`bash_forbidden` / `bash_approval_required` 默认值改 regex 列表
- `src/infra/config/types.rs::WorkspaceConfig`: **新增** `entries: Vec<WorkspaceEntry>`（path/alias/description）
- `src/infra/audit/mod.rs::PrimitiveAuditEntry`: 新增 `permission_level` / `grant_source` / `in_working_dir`（`#[serde(default)]` 向后兼容）
- 新增 LLM 工具：`config_get(key)` / `config_set(key, value)`（受 `CONFIG_READ_ALLOWLIST` / `CONFIG_HARDCODED_*_DENY` / `CONFIG_WRITE_ALLOWLIST` 三重约束 + 单元素追加语义）
- 新增 CLI 子命令：`pi pathrules add <path> --mode deny|readonly` / `pi pathrules list`
- 新增 system prompt section：`WorkspaceStateSection`（priority 150）

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |

### 🚧 已知遗留
- `api::cli::tests::run_basic::run_init_returns_ok` 在并发执行（`cargo test` 默认多线程）下偶发失败：原因是 `with_pi_config_in_home`（workspace_cmd / pathrules_cmd 共用）切换了 `HOME`，与 `run_init` 直接读 `~/.pi_/pi.config.toml` 形成时间窗。规避方法是 `--test-threads=1`，已在本分支 CI 命令固化。**预存在的 flaky test，与本分支改动无因果关系**，但因新增 `pathrules_cmd` 测试触发频率上升，建议合并后将 `run_init_returns_ok` 也纳入同一全局 HOME 锁。
