| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| @Jerry | 2026-04-27 23:55 | PENDING_INTEGRATION | feature/workspace-permission-tiers-hotfix | - |

### 🩹 Hotfix 范围

T2-P0-004（workspace-permission-tiers）已合并 develop（merge `11eb5e7`）后，发现 plan §8.2 / §7 / §8.0 三处实施侧偏离，本分支统一打补丁，最终一次性出 1 个 commit 重新提交集成。

依据 plan：`.cursor/plans/cwd-startup-prompt-fix_7af19637.plan.md`

### ✅ TODO（进行中）

- [x] **[P0]** A.0：ChatContext 启动 snapshot `cwd` / `cfg_path`（`std::env::current_dir()` 一次性，避免 `set_current_dir` footgun）
- [x] **[P0]** A.0：`WorkspaceState` + `WorkspaceStateSection` 注入「## Current Working Directory」段（先于 Workspace State，三态文案随 `cwd ∈ effective_roots` 切换）
- [x] **[P0]** A.1：删除 `print_startup_banner` 与 `chat_loop` 调用（启动横幅完全移除）
- [x] **[P0]** A.2：新建 `src/api/chat/cwd_lazy_prompt.rs` lazy first-touch 装饰器（`[a]/[s]/[n]` 三分支，`Arc<AtomicBool>` 共享 dismissed，非 TTY 自动 dismiss 防 CI 阻塞）
- [x] **[P0]** A.3：`ChatContext::from_config` 用 `CwdLazyPrompt::new(...)` 包裹 `CliConfirmation`
- [x] **[P0]** B：`dragged_path::interpret_dragged_paths` 修正「带引号路径 + 紧贴非 ASCII 意图」误判（`split_path_and_suffix` 存在性 + 字符边界切分）
- [x] **[P0]** Test：单测 + `tests/cwd_lazy_prompt_e2e.rs` 集成测试（6 用例全绿）+ 4 个 `dragged_path` 回归 case + system_prompt cwd 段单测
- [x] **[P0]** Doc：`openspec/specs/architecture/permission-system.md` 顶部 hotfix 引用 + §8.1（Token 内存在性切分）+ §10.0（cwd 注入）+ §10.1（Lazy First-Touch 范围级授权）三块更新
- [x] **[P0]** Doc：`agents/TASK_BOARD_002.md` 追加双 hotfix 行（不把 T2-P0-004 主状态改回 DOING）
- [x] **[P0]** Gate：`cargo fmt --all --check` + `cargo clippy --all-features --all-targets -- -D warnings` + `cargo test --all-features --lib`（567 passed / 0 failed / 1 ignored）+ `cargo test --all-features --tests --no-fail-fast -- --test-threads=1`（含 `cwd_lazy_prompt_e2e` 6 / `cli_tests` 77 / `wasmedge_e2e_tests` 39 等 17 个 crate 全绿，共 768 用例）
- [ ] **[P0]** Commit：按 commit-guard 出 1 个 hotfix commit + push origin

### 🔌 INTERFACE（接口变更）

- `core::system_prompt::WorkspaceState`: 新增 `cwd: String` 字段（`#[serde(default)]` 向后兼容）
- `api::chat::ChatContext`: 新增 `cwd: PathBuf` / `cfg_path: PathBuf` 字段（pub，但被 `ChatContext` 整体 `pub` 暴露，影响下游构造方仅在 `from_config` 内部完成）
- 新建 `src/api/chat/cwd_lazy_prompt.rs`：`pub mod` 暴露 `CwdLazyPrompt` / `CwdPromptChoice` / `parse_choice` / `target_in_cwd` / `split_path_and_suffix`（部分仅供 cross-module test 使用，标 `#[doc(hidden)]`）
- `print_startup_banner` 从 `src/api/chat/mod.rs` **移除**（无对外 API；`compute_workspace_state` 保留）
- `dragged_path::interpret_dragged_paths` 行为校正：「带引号路径 + 紧贴非 ASCII 意图文字」从误入 `PromptMenu` 改为正确返回 `AutoAllow`，新增 `pub fn split_path_and_suffix`

### 🧪 验收记录

| 验收项 | 结果 | 备注 |
| :--- | :--- | :--- |
| `cargo test --all-features --test cwd_lazy_prompt_e2e --test-threads=1` | 6 passed / 0 failed | 覆盖非 TTY fallback / dismissed 短路 / `[a]` 写盘 + SessionGrants / `[s]` 仅 SG / `[n]` dismissed + forward / cwd 已授权短路 |
| `src/api/chat/cwd_lazy_prompt::tests` (lib) | 17 passed / 0 failed | parse_choice / target_in_cwd / extract_target_from_preview / 装饰器 5 分支 / apply_choice 4 分支 / 非 TTY |
| `src/api/chat/dragged_path::tests` (lib) | 20 passed / 0 failed | 含新增 7 个回归 case：split_path_and_suffix 4 case + quoted_path_with_intent_text_returns_auto_allow / quoted_path_with_space_and_intent_returns_auto_allow / nonexistent_ascii_path_keeps_prompt_menu |
| `src/core/tests/system_prompt::*` (lib) | 19 passed / 0 failed | cwd 段渲染顺序 / 三态文案 / 空 cwd 跳过（5 个新增 case） |
| `cargo fmt --all --check` | clean | - |
| `cargo clippy --all-features --all-targets -- -D warnings` | 0 warnings, 0 errors | 14.82s |
| `cargo test --all-features --lib` (full) | 567 passed / 0 failed / 1 ignored | 9.59s |
| `cargo test --all-features --tests --no-fail-fast` (full) | 17 crate / 195 passed / 0 failed | 含 cli_tests 77 + wasmedge_e2e_tests 39 + cwd_lazy_prompt_e2e 6 |

### ⚠️ BLOCKED

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |

### 🚧 已知遗留

- 与 T2-P0-004 主分支的 `run_init_returns_ok` flaky test 同源问题保持已知（与本补丁无因果）。
- E2E 场景库 `E2E_SCENARIO_LIBRARY.md` 三条新增编号（`E2E-CLI-cwd-current-directory-system-prompt` / `E2E-CLI-cwd-lazy-first-touch-authorization` / `E2E-CLI-dragged-path-with-nonascii-intent`）尚未挂入；记 follow-up，不阻塞本补丁交付。
