| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Nibbles | 2026-07-12 20:40 +0800 | ACTIVE | feature/add-model-form-redesign | — |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** Sticky 底部流式残留 bug 修复：只要最新一轮 user message 仍在屏幕内（顶部或底部都算），sticky 一律隐藏，不再悬浮上一轮问题；待该轮被自身回答顶出一屏后再显示新 sticky @2026-07-12
- [✓] **[P1]** 验收：`check:wire`、扩展 `lint`、core unit(113)、GUI unit(177)、sticky Devhost 过滤场景全绿；扩展 bump `0.1.10`，`tomcat-vscode-ext-0.1.10.vsix` 已重打（gitignore，不入库）@2026-07-12
- [✓] **[P0]** Sticky 新提示词 reveal 收口：sticky 改按“视口顶部所属轮次”选择；当前轮 user message 仍露在顶部时先隐藏，不再回退显示旧问题；历史滚动保持按轮次切换 @2026-07-12
- [✓] **[P1]** 验收：扩展 `lint`、core unit(113)、GUI unit(176)、sticky Devhost 过滤场景全绿；扩展 bump `0.1.9`，`tomcat-vscode-ext-0.1.9.vsix` 已重打（gitignore，不入库）@2026-07-12
- [✓] **[P0]** Transcript 双 Creating plan 根因修复：plan 抑制改按 `toolName`（运行中尚无 `display`），覆盖 grouped / standalone / raw tool；测试改为真实流式态无 display @2026-07-12
- [✓] **[P0]** Transcript 横向溢出收口：外层只允许纵向滚，flex 子项 `min-width:0`，长 token `overflow-wrap:anywhere`；宽内容仅在 diff/terminal/code 局部滚动 @2026-07-12
- [✓] **[P1]** 验收：`check:wire`、`lint`、core unit(113)、GUI unit(175)、Devhost 全量 33 绿；扩展 bump `0.1.8`，`tomcat-vscode-ext-0.1.8.vsix` 已重打（gitignore，不入库）@2026-07-12
- [✓] **[P0]** Transcript diff/plan UX 修复：`tomcat-diff://` prepared diff 改为 path-key 保住 mixed-case `toolCallId`，View diff 不再白屏；edit 折叠预览改为首个真实变更锚定，不再误看文件尾巴 @2026-07-12
- [✓] **[P0]** Transcript 计划与 sticky 体验收口：Variant B 保留单个 `Creating plan` 组头、隐藏内层成功 plan 行；`View Plan` 在 create/update plan 期间改为呼吸 `...`；sticky user prompt 可随历史滚动切到对应轮次 @2026-07-12
- [✓] **[P1]** 验收：`check:wire`、扩展 `lint`、core unit(113)、GUI unit(69)、Devhost 失败集 10 条全绿；修复 fake serve 生成脚本转义与 no-session bootstrap 假死 @2026-07-12
- [✓] **[P0]** Transcript 视觉整改 v2：Jump 箭头、DisclosureCard/TerminalOutput/DiffView、命令淡黄、read 图标去重；扩展 `0.1.6` @2026-07-12
- [✓] **[P0]** 核心下发结构化 `FileDiffLine` diff（write/edit/hashline）；`ToolDisplay::File.diff` 贯通 serve schema / wire；View diff 由 host 重建 before/after 打开 `vscode.diff` @2026-07-12
- [✓] **[P1]** 验收：`cargo test`、扩展 lint/unit/Devhost E2E 全绿；env_lock 串行稳住 HOME 竞态；纯插件包 `tomcat-vscode-ext-0.1.6-pure.vsix` 已打（gitignore，不入库）@2026-07-12
- [✓] **[P0]** Composer `@` 提及上下文搜索：TipTap Suggestion + host `searchContext`；文件/目录候选、模糊匹配、chip 注入与协议往返；关闭统一走 `exitSuggestion`（单一真源）@2026-07-11
- [✓] **[P0]** `@` UX 收口：加载中保留旧 matches 防闪、子序列高亮与 host 算法对齐、listbox a11y、候选缓存只刷 `isOpen` @2026-07-11
- [✓] **[P1]** 验收：`gate:fast` / `test:integration` 绿；Devhost E2E 覆盖文件 `@`、目录、无工作区关闭；纯插件包 `tomcat-vscode-ext-0.1.5-pure.vsix` 已重打（gitignore，不入库）@2026-07-11
- [✓] **[P0]** 后端 `source` 语义修复：`ModelCatalog` 新增 `builtin_ids` / `is_builtin_seed`；`ModelView.source` 按出厂 seed 判定 Builtin，与「能否删除」(`is_user_model`) 解耦；`tomcat init` 抄入 `models.toml` 的官方模型不再误标为 user @2026-07-10
- [✓] **[P0]** Add Model UI：分段控件改为标准 Tab（tablist/tab/tabpanel + 方向键）；空官方预置时默认落中转站 Tab 并给出引导；`saveDisabled` 旁显示原因；`findMatchingProviderPreset` 放宽为 provider(+api) 匹配 @2026-07-10
- [✓] **[P0]** 双锚点表单落地：官方 preset（模式 A）与 relay/base_url 派生（模式 B）；`thinking_format=Auto` 严格按 wire `api` 推断，不再被 model_name 误导 @2026-07-10
- [✓] **[P1]** 验收门禁：`cargo test`、扩展 `build` / `test:unit` / `test:e2e:vscode-devhost` 全绿；手测清单与 hostE2e 场景已更新；纯插件包 `tomcat-vscode-ext-0.1.5.vsix` 已重打 @2026-07-10
- [✓] **[P2]** 测试稳定性：`TempHomeGuard` 加进程内锁，消除全量 `cargo test` 下 HOME 并发竞态；Auto thinking 断言对齐默认 `high` effort @2026-07-10

