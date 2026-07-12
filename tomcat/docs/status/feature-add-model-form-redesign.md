| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Nibbles | 2026-07-12 12:57 +0800 | ACTIVE | feature/add-model-form-redesign | — |

### ✅ DONE (已完成/进行中)
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
- 本分支目标：Add Model 双模式 Tab + 官方 seed 语义；叠加 Composer `@` 提及；再叠加 Transcript 扁平化与结构化内联 diff（扩展 `0.1.6`）。
- Transcript：命令/写编辑共用 DisclosureCard（折叠 tail5、展开半屏滚动）；核心下发对齐 diff，前端 DiffView 彩色内联；View diff 重建编辑器对比。
- `@` 行为：有工作区时敲 `@` 出下拉；无工作区/切会话经 `closeMention()` 关闭；选中注入 context chip，不改 serve/wire。
- 验收：`cargo test` + 扩展 lint/unit/Devhost E2E；`package:vsix` / 纯插件包不入库。
