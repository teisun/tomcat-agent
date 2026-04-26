| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| @Spike | 2026-04-26 | DOING | feature/compaction-prompt-9section | - |

## T2-P0-002 | compaction-prompt-and-ctx-v2 | 摘要 prompt 9 节升级 + Compaction 收尾

> 看板单：[../../agents/TASK_BOARD_002.md#t2-p0-002--compaction-prompt-and-ctx-v2--摘要-prompt-升级--context-v2-收尾](../../agents/TASK_BOARD_002.md)
>
> 计划文档：`~/.cursor/plans/compaction_prompt_9-section_41653219.plan.md`
>
> 关联报告：[../reports/compaction-prompt-cc-vs-pi.md](../reports/compaction-prompt-cc-vs-pi.md) §5.3 / §5.4 / §5.7
>
> 关联 TODOS：`#T-041` `#T-136` `#T-137`；改判：`#T-040` 关闭归并 / `#T-043` 抽出 T2-P0-011 / `#T-044` 报告决议关闭

---

### 🟡 DOING (进行中)

#### 流程类
- [x] **[流程]** 立项决议落档（`2026-04-26`：`#T-040` / `#T-043` / `#T-044` 改判 + 抽出 T2-P0-011；详见看板 §6 变更记录）
- [x] **[流程]** 看板认领 T2-P0-002（TODO → DOING，负责人 Spike）
- [x] **[流程]** 创建分支 `feature/compaction-prompt-9section`（基于 `develop`）
- [x] **[流程]** Phase B 末门禁：`cargo fmt --check` + `cargo clippy --all-targets -- -D warnings` + `cargo test -j 1 --lib -- --test-threads=1` 全绿（449 passed / 0 failed / 1 ignored，含新增 13 个 `prompt_snapshot` 测试）
- [ ] **[流程]** F 阶段 §4 全量门禁（按 INTEGRATION_MERGE_AND_ACCEPTANCE「后台写日志 + 轮询」）

#### 实施类（按 plan §6）
- [x] **[Phase A]** Two-pass 不实施决议落档：在 `docs/reports/compaction-prompt-cc-vs-pi.md §5.7.1` 新增「Two-pass 决议固化」段落（背景：CC fork 子代理 + prompt cache vs. Pi 单次 LLM 直发；决议：不实施；替代：模板追加 `First reason internally, then output the final summary.`；反向逃生口；关闭轨迹）；§5.7 表格 Two-pass 行回链改指 §5.7.1；`docs/TODOS.md` 顶部速查表 + 详细条目区两处回链同步刷新 — 关闭 `#T-044`
- [x] **[Phase B-1]** `preheat.rs` 两个 const 升级 9 节模板（来源 报告 §5.3 BASE / §5.4 UPDATE；`Recent User Messages` 保留最近 10 条用户原话；`Next Steps` verbatim；指令区追加 `First reason internally, then output the final summary.`），可见性改 `pub(super)` 供测试访问
- [x] **[Phase B-2]** `generate_summary` 的 `ChatRequest` 显式 `tools: None` + 注释「Compaction MUST NOT carry tools」（双保险：prompt 首行 + 请求体无 tool schema）；两个模板首行固定 `Respond with text only. Do not call any tools.`；新增 `tests/prompt_snapshot.rs` 13 个断言（9 节标题、text-only 首行、内部 reason、最近 10 条、verbatim 引用、文件锚点、占位符、`tools.is_none()` / `stream=Some(false)`、`pub(super)` 可见性 lock）
- [ ] **[Phase D-1]** `preheat.rs` 重试 loop Err 分支尾部追加 `tokio::time::sleep` 指数退避（500ms / 1s / 2s）；用 `tokio::time::pause` 写虚拟时钟单测
- [ ] **[Phase D-2]** `BranchSummaryEntry` 新增 `error: Option<String>` / `attempts: Option<u32>`（`#[serde(skip_serializing_if = "Option::is_none")]` 向后兼容）；3 次耗尽后 `insert_entry_after_message_id` 写入失败锁点；`legacy_transcript_compat.rs` 兼容性测试
- [ ] **[Phase F]** 全量回归门禁：`cargo test -j 1 --test '*' --test-threads=1` 后台日志 + 轮询；失败项在本分支修复，不弱化断言
- [ ] **[Phase G]** F 全绿后仅在 `openspec/specs/architecture/context-management.md` 补充 `Compaction v2（T2-P0-002）` 简短小节（9 节模板 / 禁 tools / 退避+留痕 + 3 项不实施决议回链）；`docs/reports/compaction-prompt-cc-vs-pi.md §5.6` 三项 TODO 改 `[x]`

### 🚫 不实施（已落档关闭）

| 子项 | 原 TODO | 决议 | 落档位置 |
| :--- | :--- | :--- | :--- |
| 子项 3 | `#T-040` 超大消息 / 字符边界 panic | **关闭归并**：现有 Layer 0（`>= 50K` 落盘 + 200 字 preview）+ Phase D 失败留痕已覆盖；`messages_to_text` 对 User/Assistant 不切片，原 panic 描述实指 Layer 0 路径，且早已稳定 | plan §6.C / 报告 §5.7 |
| 子项 5 | `#T-043` 大文件多次编辑写入 | **抽出 T2-P0-011**：原 TODO 真实归属是 `executor/primitives.rs::edit_file`（agent 写大文件方式），与 compaction 无关；`_index.jsonl` 信息可由 `transcript 占位符 + fs mtime + 文件名（tool_call_id）` 完全重建 | plan §6.E / 报告 §5.7 / 看板 T2-P0-011 |
| 子项 6 | `#T-044` Two-pass summary | **报告决议关闭**：CC 用 fork+cache 抵消草稿成本，Pi 单次 LLM 直发性价比不好；改在 prompt 内加一句 `First reason internally, then output the final summary.` 隐式诱导 | plan §6.A / 报告 §5.7 |

### 📐 计划改动文件清单（仅业务源文件 + transcript 数据载体）

| 文件 | 类型 | 改动概述 |
| :--- | :--- | :--- |
| `src/core/compaction/preheat.rs` | 业务 | 两个 const 升级 9 节模板（B）；`generate_summary` 显式 `tools: None`（B）；Err 分支退避（D） |
| `src/core/session/transcript.rs` | 业务 | `BranchSummaryEntry` 新增 2 字段 `error` / `attempts`（D） |
| `src/core/compaction/tests/prompt_snapshot.rs` | 测试 | 新建 — 断言 9 节标题与 text-only 首行 |
| `src/core/compaction/tests/preheat_and_truncation.rs` | 测试 | 扩展 — 退避 + 失败留痕 |
| `src/core/compaction/tests/legacy_transcript_compat.rs` | 测试 | 新建 — 旧 transcript 反序列化兼容 |
| `openspec/specs/architecture/context-management.md` | 文档 | Phase G 增 `Compaction v2（T2-P0-002）` 简短小节 |
| `docs/reports/compaction-prompt-cc-vs-pi.md` | 文档 | Phase A：§5.7 补 Two-pass 决议；Phase G：§5.6 三项 TODO 改 `[x]` |
| `agents/TASK_BOARD_002.md` | 文档 | DOING / PENDING_INTEGRATION 状态 + §6 变更记录 |

### 📝 待 PENDING_INTEGRATION 时填写

- [ ] 各 Phase commit hash 列表
- [ ] §4 全量门禁结果（`cargo test -j 1 --test '*' --test-threads=1` 通过率 + 用例数）
- [ ] `feature/compaction-prompt-9section` push 远端 + 看板 `DOING → PENDING_INTEGRATION`
