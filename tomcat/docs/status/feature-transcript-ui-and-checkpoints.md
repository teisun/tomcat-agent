| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| tomcat | 2026-07-15 08:53 +0800 | ACTIVE | feature/transcript-ui-and-checkpoints | — |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** Checkpoint 最新一轮消失修复：`setCheckpoints` 不再触发 `rebuildHistoryTimeline`；分隔线改为 GUI 渲染层现算（`checkpointMarkers.ts` + `TranscriptView` checkpoints prop）@2026-07-13
- [✓] **[P0]** Checkpoint 分隔线缺失修复：`count_worktree_files_until` 改用 `git ls-files --cached --others --exclude-standard`，计数口径≡快照，尊重 `.gitignore` 与 DEFAULT_EXCLUDE_RULES @2026-07-13
- [✓] **[P1]** 分层验收：state/provider/webview_provider_flow + GUI checkpointMarkers 单测；shadow_git 忽略目录计数与 ignored-only 语义单测；E2E 库登记 VSCEXT-024/025；不含 CLI 的 `0.1.11` VSIX 已手工复验（gitignore，不入库）@2026-07-13
- [✓] **[P0]** Transcript checkpoint restore 落地：list/restore 协议、三态确认弹层、`revertFiles` 截断对话并可选回滚文件 @2026-07-13
- [✓] **[P0]** 上一轮进行态收尾：`TranscriptView` 仅 live cluster 持有 thinking streaming；`agent_idle` 经 `settleRunningTools` 收敛残留 running/streaming 工具卡；GUI/state/provider 分层测试 + E2E-VSCEXT-026 @2026-07-13
- [✓] **[P0]** Sticky reveal 触发加固：`useAutoScroll` 改按 `latestUserMessageId` 变化触发 reveal，护栏 `userMessageCount` 未减少；超一屏后切回 follow-bottom 并显示当前轮 sticky；分层测试 + E2E-VSCEXT-027；ext/gui bump `0.1.12` @2026-07-13
- [✓] **[P0]** 真机「新提示词不置顶」真因修复：真实浏览器 smoke（Vite dev server + CDP）复现——reset effect 与 reveal effect 共享 `previous*Ref`，`resetKey` 与新 user **同帧变化**时，先声明的 reset 把 ref 洗成"已见过"，静默吞掉 reveal（jsdom mock 布局测不出）。合并为**单一确定性 `useLayoutEffect`**（`resetKey/latestUserMessageId/oldestItemKey/userMessageCount`），reset 分支权威落底、同会话追加必 reveal；真机复验 reveal 到顶→超一屏切 follow-bottom→sticky 全链路 + collision 落底；新增 `useAutoScroll.test.tsx` collision/remount-后发送 两条不变量 + E2E-VSCEXT-028；ext bump `0.1.13` @2026-07-13
- [✓] **[P0]** 真机「新提示词不置顶」**第二真因**修复（0.1.13 仍复现）：用**生产构建 `gui/dist`** 静态服务 + CDP 复刻真机时序（`busy=false` echo→`busy=true` 翻转→流式）取证——`busy` 翻转使 composer 变高、stream 容器 `clientHeight` 变大，reveal spacer 仍按旧视口算导致**钳底 + `scroll` 事件翻成 follow-bottom**，reveal 当场丢失（jsdom mock `clientHeight` 恒定测不出）。修复：`ResizeObserver` 检测 `clientHeight` 变化即**重算 spacer 并重新固顶**（可增大 spacer）；钳底 `scroll` 事件在当前轮仍装得下时**重新固顶而非 follow-bottom**。生产构建复验：`busy` 翻转后 u2 保持 `top=0`、长回复溢出仍 follow-bottom；新增 `useAutoScroll.test.tsx` 可变 `clientHeight` 两条不变量 + E2E-VSCEXT-029；ext/gui bump `0.1.14` @2026-07-13
- [✓] **[P0]** `.plan.md` Cursor 风自定义预览编辑器落地：`CustomTextEditorProvider` + Vite 多入口；Preview（正文→N To-dos→四态清单 + 懒加载 mermaid SVG）/ Markdown 只读切换（原生 `editor/title` 溢出菜单）；默认 `tomcat.plan.toolbarStyle=hybrid` 细 sticky 半透明操作条（无白字扁平模型下拉 + 小圆角黄 Build）；全局 `tomcat.plan.buildModel`；选区「加入 Tomcat Chat」（浮动按钮 + `webview/context`）；`planDocument` 统一 frontmatter 解析供预览与聊天卡片复用；host/GUI/E2E（含 mermaidSvgCount + 两路 selection chip）+ 真机自检全绿 @2026-07-14
- [✓] **[P0/P1]** Plan 选区加入聊天：修「点了不进 chip」+ 稳定 `文件名:行号`。P0 根因是无行号 selection 在 `referenceIdentity` 塌成同一键被 Composer 静默去重；无行号 selection 追加文字 FNV hash；P1 以 `bodyLineMap` + `data-source-line` 替代脆弱原文 substring。host/GUI/E2E（两路含 `:行号` + path3 两段无行号 todo 皆落 chip）全绿；`0.1.14` VSIX 已打包（gitignore）@2026-07-14
- [✓] **[P0]** Plan 预览顶栏铺满 + 正文底部留白：`body.tc-plan-webview{padding:0}` 去掉 VS Code webview 默认 20px；`.tc-plan-preview__content` 底部 padding 提到 40px；DOM 快照加 `stripInsetLeft` 断言顶栏 inset≈0 @2026-07-15
- [✓] **[P0]** Thinking 分组标题带目的：`generate_turn_summary` 禁止裸 `Used N tools`；命中后二次 utility 调取 purpose clause，拼成 `Used N tools for <clause>`；title_generator/集成/ThinkingGroup 单测覆盖 @2026-07-15
- [✓] **[P0]** Bash 卡片终端化（抄 Cursor）：后端 `generate_command_summary` + `tool.summary_updated`（`ServeToolEvent`）异步升级目的短句；ToolRow 头改为 `summaryTitle|Ran` + 命令名标签，正文 `TerminalOutput` 前置 `$ 完整命令`；host/GUI/state/E2E 覆盖；已知限制：live-only，history 重载回落占位 @2026-07-15
- [✓] **[P0]** Round2：`tool.summary_updated` 补进 serve `event_pump` 白名单（真因：emit 了但未转发）；`commandBinaries` 跳过注释/heredoc/非法 token、上限 3；plan 预览改为 `plan.create` 登记、`plan.review` 后打开；正文左右 padding 16px + `bodyInsetLeft` 断言；event_pump/provider/ToolRow/host E2E 覆盖；`0.1.14` VSIX 已重打 @2026-07-15

