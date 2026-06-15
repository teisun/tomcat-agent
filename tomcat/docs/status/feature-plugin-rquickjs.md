| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tom | 2026-06-15 12:30 +0800 | ACTIVE | feature/plugin-rquickjs | 66.2 |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** 移除 WasmEdge 运行时、资产与脚本，统一 Plugin 命名 @2026-06-14
- [✓] **[P0]** 插件加载默认放行 requiredPermissions @2026-06-14
- [✓] **[P0]** 原生 aes-gcm / ed25519 与 JS crypto shim 接线 @2026-06-14
- [✓] **[P1]** 机会主义 idle VM 回收策略与配置项 @2026-06-14
- [✓] **[P1]** 引擎实现迁入 `src/ext/runtime/` 抽象边界 @2026-06-14
- [✓] **[P1]** 测试补全：runtime_manager、crypto、chat ctx、tool executor、doctor、plugin unload @2026-06-14
- [✓] **[P2]** 并发测试 cwd_lock 消除 PoisonError @2026-06-14
- [✓] **[P2]** 文档与配置表更新（plugin-system-overview、user-guide、tomcat.config.toml） @2026-06-14
- [✓] **[P1]** 三类真实插件测试：纯工具型 E2E、session_start 集成探针、legacy 工具箱 E2E、跨事件 VM 状态 @2026-06-14
- [✓] **[P1]** PackageManager 统一安装：三层 package/plugin/skill 安装与卸载、`/install` live refresh、黑盒/集成/E2E/文档收口 @2026-06-15
- [✓] **[P1]** PackageManager 规范对齐：registry schema、严格 npm 版本、`plugin.json` 单文件名、§9 验收矩阵与文档回链 @2026-06-15

- [✓] **[P2]** 清理 vendored OpenSpec Cursor skills（`openspec-*`） @2026-06-15

### 2026-06-15 | chore(cursor): 移除 vendored OpenSpec skills

- **交付**：删除 `tomcat/.cursor/skills/openspec-{apply-change,archive-change,explore,propose}/SKILL.md`，避免仓库内重复维护 OpenSpec 工作流技能副本。
- **验证**：纯文档/技能清理，无代码路径变更。
- **覆盖**：状态页沿用当前分支既有 `66.2%` baseline。

### 2026-06-15 | fix(package): PackageManager 规范对齐

- **交付**：`packages/registry.json` 写入 `tomcat.package.registry.v1` 并拆分 `plugins[]`/`skills[]`；版本只认外层 `package.json.version` 且拒绝 `tomcat.version`；manifest 空列表时自动扫描 `plugins/*`/`skills/*`；移除 `pi-plugin.json` 兼容；`/install` 刷新失败 warning 补齐重载指引；`plugin list` 分层遮蔽测试与 agent 跨 scope 持久化 E2E。
- **验证**：PackageManager 回归集通过（`core::package` 单测、`package_cmd_test`、`plugin_cmd_test`、`cmd_install_test`、`cli_tests` 相关场景）；全量 `cargo test -p tomcat` 仍有 9 个真实 LLM CLI 用例因 `401 invalid_api_key` 失败，与本次改动无关。
- **覆盖**：状态页沿用当前分支既有 `66.2%` baseline；新增版本规则、legacy registry 迁移、自动扫描与 layered plugin list 单测覆盖。

### 2026-06-15 | feat(package): PackageManager 统一安装收口

- **交付**：新增 `core/package` 事务层，打通 `tomcat install` / `tomcat uninstall` / `tomcat packages` 与会话内 `/install`，落地 `scope|agent|global` 三层 `packages/registry.json` / `plugins/registry.json`，并补齐 `code` / `claw` 会话内 skill 与静态 plugin catalog 的即时可见性。
- **验证**：`cargo fmt --all`、`cargo clippy --all-targets --all-features -- -D warnings`、`cargo test --lib`、`cargo test --test cli_tests`、`./scripts/run-integration-tests.sh integration` 全通过；真实 TTY chooser/cancel/shadow warning 与真实 `tomcat code` 会话 `/install` live refresh 已完成人工补验。
- **覆盖**：状态页沿用当前分支既有 `66.2%` baseline；本次新增 `src/core/package/tests/`、CLI/chat 单测、`tests/cli_tests.rs` 黑盒场景与 `E2E_SCENARIO_LIBRARY.md` 场景登记，覆盖 scope/agent/global 安装、卸载、shadow warning、live refresh 与“不热替换已加载 plugin”边界。

### 🔌 INTERFACE (接口变更)
- `crate::ext::runtime/`：引擎/实例/crypto 迁入子模块，对外仍通过 `crate::ext::PluginEngine` 等再导出。
- `PluginConfig` / `PluginEngineConfig`：新增 idle TTL、heap/timeout 等运行时配置项（见 `tomcat.config.toml` 与 user-guide）。
- `crypto` JS shim：新增 `aesGcm.*` / `ed25519.*` 命名空间 API。
- `plugin unload`：仅注册于 registry 的插件也会从 registry.json 移除。
- `PluginToolExecutor`：crate root 再导出，供 E2E 测试直接搭建工具执行链路。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | — | — |
