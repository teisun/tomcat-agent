| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Spike | 2026-06-07 01:37 +0800 | PENDING_INTEGRATION | feature/skill-system | — |

### ✅ DONE
- [✓] **[P1]** 完成 Skill 主链：`core/skill` frontmatter / config / 三层发现 / `SkillSet` 运行时状态 / `<available_skills>` prompt 注入 / `load_skill` 工具 / `/skill` 与 `tomcat skill`
- [✓] **[P1]** 已补集成与 E2E 覆盖：新增 `tests/skill_tool_tests.rs`，在 `tests/cli_tests.rs` 增补 `/skill` 与 `tomcat skill` 用户路径，并登记 `scripts/test-groups.sh`
- [✓] **[P1]** 已同步规格文档：`docs/openspec/specs/User_Stories.md` 与 `docs/openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md` 补齐 Skill 发现/披露/装载/命令场景
- [✓] **[P1]** 本地门禁通过：`cargo fmt --all`、`cargo clippy --all-targets -- -D warnings`、`cargo test --lib -- --nocapture`、`./scripts/run-integration-tests.sh all`

### 🔌 INTERFACE
- 新增顶层 `[skills]` 配置及 env override，驱动 Skill 发现、prompt 预算、禁用列表与 reviewer 暴露策略
- 新增 `load_skill(name, file?)` 工具、`<available_skills>` prompt section、聊天命令 `/skill list|reload|use` 与外层 CLI `tomcat skill list|reload`
- 新增集成测试目标 `tests/skill_tool_tests.rs`，并把 Skill 路径接入 `scripts/test-groups.sh`

### ⚠️ BLOCKED
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