### 🔌 INTERFACE (接口变更)
- Transcript plan 抑制：`isSuppressedPlanToolRow` 以 `create_plan` / `update_plan`（及结束态 `display.kind==plan`）为真源，不再依赖运行中尚不存在的 `display`；非 error plan 工具行在 grouped/standalone/raw 路径均不渲染。
- Transcript 滚动边界：`.tc-stream` 仅纵向滚动；消息/cluster 强制 `min-width:0` + `overflow-wrap:anywhere`，禁止整页横漂；diff/terminal/code 保留局部横滚。
- `tomcat-diff://` prepared diff URI：host 侧 key 从 `authority` 挪到 `path`，避免 VS Code 规范化 `authority` 小写后丢失 mixed-case `toolCallId`。
- Transcript Webview：`DiffView` 折叠预览语义从“尾部 tail5”改为“首个真实变更锚定预览”；`PlanFileCard` 新增 creating/busy 呈现；`useAutoScroll` / `App` 贯通 `activeStickyMessageId`。
- Devhost 测试夹具：新增 plan-tool-ux / sticky-history 场景，bootstrap 接受 `ready + no sessions`，避免空页面假死。
- `FileDiffLine` / `DiffTag`：核心 `ToolDisplay::File.diff`、`WriteFileResult`/`EditFileResult.diff` 新增结构化行级 diff；serve schema / wire 已同步。
- Webview：`WebviewToolCard.diff`；`openDiff` 经 host 从 diff 重建 before/after 打开编辑器对比。
- Webview↔Host：`searchContext` 请求/响应与 context search 相关消息；Composer 经 Suggestion 插件驱动，关闭统一 `exitSuggestion` / `closeMention()`。
- `ModelCatalog::is_builtin_seed(id)` / `SharedModelCatalog::is_builtin_seed(id)`：新增；按内嵌 `builtin_models.toml` 出厂清单判定。
- `ModelView.source`：语义从「是否在用户 models.toml」改为「是否在出厂 seed 清单」；GUI Delete 可见性随之变化（seeded 官方模型隐藏 Delete）。
- `ThinkingFormat::Auto`：运行时仅按 wire `api` 解析；`resolve_for_model` 仅保留给测试/工具路径。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |

### 集成说明
- 本分支目标：Add Model 双模式 Tab + 官方 seed 语义；叠加 Composer `@` 提及；再叠加 Transcript 扁平化、结构化内联 diff 与 transcript UX 修复（扩展 `0.1.10`）。
- Transcript：命令/写编辑共用 DisclosureCard；edit 折叠预览改为变更锚定；prepared diff 改 path-key 后，View diff 在真机 mixed-case `toolCallId` 下可稳定打开非空对比。
- Plan / Sticky：运行态按 `toolName` 抑制内层 plan 行，仅保留单头 + PlanFileCard；运行中 `View Plan` 呼吸提示；sticky 可切回历史轮次，且只要最新一轮 user message 仍在屏幕内（顶部或底部）就保持隐藏，待该轮被自身回答顶出一屏后再显示新 sticky；transcript 外层禁止横滚。
- `@` 行为：有工作区时敲 `@` 出下拉；无工作区/切会话经 `closeMention()` 关闭；选中注入 context chip，不改 serve/wire。
- 验收：本轮 `check:wire`、扩展 `lint`、GUI/core unit、sticky Devhost 过滤回归绿；`package:vsix` / 纯插件包不入库。