### 🔌 INTERFACE (接口变更)
- Webview store：`session.checkpoints` 与 `timeline` 解耦；timeline 仅含消息/工具项，checkpoint 分隔线由 GUI `injectCheckpointMarkers(timeline, checkpoints)` 现算。
- Checkpoint 文件计数：shadow git 上限计数改走 `git ls-files -z --cached --others --exclude-standard`，与 `git add -A` 快照口径一致；仅改 ignore 文件的一轮不产生新存档（设计行为）。
- AutoScroll：reveal 触发输入由 `lastItemIsLatestUser` 改为 `latestUserMessageId`；`agent_idle` 必须结算残留 running 工具卡。
- AutoScroll（0.1.13）：reset 与 reveal 合并为单一 `useLayoutEffect`，deps 增加 `oldestItemKey`；判定用单一 `revealTrackingRef`，消除双 effect 共享 ref 的同帧竞态。
- AutoScroll（0.1.14）：reveal 对视口高度变化免疫——`ResizeObserver` 以 `previousClientHeightRef` 侦测 `clientHeight` 变化并重算 spacer 重新固顶；`handleScroll` 在钳底且当前轮仍装得下时重新固顶而非 follow-bottom。
- Plan Preview：自定义编辑器 `tomcat.planPreview`；协议 `planPreviewProtocol`（state 帧含 mode/toolbarStyle/`bodyLineMap`；intent `setBuildModel`/`build`/`addSelectionToChat`；事件 `captureSelectionForChat`）；配置 `tomcat.plan.buildModel` + 临时 A/B `tomcat.plan.toolbarStyle`（默认 hybrid）；聊天 `PlanFileCard` 与预览共享扁平 `PlanBuildModelSelect` 与统一 Build 行为；`buildSelectionReferenceFromParts` 供编辑器选区与预览选区共用；`MarkdownBody` 块级打 `data-source-line`；`referenceIdentity` 对无行号 selection 追加文字 hash。
- Serve/wire：新增 `tool.summary_updated`（`ServeToolEvent`，含 `toolCallId` + `summaryTitle`）；**必须列入 `event_pump.EVENT_NAMES` 白名单才会转发到插件**。
- Plan auto-open：`plan.create` 只登记 `planId→path`；`plan.review` 到达后才 `openWith`（审稿完成再开，避免抢焦点）。
- Turn summary：裸 `Used N tools` 触发二次 purpose 调用，前端 ThinkingGroup 可渲染 `Used N tools for <purpose>`。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| （空）round2 三项已修；`tomcat.plan.toolbarStyle` 仍为临时 A/B | 待合并前决定是否去掉 native 分支 | — |

### 集成说明
- 本分支目标：Transcript UI + checkpoint restore；已落地 `.plan.md` Cursor 风预览，并完成顶栏铺满/正文留白、thinking 目的标题、bash 卡片终端化，以及 round2 真机断链修复（白名单 + 审稿后打开）。
- 验收分层：GUI util / TranscriptView / App / state / provider 单测；E2E 场景登记 VSCEXT-026/027/028/029；plan 预览另含 host/GUI/E2E（hybrid 出条、selection 行号、mermaid、stripInsetLeft、bodyInsetLeft、`plan.create`→`plan.review` 才打开、bash summaryTitle）；纯布局/时序真因以真实浏览器 smoke 取证。
- 已知后续：`tomcat.plan.toolbarStyle` 为临时 A/B，选定后应删 flag 与 native 分支；devhost 全量偶发失败项 `lazy loads a giant historical tool group` 与本轮无关，待另案排查。
