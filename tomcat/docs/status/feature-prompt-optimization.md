| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| tomcat | 2026-07-16 06:07 +0800 | ACTIVE | feature/prompt-optimization | — |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** 系统提示词瘦身优先版落地：`core_identity` 增补运营原则（#6/#7/#8 一字不差复述）、新增 `parallel_tools` / `verification` 段、`tool_instructions` 改为 `{tool_guidelines}` 注入；`catalog.rs` 增加 `prompt_guidelines` 并按成功率红线精简 description @2026-07-16
- [✓] **[P0]** 工程规范 #6–#10 写入提示词：#6/#7/#8 在 `core_identity.txt` 与 `planner.txt` 逐字一致；#9/#10 落在 planner（多视角测试 / todos 宁多勿少）；UI(#8) 从工具 guidelines 迁出；单测 `standards_6_7_8_are_byte_identical_in_core_identity_and_planner` 防漂移 @2026-07-16
- [✓] **[P1]** 二次瘦身（常驻去重 + 工具描述去实现细节）：压缩 `background_shell_monitor` / `workspace_state` Config 块；精简 `search_files` / `web_fetch` / `config_*` / `bash` description；轻度压缩 executor/verify 内部重复 @2026-07-16
- [✓] **[P1]** 回归与文档：红线/聚合/section 顺序单测、`prompt_size_budget` 预算门、`tool-catalog.md` 重生；E2E-PROMPT-026–029 写入场景库 @2026-07-16
- [✓] **[P2]** 附带同步：`cloud-scale-serving-01/` 方案文档与架构 README / Architecture 入口；`commit-with-status` command 迁至仓库根 `.cursor/commands/`；刷新 `.cursor/rules/engineering-standards.mdc` 措辞与提示词对齐 @2026-07-16

### 🔌 INTERFACE (接口变更)
- `BuiltinToolCatalogEntry` 新增字段 `prompt_guidelines: &'static [&'static str]`；`render_tool_guidelines_with_policy` 聚合去重后注入 `tool_instructions.txt`。
- `PromptKey` 新增 `SystemParallelTools` / `SystemVerification`；默认系统提示词 section 链插入 priority 22 / 50。
- 无对外部客户端的破坏性 API 变更；系统提示词行为契约以 prompt-only 单测与 E2E-PROMPT-026–029 锁定。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |

### 集成说明
- 本轮以提示词/工具描述静态文本与拼装逻辑为主；验证走 focused lib/doc 测试与 `prompt_size_budget`（CHAT tool defs 25023 / SYS 6996），未跑全量 tarpaulin，Cov% 留空。
- 云化方案 `cloud-scale-serving-01/` 为文档同步，尚未进入 Phase A 编码。
