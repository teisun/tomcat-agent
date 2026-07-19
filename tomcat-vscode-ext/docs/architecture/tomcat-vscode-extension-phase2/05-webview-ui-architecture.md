# Tomcat VSCode 扩展 · Phase 2 · 05 Webview UI 架构与实现细节

> 总览见 [`../tomcat-vscode-extension-phase2.md`](../tomcat-vscode-extension-phase2.md)；Stage B 的落地选型与分层基线见 [`03-stage-b-webview.md`](03-stage-b-webview.md)；协议/运行时字段表见 [`04-protocol-runtime.md`](04-protocol-runtime.md)。
> transcript 稳定 id / reload 切回错乱的专项 companion 见 [`../webview-transcript-stable-id-upsert.md`](../webview-transcript-stable-id-upsert.md)；checkpoint 恢复点的专项 companion 见 [`../transcript-checkpoint-restore.md`](../transcript-checkpoint-restore.md)；本文仍以“当前实现是怎么跑的”为主。
> 本文不是“想做什么”的方案文，而是“已经如何实现”的实现文：事实源以 [`gui/src/**`](../../../gui/src) 与 [`src/ui/webview/**`](../../../src/ui/webview) 为准。
> 外部参考仓库（仅作实现思路来源，不进本仓）：`/Users/yankeben/workspace/cline`、`/Users/yankeben/workspace/continue`。

---

## 1. 定位

> 专业：Phase 2 Stage B 已经把 Tomcat 的 webview 落成“宿主 provider + typed postMessage 协议 + React GUI + timeline 状态机”的四段式结构。本文补齐实现层细节，尤其覆盖本次 UX 优化后新增的自动滚动、thinking 排序、assistant-response 二次分层、`DisclosureCard + 专用 body` 的工具行组合、内联彩色 diff / 原生 `View diff` 双路展示，以及 composer 响应式布局。
>
> 说人话：`03-stage-b-webview.md` 告诉你“为什么要做 React Webview”，本文告诉你“现在这套 UI 到底是怎么跑起来的、为什么这样拆、关键交互埋在哪些文件里”。

实现落点一图：

```text
VSCode WebviewViewProvider
  ├─ provider.ts        宿主生命周期 / intent 路由 / postState / postEvent
  ├─ protocol.ts        host<->webview typed frame / intent 校验
  ├─ state.ts           SessionSnapshot + Timeline 合并器（历史 + 实时）
  └─ gui/
      ├─ main.tsx       acquireVsCodeApi/no-op fallback + React root
      ├─ App.tsx        页面壳 / state 接收 / intent 发送 / 自动滚动接线
      ├─ useAutoScroll.ts
      ├─ components/
      └─ styles.css
```

---

## 2. 组件树与职责

> 专业：GUI 侧是“单页壳 + timeline 子组件 + 底部 composer”的平面结构，没有引入 Redux/Zustand/虚拟列表，状态全部来自宿主 `state` 快照。
>
> 说人话：前端没有复杂状态库，几乎就是“宿主发一份 session 状态，React 把它画出来”。

组件树：

```text
main.tsx
└─ App.tsx
   ├─ SessionBar
   ├─ tc-stream-shell
   │  ├─ TranscriptView
   │  │  ├─ MessageBubble
   │  │  ├─ CheckpointMarker
   │  │  ├─ ThinkingBlock
   │  │  ├─ ThinkingGroup
   │  │  │  └─ ToolRow (grouped/context)
   │  │  ├─ ToolRow (standalone/action)
   │  │  │  ├─ DisclosureCard
   │  │  │  │  ├─ TerminalOutput
   │  │  │  │  └─ DiffView
   │  │  │  └─ AnswerCard (ask_question 已回答态)
   │  │  ├─ ApprovalCard
   │  │  └─ PlanFileCard
   │  └─ Jump to latest button
   ├─ TodoListWidget
   ├─ AttachmentChips
   ├─ RestoreConfirmDialog
   └─ Composer
```

核心文件与职责：

| 文件 | 职责 | 关键点 |
|------|------|--------|
| [`gui/src/main.tsx`](../../../gui/src/main.tsx) | 挂载 React root，拿 `acquireVsCodeApi()` | 无宿主时回退到 no-op `vscodeApi`，方便 `vite` 独立调试。 |
| [`gui/src/App.tsx`](../../../gui/src/App.tsx) | 接 `state` 帧、发 intent、组装整个页面 | 统一处理 `ready` / `prompt` / `setModel` / `setPlanMode` / `openFile` / `openDiff` / `restoreCheckpoint` 等 transcript 相关意图，挂载 `RestoreConfirmDialog`，并在 restore 后把被截断轮次的 prompt 回填到 composer。 |
| [`gui/src/components/SessionBar.tsx`](../../../gui/src/components/SessionBar.tsx) | 顶部会话选择栏 | 下拉显示 `sessionId + isCurrent/owner/busy` 元信息；右侧 `New / Refresh / Close`。 |
| [`gui/src/components/TranscriptView.tsx`](../../../gui/src/components/TranscriptView.tsx) | timeline 分发器 + assistant-response 二次分层 | 输入是 **raw timeline + checkpoints**：先在组件顶部 `useMemo(injectCheckpointMarkers(timeline, checkpoints))` 临时投影出 checkpoint marker，再按 `message / checkpoint / thinking / tool / approval / plan` 6 种一等项分发；assistant 回复内部继续拆成 `action` 恒显行和 `context` 折叠盒；单个无 thinking 的 context 工具会直接扁平渲染。 |
| [`gui/src/components/CheckpointMarker.tsx`](../../../gui/src/components/CheckpointMarker.tsx) | transcript 中的 checkpoint 分隔条 | 不直接改状态，只负责把后端 checkpoint 元数据投影成可点击 marker，并把点击事件抛回 `App.tsx`。 |
| [`gui/src/components/RestoreConfirmDialog.tsx`](../../../gui/src/components/RestoreConfirmDialog.tsx) | restore 确认浮层 | 承接 `Revert` / `Don't revert` / `Cancel` 三态，键盘语义与焦点圈定都在这一层完成。 |
| [`gui/src/components/ThinkingBlock.tsx`](../../../gui/src/components/ThinkingBlock.tsx) | thinking 折叠卡 | 默认折叠；流式时标题显示 `Thinking...` 脉冲动画。 |
| [`gui/src/components/ThinkingGroup.tsx`](../../../gui/src/components/ThinkingGroup.tsx) | “思考/上下文”折叠盒 | 只容纳 thinking + context/other 工具，默认收起；只有确实还挂着工具时才采信 `summaryTitle`，避免和独立 action 行重复。 |
| [`gui/src/components/ToolRow.tsx`](../../../gui/src/components/ToolRow.tsx) | 统一工具行 | 用 `toolCategory()` 把工具分成 `edit / command / answer / context / other`，再决定图标、徽章、扁平样式、展开规则与内容体。 |
| [`gui/src/components/DisclosureCard.tsx`](../../../gui/src/components/DisclosureCard.tsx) | 内容无关的折叠外壳 | 只管 header / preview / expanded body / 左侧状态条，不关心里面是 terminal 还是 diff。 |
| [`gui/src/components/TerminalOutput.tsx`](../../../gui/src/components/TerminalOutput.tsx) | 命令输出体 | 负责等宽输出渲染与 `tail(n)` 预览；可选 `command` prop 会在输出前加一行 `$ <命令>` 提示行（终端观感）。 |
| [`gui/src/components/DiffView.tsx`](../../../gui/src/components/DiffView.tsx) | 结构化 diff 输出体 | 把核心下发的 `FileDiffLine[]` 渲染成行号列、加删底色、长 context 折叠与大文件 fallback。 |
| [`gui/src/components/AnswerCard.tsx`](../../../gui/src/components/AnswerCard.tsx) | ask_question 已回答卡片 | 作为 answer 类工具行的常显内容体，展示问题与已选答案。 |
| [`gui/src/components/ApprovalCard.tsx`](../../../gui/src/components/ApprovalCard.tsx) | AskQuestion 待审批卡 | 直接把宿主 `control_request.ask_question` 渲染成按钮组。 |
| [`gui/src/components/TodoListWidget.tsx`](../../../gui/src/components/TodoListWidget.tsx) | 停靠式 todo 小部件 | busy 阶段承接计划执行态，避免 transcript 中间夹杂太多进度语义。 |
| [`gui/src/components/AttachmentChips.tsx`](../../../gui/src/components/AttachmentChips.tsx) | 待发送附件 chips | 点击即移除，避免在 composer 内塞复杂附件 UI。 |
| [`gui/src/components/Composer.tsx`](../../../gui/src/components/Composer.tsx) | 底部输入区 | `+ / Mode / Model / Ctx / Send` 收敛在单行工具条。 |
| [`gui/src/useAutoScroll.ts`](../../../gui/src/useAutoScroll.ts) | 自动滚动 hook | `ResizeObserver + scroll` 双监听，区分“用户主动上滑”与“仍贴底”。 |

---

## 3. 宿主到 React 的数据流

> 专业：宿主不把 React 当“主动拉数”的客户端，而是把它当“被动订阅状态”的 view。宿主 `provider.ts` 在初始化、切会话、事件到达时推送 `state`/`event` 两类帧；GUI 不直接碰 `TomcatMessenger`。
>
> 说人话：前端不自己连 `tomcat serve`，一切都经由扩展宿主中转。

数据流：

```text
tomcat serve/stdout event
    │
    ▼
provider.ts.handleServeEvent()
    ├─ stateStore.applyEvent(event)
    ├─ postEvent(event)      // 增量透传
    └─ postState()           // 全量快照刷新
             │
             ▼
      App.tsx window.message listener
             │
             ▼
       React render timeline / composer / plan strip
```

宿主侧关键职责：

1. [`src/ui/webview/provider.ts`](../../../src/ui/webview/provider.ts)
   - `bootstrap()`：拉模型列表、项目级 session pool、默认 session，再调用 `refreshSessionState()` + `refreshSessionHistory()` + `postState()`。
   - `handleWebviewMessage()`：把 GUI intent 路由到 `newSession / switchSession / prompt / setModel / setPlanMode / openFile / pickAttachment` 等宿主动作。
   - `handleServeEvent()`：每来一条 `ServeEvent`，先更新 `stateStore`，再同时发 `event` 增量帧和 `state` 快照帧；mutation 类工具的 `diffStat` 与结构化 `diff` 都直接来自核心事件里的 `display.added/removed/diff`。
   - `postState()` / `postEvent()`：前者发送 `WebviewStateSnapshot`，后者发送 `HostEventFrameContent`。

2. [`src/ui/webview/protocol.ts`](../../../src/ui/webview/protocol.ts)
   - 定义 `HostToWebviewFrame` 与 `WebviewIntent`。
   - 提供 `isWebviewIntent()` 做宿主入站校验。
   - 维持 GUI 与宿主共享的 `WebviewTimelineItem`、`WebviewToolStatus` 等类型。

3. [`gui/src/App.tsx`](../../../gui/src/App.tsx)
   - 收 `channel: "state"`：整份覆盖到 React `state`。
   - 收 `channel: "event"`：当前只消费 `__test.capture_dom` 这类测试事件；正常渲染依赖宿主同步后的 `state` 快照。
   - 发 intent：统一走 `postIntent()`，保持消息 ID 生成和 frame 形态一致。

### 3.1 文件改动数据流：一份核心 diff，双路消费

> 专业：本轮没有把 diff 计算留在 VSCode 扩展侧“事后读盘猜”，而是让 Rust 核心在 `write/edit/hashline_edit` 当下直接产出 `ToolDisplay::File { added, removed, diff? }`。这样 transcript 内联彩色 diff 和原生 `vscode.diff` 都共享同一份权威事实源。
>
> 说人话：谁真正同时知道“改前”和“改后”？只有核心。所以 diff 真相从核心来，前端只负责画和打开。

```text
Rust core write/edit/hashline
  ├─ line_diff_stat(old,new) -> added / removed
  ├─ build_line_diff(old,new) -> FileDiffLine[]
  └─ ToolDisplay::File { file, added, removed, diff? }
                │
                ▼
wire.d.ts / protocol.ts / state.ts
  └─ tool.diffStat + tool.diff
                │
                ├─ ToolRow(edit/write) -> DisclosureCard + DiffView
                │     └─ transcript 内联彩色 diff（preview 首变更锚定 / expand 50vh）
                │
                └─ openDiff intent
                      └─ provider.ts reconstruct before(ctx+del) / after(ctx+add)
                            └─ VsCodeIde.openReconstructedDiff()
                                  └─ vscode.diff(tomcat-diff://left, tomcat-diff://right)
```

补充约束：

- `diff` 是可选字段：大文件超阈值时核心只发 `added/removed`，不发 `diff`；
- 这时 transcript 仍显示 `+N/-M` 徽章，但 `DiffView` 退化为“文件过大，仅显示统计”，`View diff` 按钮隐藏；
- 宿主不再自己重算 diff；`tomcat-diff://` 虚拟文档也不再把 `toolCallId` 塞进 URI authority（会被 VS Code 小写化），而是编码进 path 段作为稳定键，所以不会再出现“点 `View diff` 后左右都空白”的大小写竞态。

---

## 4. 时间线模型：为什么 thinking / tool / approval 都是一等项

> 专业：`state.ts` 把 webview transcript 建模为 `WebviewTimelineItem[]`，而不是“消息气泡里嵌杂所有附属状态”。这样历史回放与实时流式能落到同一套渲染语义。
>
> 说人话：thinking、工具结果、审批、计划文件都不是 message 的注释，而是跟 message 平级的聊天时间线节点。

timeline 类型：

| 类型 | 来源 | React 组件 | 备注 |
|------|------|------------|------|
| `message` | 历史 `role:user/assistant`；实时 `content_delta`；系统 notice/error | `MessageBubble` | `kind` 区分 `user / assistant / notice / error`。 |
| `thinking` | 历史 `thinking_trace` / `message.thinking_text` / `reasoning_continuation.fallback_text`；实时 `thinking_delta` | `ThinkingBlock` | 与 assistant 平级，保证“先思考、再回答”。 |
| `tool` | 实时 `tool_execution_*`；历史 `role:tool` | `ToolRow` / `ThinkingGroup` | timeline 里仍是一等 `tool` 项，但 assistant 回复内部会二次分层成 `action` 恒显行 与 `context` 折叠盒。 |
| `approval` | `control_request.ask_question` | `ApprovalCard` | 宿主 resolve 后 `resolved=true`，UI 自动消失。 |
| `plan` | `plan.*` 事件 | `PlanFileCard` | transcript 内保留 plan 文件足迹；执行中的 todo 状态另由底部 `TodoListWidget` 承接。 |

### 4.1 历史水合：`hydrateHistory()`

[`src/ui/webview/state.ts`](../../../src/ui/webview/state.ts) 现在有两个关键实现点：

1. **assistant 历史条目拆成两个 timeline 节点**
   - `extractThinkingText()` 先读 `thinking_text`，再读 `reasoning_continuation.fallback_text`。
   - `parseHistoryEntry()` 遇到 assistant 时，先产出 `thinking`，再产出 `assistant message`。

2. **历史工具结果回放成 `tool` 卡**
   - 先扫描历史 assistant 的 `message.tool_calls[].{id,function.name}` 建 `toolCallId -> toolName` lookup。
   - 再把历史 `role:"tool"` 映射成 `WebviewToolCard`，保留 `toolCallId` 和工具名。

3. **checkpoint 边界不再写回 host timeline，而是在 GUI 渲染前临时投影**
   - `state.ts.setCheckpoints()` 现在只更新 `session.checkpoints`，不再触发 `rebuildHistoryTimeline()`；`state.ts` 里的 raw `timeline` 只保留 message / thinking / tool / plan / approval / boundary 等“事实节点”。
   - `TranscriptView.tsx` 在 render 前执行 `useMemo(injectCheckpointMarkers(timeline, checkpoints))`；若锚点 assistant 只有 tool/thinking、没有正文 message，则回退 `${messageAnchor}-thinking`，保证 marker 仍能落在下一条 user message 之前。
   - 这让 `refreshCheckpoints()` 对 live timeline 没有副作用：它只换一份 checkpoint 数据，最新一轮 user/assistant 不会因“刷 marker”被重建丢失。
   - marker 是否存在仍由后端 `list_checkpoints` 真相决定；后端计数现改为 `git ls-files --cached --others --exclude-standard`，所以 `.gitignore` 与 `DEFAULT_EXCLUDE_RULES`（如 `target/` / `node_modules/`）都不会误占上限，而“只改 ignored 文件”的 turn 也不会新建 marker。

### 4.2 历史/实时去重：为什么改成稳定 key

旧实现只按文本指纹合并，问题是：

- 历史里没有独立 `thinking_trace` 时，会丢 thinking；
- 实时 thinking 往往会被追加到 assistant 后面；
- 历史 `role:"tool"` 与实时 `tool_execution_end` 不能稳定合并。

现在的做法是：

```text
message  -> message:id + message:text
thinking -> thinking:id + thinking:text
tool     -> tool:toolCallId
approval -> approval:requestId
plan     -> plan:path:planId:state
```

`hydrateHistory()` 先产出 history items，再用上面的 merge keys 过滤掉已被历史覆盖的 live items，因此：

- assistant / thinking 仍可在不同 ID 场景下靠文本兜底去重；
- tool / approval / plan 走稳定业务 key；
- “先思考再回答”顺序可以在历史与实时两条路径上统一。

### 4.3 实时流式：`appendStreamingMessage()`

实时阶段同样补了顺序修正：

- `content_delta`：沿用“找到当前 assistant 节点并 append”的逻辑。
- `thinking_delta`：若本轮 assistant 已存在，则把新建 thinking `splice` 到该 assistant 之前，而不是无脑 `push` 到尾部。

这解决了用户看到的第三个问题：**thinking 不该显示在 assistant message 下方**。

### 4.4 assistant-response 二次分层：为什么 action 要恒显

> 专业：`groupTimelineByAssistantResponse()` 先把“同一轮 assistant 回复”的 `preamble / thinking / tools[]` 收成一个逻辑组；随后 [`gui/src/components/TranscriptView.tsx`](../../../gui/src/components/TranscriptView.tsx) 再执行一次 `partitionAssistantResponseGroup()`，把组内工具按信息价值拆成 `action` 与 `context/other` 两层。
>
> 说人话：不是所有工具都值得占据第一屏。真正影响用户决策的是“改了什么文件、跑了什么命令、问了什么问题”；read/search 这类铺垫信息应该降噪收起，但不能打乱时间顺序。

当前实现的心智模型：

```text
assistant response group
  ├─ preamble message
  ├─ [ThinkingGroup]
  │    └─ thinking + context/other tools (collapsed)
  ├─ [ToolRow standalone] edit / write / hashline_edit
  ├─ [ToolRow standalone] bash / shell / execute_command
  ├─ [ToolRow standalone] ask_question
  └─ [ThinkingGroup]
       └─ trailing context/other tools (collapsed)
```

冲刷算法（flush）：

```text
按时间序遍历 tools[]
  ├─ 遇到 context/other -> 放进 buffer
  ├─ 遇到 action       -> 先把 buffer 冲刷成 ThinkingGroup，再渲染 action ToolRow
  └─ 结束              -> 冲刷剩余 buffer
```

`toolCategory()` 当前分层：

- `edit`：`edit / write / hashline_edit`
- `command`：`bash / shell / execute_command`
- `answer`：`ask_question`
- `context`：`read / read_file / grep / search_files / search_workspace / list_dir / web_search / web_fetch / load_skill`
- `other`：`create_plan / update_plan / todos / config_get / config_set` 等无专属 UI 的工具

这样做的结果是：

1. 高信号动作永远不再被整体折叠隐藏。
2. 低信号上下文仍能按时间顺序回看，只是默认降噪。
3. 宿主 E2E 可以直接断言 `actionToolRowCount / groupFoldTitles / commandBlockCount / editDiffBadgeCount`，不再靠脆弱的“整卡是否展开”猜 UI 状态。

---

## 5. 关键交互细节

### 5.1 自动滚动：只在“该跟随时”跟随

实现位于 [`gui/src/useAutoScroll.ts`](../../../gui/src/useAutoScroll.ts)。

机制：

1. `ResizeObserver` 观察滚动容器及其直接子节点；
2. 用户仍贴底时，内容变高就 `scrollTop = scrollHeight`；
3. `scroll` 监听根据 `|scrollHeight - scrollTop - clientHeight| < 2` 判断是否贴底；
4. reveal 触发不再依赖“当前帧最后一项恰好是 user”，而是看 **`latestUserMessageId` 是否变化**，并额外要求 `oldestItemKey` **未变**（同一会话内的“追加”，区别于会话切换 / 历史翻页）且 `userMessageCount` **没有减少**（拒绝 restore 截断）。这样即使 host 同一帧把“新 user + 第一条 thinking”一起发来，也不会漏掉 reveal。
   - **单一 effect、单一 tracking ref（关键修复）**：早先“会话重置”与“新 user reveal”是两个各自持有 `previous*Ref` 的 `useLayoutEffect`。当 `resetKey`（= 活动 sessionId）与 `latestUserMessageId` **同一次 commit 一起变化**（会话切换、webview 重挂后立即发送、或 `activeSession` 短暂 flicker）时，先声明的 reset effect 会把共享 ref 洗成“已见过”，导致后声明的 reveal effect 判定“没变化”而**静默吞掉 reveal**——这类竞态在 jsdom mock 布局里测不出、只在真机时序暴露（0.1.12 即栽在此）。现改为**合并为单一 `useLayoutEffect`**（deps: `resetKey / latestUserMessageId / oldestItemKey / userMessageCount`），用同一个 `revealTrackingRef` 做判定：`resetKey` 变化 → 走“会话加载/切换”分支（落底、不 reveal，且状态确定不残留 spacer）；否则才判断是否 reveal。这样 reset 分支是**权威的、不会被 reveal 触发清洗**，reveal 也不再依赖两个 effect 的声明顺序。
5. 触发 reveal 时，`useAutoScroll.ts` 先把**当前轮 user message 滚到视口顶部**，再按“当前轮剩余高度”补一个底部 spacer，让回答先在它下方流式生长。
6. 当当前轮内容长到**超过一屏**时，hook 会把 spacer 收缩到 0，并自动从 `revealUser` 切回 `followBottom`；这样最新 token 继续留在视口底部可见，而不是把用户永久钉死在顶部。
   - **视口高度变化时重新固顶（关键修复）**：当 `busy` 翻转（发送 → 进行态）导致 composer 变高、进而 stream 容器 `clientHeight` 变大时，reveal spacer 仍按旧（更矮）视口计算就会“不够”——浏览器把 `scrollTop` 钳到底、随之而来的 `scroll` 事件把模式翻成 `followBottom`，reveal 当场丢失（0.1.13 即栽在此，只在真机、且要 `busy` 从 false→true 翻转才暴露）。现在 reveal 对视口高度变化免疫：`ResizeObserver` 每次检测到容器 `clientHeight` 变化就**重算 spacer 并重新固顶**（既能收缩、也能把 spacer **重新增大**）；万一钳底的 `scroll` 事件先到，只要当前轮仍能装进视口（`latestTurnHeight <= clientHeight`）就**重新固顶而非切 followBottom**，只有真正超一屏才交给 follow-bottom。
7. `App.tsx` 在 `userHasScrolled=true` 时显示 `Jump to latest` 向下箭头图标按钮（保留 `scroll-to-bottom` test id，弱化视觉重量）；
8. sticky prompt 不再只认“最后一条 user message”，而是扫描 transcript 里**所有** user message，先找出“当前视口顶部属于哪一轮”（`top <= scrollTop + threshold` 的 user message 里 `top` 最大者），再判断这一轮自己的 user message 是否已**完全**滚出顶部（`bottom <= scrollTop + threshold`）；只有完全滚出时才有资格显示 sticky。
9. 在此基础上，再加一条更贴近真实对话流的保护：**只要最新一轮 user message 仍在屏幕内可见（顶部或底部都算），sticky 就保持隐藏**，绝不悬浮更旧的问题。这样既覆盖“新问题被 reveal 到顶部”的情况，也覆盖“新问题留在底部、回答在其下方流式生长”的真机情况。
10. 因此，发出**新的提示词**时，旧 sticky 会立即消失；新问题先被 reveal 到顶部，等它被自己的回答顶出一屏后，sticky 再接棒显示当前轮问题。向上翻历史时，若最新问题已在屏幕外，sticky 会按视口实际落在哪一轮而切换；滚到第一条 user 之上时自动消失；经过某一轮 user message 头部的瞬间，sticky 仍会短暂隐藏，等该 user message 完全滚出顶部后再显示该轮问题。

当前这套滚动/吸顶逻辑有三个必须守住的不变量：

- **当前轮 sticky 不会被旧轮抑制规则误伤**：旧轮抑制只拦“更老的问题”，不会把 `owning===newest` 的当前轮 sticky 也一起吞掉。
- **reveal 只由“新 user 到来”驱动**：tool / notice / thinking 流式更新不会把用户强行拉回顶部或底部。
- **超一屏后优先保住最新 token 可见**：一旦当前轮超过一屏，系统宁可切回 follow-bottom，也不会继续把用户钉在顶部看不到新输出。
- **reset 权威、reveal 不被同帧 resetKey 变化吞掉**：`resetKey` 与 `latestUserMessageId` 同帧变化时，reset 分支胜出（落底），reveal 触发绝不因为共享 ref 被清洗而丢失；同一会话内发送（`resetKey`/`oldestItemKey` 不变）则必定 reveal，即使紧跟在一次会话加载之后。
- **reveal 对视口高度变化免疫**：`busy` 翻转 / composer 变高导致 stream 容器 `clientHeight` 变化时，reveal 会重算 spacer 并重新固顶，不会因为视口变大被钳到底而误切 follow-bottom；只有当前轮真正超过一屏才交给 follow-bottom。

> 纯布局/时序类真因（如上面两条 reveal 真因：同帧 reset 竞态、视口高度变化钳底）无法靠 jsdom mock 布局取证：`getBoundingClientRect/scrollHeight/clientHeight` 全是假值且恒定，单测恒绿。定位手段是**真实浏览器 smoke**——把**生产构建** `gui/dist`（`vite build` 产物，与真机 webview 完全一致的压缩 React 生产版）用静态服务器起起来（而非只跑 `npm run dev` 的开发版），用 `window.postMessage` 灌真实 `state` 帧序列，且必须复刻真机时序：`busy=false` 的 echo 帧（reveal 到顶）→ `busy=true` 翻转帧（composer 变高、`clientHeight` 变化）→ 流式 thinking/tool。再用 CDP 读真实 `scrollTop / clientHeight / scrollHeight / spacer` 与 `[data-message-kind="user"]` 的 `getBoundingClientRect().top` 验证 reveal 是否**在 `busy` 翻转后仍保持置顶**。逻辑不变量则用 `useAutoScroll.test.tsx` 以 store 状态（props）+ 可变 `clientHeight` 驱动锁定，不 mock 触发条件。

为什么不用虚拟列表：

- 当前 webview transcript 规模小；
- 相比引入 `react-virtuoso`，这一版更容易和现有 DOM、测试、宿主 DOM snapshot 机制对齐；
- 但交互语义（上滑暂停、底部恢复）已经对齐 `cline/continue`。

### 5.2 Thinking：独立卡片，不并入 assistant 气泡

thinking 仍保持独立卡，而不是嵌入 assistant 气泡，原因有三：

1. 与 `state.ts` 的 timeline 一等项模型一致；
2. 历史与实时更容易复用同一条渲染路径；
3. 后续若引入更多 reasoning 元信息（耗时、脱敏、可复制等）更容易扩展。

[`gui/src/components/TranscriptView.tsx`](../../../gui/src/components/TranscriptView.tsx) 不再对**整条 transcript**找“最后一个 thinking”，而是只在 `liveClusterTimeline`（最后一条 user 之后、且 `showProgress=true` 的那一组）内部找 `clusterLastThinkingId`。也就是说：

- 旧轮 thinking 永远是过去态，不会因为全局 `busy=true` 又重新转圈；
- leading/history cluster 的 `ThinkingGroup` 不会被新一轮的 busy 连坐成 shimmer；
- 只有当前 live cluster 里最后一个 thinking 才有资格拿到 `isStreaming=true`。

### 5.3 Transcript 工具行：标题恒显，结果体按类型折叠

> 专业：这次优化后，Tomcat 不再把所有工具都画成同一种 `ToolCallCard`。统一入口仍是 [`gui/src/components/ToolRow.tsx`](../../../gui/src/components/ToolRow.tsx)，但视觉与展开规则由 `toolCategory()` 驱动。
>
> 说人话：用户第一眼应该先看到“发生了什么”，再决定要不要看细节；不是先看到一堆长得一样的白字卡片。

状态收敛上还有一条更底层的不变量：**`agent_idle` 到来时，界面里不允许再残留任何“进行中”工具态**。因此 [`src/ui/webview/state.ts`](../../../src/ui/webview/state.ts) 在 `agent_idle` 分支会执行 `settleRunningTools(session)`，把残留的 `running/streaming` 工具卡统一收敛为 `complete`，但保留原有 `summary` / `isError`。这保证“上一轮 edit 还显示 Editing…”这类 UI 泄漏不会跨轮延续。

视觉/交互模型：

```text
ToolRow
  ├─ edit/write
  │    └─ DisclosureCard
  │         ├─ header  = 动词 + FileChip + +N/-N 徽章 + View diff
  │         ├─ preview = DiffView.changeAnchoredPreview(5)
  │         └─ body    = DiffView
  ├─ command
  │    └─ DisclosureCard
  │         ├─ header  = 目的短句(utility-flash) + 命令名标签(git, echo)
  │         ├─ preview = TerminalOutput(command=$完整命令).tail(5)
  │         └─ body    = TerminalOutput(command=$完整命令)
  ├─ answer  -> 直接挂 AnswerCard，始终展开
  └─ context -> 极简单行；单条 read/search 可直接扁平直出
```

各类规则：

1. **edit**
   - 只认 `edit / write / hashline_edit`，与 transcript 内部的 mutation 语义对齐。
   - diff 徽章与逐行 diff 都来自核心 `ToolDisplay::File.added/removed/diff`，经 [`src/serveClient/wire.d.ts`](../../../src/serveClient/wire.d.ts) → [`src/ui/webview/state.ts`](../../../src/ui/webview/state.ts) 直达 GUI。
   - 有 `diff` 时，`ToolRow` 会装配 `DisclosureCard(body=DiffView)`：折叠态不是看“文件尾部 5 行”，而是围绕**第一处真实改动**取迷你预览；展开态看完整结构化 diff（最大半屏、高度内滚动）。
   - `toolCallId + diff` 同时存在时，卡片右上角显示 `View diff` 图标按钮；点击发 `openDiff` intent。
   - 宿主 [`src/ui/webview/provider.ts`](../../../src/ui/webview/provider.ts) 会按 `ctx+del` 重建 before、按 `ctx+add` 重建 after，再通过 [`src/ide/VsCodeIde.ts`](../../../src/ide/VsCodeIde.ts) 复用既有 `tomcat-diff://` + `vscode.diff` 原生链路打开 diff 编辑器；虚拟文档键现编码进 URI path，规避 authority 被小写化后的空白 diff。
   - 大文件拿不到 `diff` 时，仍保留 `+N/-M` 徽章，但 `DiffView` 只显示 fallback 提示，`View diff` 自动隐藏。

2. **command**（抄 Cursor 的终端观感，见 §5.3.1）
   - `bash / shell / execute_command` 作为 standalone action 行常驻。
   - 卡片头**不再塞完整命令**：改成「目的短句 + 命令名标签」。
     - **目的短句**由后端 utility 模型（`utility-flash`）在命令执行完后**异步**生成，经 `tool.summary_updated` 事件按 `toolCallId` 热更新（见 §5.3.1 数据流）；短句到达前用确定性占位动词（运行中 `Running` / 完成 `Ran` / 中断 `Interrupted`）。
     - **命令名标签**（灰字、逗号分隔，如 `git, echo`）由 GUI 端零 LLM 解析：`commandBinaries(cmd)` 按 `&& || | ; \n` 切段，跳过注释与 heredoc 正文，取每段首个可执行名，剔除 `VAR=…` 环境赋值与 `sudo`，去重、上限 3 个。
   - **完整命令下沉到正文**：`TerminalOutput` 支持可选 `command` prop，在输出前面加一行 `$ <完整命令>` 提示行（终端观感），预览态与展开态都带。
   - 用 `DisclosureCard(header=目的短句+标签, preview/body=TerminalOutput)` 统一折叠行为。
   - `complete && !isError` 默认折叠；`running / isError` 默认展开。
   - 折叠态不是“什么都不看见”，而是直接给 `$ 命令` + 尾部 5 行 preview；展开态上/下/左/右都可滚动，避免长命令输出把 transcript 拉爆。
   - **无输出的 command 走扁平行**（没有正文可承载命令），此时仍把命令 `<code>` 内联保留，目的短句到达后作为前缀，信息不丢。

3. **answer**
   - `ask_question` 的已回答态不再躲在折叠体里，而是直接渲染 [`AnswerCard.tsx`](../../../gui/src/components/AnswerCard.tsx)。
   - 这和待回答态 [`ApprovalCard.tsx`](../../../gui/src/components/ApprovalCard.tsx) 形成前后两段：前者让用户答题，后者保留 transcript 证据。

4. **context / other**
   - `read/search/web_*` 等保持小图标 + 描述色的一行摘要。
   - 连续多个会被 `ThinkingGroup` 收纳，避免 transcript 变成工具日志墙；单个无 thinking 的 context 工具直接扁平显示，保留 `FileChip` 与配色。
   - `read / read_file` 前导图标改成 `codicon-eye`，避免和 Markdown `FileChip` 的书本图标撞语义。
  - `create_plan / update_plan` 在 grouped 场景采用 Variant B：分组头仍可显示 `Creating plan`（或更具体的 thinking summary），但 transcript 里任何**非 error** 的 plan 工具行都会被抑制，避免“分组头 + 内层行”双重重复。关键点是抑制判据不能只看 `display.kind === "plan"`，因为运行中的 plan 工具直到 `tool_execution_end` 才拿到 `display`；真正可靠的真源是 `toolName === create_plan/update_plan`（再兼容结束态 `display.kind === "plan"`）。

5. **plan 卡片**
   - `PlanFileCard` 始终是 plan 文件的正式足迹：文件名、语义标题、todo 数、`View Plan / Build` 都在这里。
   - 当 `create_plan / update_plan` 仍处于 `running / streaming` 时，卡片底部 `View Plan` 会切成呼吸省略号按钮（disabled + `aria-busy=true`）；完成后恢复普通 `View Plan`。卡片优先按 `planId` 与运行中的工具匹配；只有工具暂时拿不到 `planId/path` 时，才兜底点亮当前 cluster 里的最新 plan 卡。

还有三个实现细节很关键：

- 结果体仍是懒挂载，展开前不进 DOM，减少长输出的布局压力。
- thinking-only 残组不会继续拿 `summaryTitle` 当折叠标题，避免命令标题在 action 行和折叠头各出现一次。
- `DisclosureCard` 是内容无关外壳，terminal / diff 细节全部留给 `TerminalOutput` / `DiffView`；这比在一个万能组件里堆 `mode` 开关更稳。
- transcript 外层 `.tc-stream` 现在只允许**纵向**滚动；消息文本、cluster 容器和其直接子节点都强制 `min-width: 0` + `overflow-wrap: anywhere`，所以 Markdown 里的长横杠分隔线、长文件名或其他无空格 token 只会在局部断行，不会再把整条 transcript 横向撑出视口。真正需要横向滚的只有 diff / terminal / code block 这类局部内容体。

#### 5.3.1 bash 卡片标题：`tool.summary_updated` 异步升级（抄 Cursor）

**为什么**：过去 bash 卡片标题是硬编码的 `Ran <命令首行>`，命令一长标题就爆。Cursor 的做法是标题只放「这条命令想干嘛」的短句，命令本身留给终端正文。

**怎么做**：复用 turn summary 那套「先占位、后升级」——命令执行不被阻塞，标题事后被 utility 模型的短句覆盖。

```text
agent_loop            tool_dispatcher                utility-flash        ext host(state.ts)   ToolRow
   │  run bash              │                              │                    │                │
   │───────────────────────▶  ToolExecutionEnd(命令+输出) ──────────────────────▶ 卡片(占位: Ran + 命令名标签) ──▶ 渲染
   │                        │  spawn 目的短句(fire&forget)  │                    │                │
   │                        │─────────────────────────────▶│                    │                │
   │                        │                              │  tool.summary_updated {toolCallId, summaryTitle}
   │                        │                              │───────────────────▶ 按 toolCallId 写 tool.summaryTitle ──▶ 头部升级为目的短句
```

- **后端**：[`tool_dispatcher.rs`](../../../../tomcat/src/core/agent_loop/tool_dispatcher.rs) 发出 `ToolExecutionEnd` 后，对 `bash/shell/execute_command` 调 [`tool_summary_update::maybe_spawn_tool_summary_update`](../../../../tomcat/src/core/agent_loop/tool_summary_update.rs)（`tokio::spawn`，8s 超时，复用 `title_provider/title_model/emitter`）。短句由 [`generate_command_summary`](../../../../tomcat/src/core/summary/title_generator.rs)（祈使句 2–6 词）产出，失败回落 `Run <首个命令名>`。
- **事件**：新增 `WIRE_TOOL_SUMMARY_UPDATED = "tool.summary_updated"`（[`infra/events/mod.rs`](../../../../tomcat/src/infra/events/mod.rs)）与 `ServeToolEvent::ToolSummaryUpdated { sessionId?, toolCallId, summaryTitle? }`（[`api/serve/types.rs`](../../../../tomcat/src/api/serve/types.rs)，并入 `ServeEvent` union）；`serve --print-schema` 已同步到 [`wire.d.ts`](../../../src/serveClient/wire.d.ts) 与 serve fixture。注意 **serve 侧还要把它列进 [`event_pump.rs`](../../../../tomcat/src/api/serve/event_pump.rs) 的 `EVENT_NAMES` 白名单**，否则后端虽然 emit 了 `tool.summary_updated`，插件/webview 仍收不到。
- **宿主**：[`state.ts`](../../../src/ui/webview/state.ts) 新增 `case "tool.summary_updated"`：按 `toolCallId` 找到工具卡片写 `summaryTitle`（未命中则忽略）。与 `turn.summary_updated`（写 thinking 分组头）互不干扰。
- **已知限制（v1）**：per-tool 摘要**只在 live 生效、不回写 transcript**。历史重载时 bash 卡片回落到确定性占位（`Ran` + 命令名标签 + 正文 `$ 命令`），不会重放短句；持久化留作后续项。这与 turn summary 会回写 assistant message 的做法不同，是刻意的成本权衡（每条前台 bash 已多一次 utility 轻量调用）。

### 5.4 Composer：不换行、只压缩可压缩项

[`gui/src/components/Composer.tsx`](../../../gui/src/components/Composer.tsx) + [`gui/src/styles.css`](../../../gui/src/styles.css) 现在采用：

- `.tc-composer__bar { flex-wrap: nowrap; overflow: hidden; }`
- `Model` 是唯一主要弹性项：`flex: 1 1 auto; min-width: 0`
- `Mode`、`Ctx`、`+`、`Send` 都是固定/弱弹性项
- 窄宽度隐藏字段标签（保留下拉本体），避免“Mode / Model”文字把布局挤乱

目标不是“所有内容永远完整显示”，而是：

1. 固定控件位置不乱；
2. 模型名优先被压缩；
3. sidebar 横向缩放时布局语义保持稳定，尽量贴近 VS Code Chat。

---

## 6. 样式与主题约定

> 专业：当前 GUI 不引入 CSS-in-JS，统一用 `styles.css` 中的 `tc-*` BEM-ish 命名；颜色全部走 VSCode 主题变量。
>
> 说人话：样式集中在一张表里，靠 `--vscode-*` 变量吃宿主主题，不在组件里散落硬编码颜色。

约定：

- 所有类名前缀统一 `tc-`，避免污染宿主或外部 CSS；
- 面板背景、输入框、按钮、描述色都来自 `--vscode-*`；
- 组件层只关心结构 class，不在 TSX 里拼复杂行内样式；
- Transcript 工具行的语义色统一走主题 token：编辑类优先 `--vscode-chat-linesAddedForeground / --vscode-chat-linesRemovedForeground`，DiffView 背景优先 `--vscode-diffEditor-insertedLineBackground / --vscode-diffEditor-removedLineBackground`，命令文本优先 `--vscode-textPreformat-foreground`（淡黄），命令块优先 `--vscode-panel-background / --vscode-terminal-foreground`；
- 本次新增的交互动效也留在 CSS：`tc-thinking-pulse`、`tc-tool-spin`。
- standalone 工具行本身保持扁平：`DisclosureCard` 去掉旧式厚圆角盒感，只保留左侧状态条、轻量边框和 hover/expanded 差异，把视觉重量留给文件 chip、diff 文本和 terminal block。

---

## 7. 测试与验收

自动化测试：

| 文件 | 覆盖点 |
|------|--------|
| [`gui/src/useAutoScroll.test.tsx`](../../../gui/src/useAutoScroll.test.tsx) | 贴底跟随、上滑暂停、session 切换与 user message 重置 |
| [`gui/src/App.test.tsx`](../../../gui/src/App.test.tsx) | composer/DOM snapshot 埋点接线、跳底箭头按钮、上一轮进行态收尾，以及“reveal 到顶 → 超一屏切回当前 sticky”整链路 |
| [`gui/src/components/DisclosureCard.test.tsx`](../../../gui/src/components/DisclosureCard.test.tsx) | 折叠/展开外壳、preview/body 切换 |
| [`gui/src/components/DiffView.test.tsx`](../../../gui/src/components/DiffView.test.tsx) | 行号列、加删底色、长 context 折叠、大文件 fallback |
| [`gui/src/components/ToolRow.test.tsx`](../../../gui/src/components/ToolRow.test.tsx) | edit diff 徽章 + View diff 按钮、command disclosure、answer/context 渲染语义、read 图标去重 |
| [`gui/src/components/TranscriptView.partition.test.ts`](../../../gui/src/components/TranscriptView.partition.test.ts) | assistant-response 冲刷算法（context/action 交错边界） |
| [`gui/src/components/TranscriptView.test.tsx`](../../../gui/src/components/TranscriptView.test.tsx) | 单 context 工具直出、action/context 分层、旧轮 thinking 不被新一轮 busy 连坐成 streaming |
| [`gui/src/components/markdown/ChatMarkdown.test.tsx`](../../../gui/src/components/markdown/ChatMarkdown.test.tsx) | assistant 正文 markdown 富渲染：标题/普通段落、代码卡片（bare / 路径头）、inline path、copy、普通 `<a>`、sanitize、未闭合围栏；**流式过程中代码块同步出现 `code.hljs`、mermaid 仍异步；追加尾块只重算新块（按块 memo）** |
| [`gui/src/components/markdown/markdownRuntime.test.ts`](../../../gui/src/components/markdown/markdownRuntime.test.ts) / [`gui/src/components/markdown/richRenderRuntime.test.ts`](../../../gui/src/components/markdown/richRenderRuntime.test.ts) | `splitTopLevelBlocks()` 过滤 `space` token / 未闭合围栏尾块、`highlightToHtml()` 在模块加载期完成语言注册、别名与未知语言回退 |
| [`gui/src/components/ThinkingGroup.test.tsx`](../../../gui/src/components/ThinkingGroup.test.tsx) | thinking-only 残组不复用 `summaryTitle` |
| [`src/ui/webview/tests/dual_channel.test.ts`](../../../src/ui/webview/tests/dual_channel.test.ts) | thinking 在 assistant 前、历史 `role:tool` → 工具卡、历史/实时去重 |
| [`src/ui/webview/tests/provider.test.ts`](../../../src/ui/webview/tests/provider.test.ts) | mutation 工具结束后从 `display.added/removed/diff` 注入 `diffStat/tool.diff`、errored tool 收敛为 `complete+error`，以及 `openDiff -> ide.openReconstructedDiff` 路由 |
| [`src/ui/webview/tests/state.test.ts`](../../../src/ui/webview/tests/state.test.ts) | `agent_idle` 收敛残留 `running/streaming` 工具卡，并保留 `summary/isError` |
| [`src/ide/tests/diff_apply_edit.test.ts`](../../../src/ide/tests/diff_apply_edit.test.ts) | `openReconstructedDiff()` 复用原生虚拟文档 diff 链路 |
| [`src/ui/planPreview/tests/planDocument.test.ts`](../../../src/ui/planPreview/tests/planDocument.test.ts) | `.plan.md` 解析：四态 todos、缺 frontmatter、`name`/`goal` 回退、`bodyMarkdown` 剥离 `## Todos Board`、CRLF；**`bodyLineMap` 源码行映射（frontmatter 偏移 / 无 frontmatter / board 剪除后非线性 / CRLF）** |
| [`src/ui/planPreview/tests/PlanPreviewEditorProvider.test.ts`](../../../src/ui/planPreview/tests/PlanPreviewEditorProvider.test.ts) | 编辑器 Provider 纯逻辑：`buildState`（buildModel 回退 / canBuild 派生 / init 失败降级 / 帧含 toolbarStyle、**默认 hybrid**）、`handleIntent`（ready/openLink/setBuildModel/build/**addSelectionToChat 含/不含行号**）、`classifyPlanLink`；活动面板机账（伪造 panel 驱动 `runBuildForActive` / `getActivePlanPath` / `getActivePlanInfo` / `onDidChangeActivePlan` / 失焦清理 / **`requestCaptureSelection` 发 `captureSelectionForChat` 事件、无焦点 no-op**）；**`refreshFromServeEvent(planId,pathHint)` 从磁盘重读而非旧缓冲，并支持 canonical path hint 命中** |
| [`tests/contextReferences.test.ts`](../../../tests/contextReferences.test.ts) | `buildSelectionReferenceFromParts`（多行/单行 label、无行号回落文件名、空文本 null、截断）+ `buildSelectionReference` 薄封装复用 |
| [`gui/src/plan/PlanPreviewApp.test.tsx`](../../../gui/src/plan/PlanPreviewApp.test.tsx) / [`gui/src/plan/PlanSelectionActionButton.test.tsx`](../../../gui/src/plan/PlanSelectionActionButton.test.tsx) / [`gui/src/components/PlanActionStrip.test.tsx`](../../../gui/src/components/PlanActionStrip.test.tsx) / [`gui/src/components/PlanFileCard.test.tsx`](../../../gui/src/components/PlanFileCard.test.tsx) / [`gui/src/components/TodoList.test.tsx`](../../../gui/src/components/TodoList.test.tsx) / [`gui/src/components/MarkdownBody.test.tsx`](../../../gui/src/components/MarkdownBody.test.tsx) / [`gui/src/components/PlanBuildModelSelect.test.tsx`](../../../gui/src/components/PlanBuildModelSelect.test.tsx) | 预览渲染顺序（正文→N To-dos→分割线→四态清单）、不渲染 name/overview、**恒渲染 Preview（无 webview 内 markdown/源码视图）**、**hybrid 固定头 strip 是 `plan-content` 的兄弟节点而非后代（滚正文不带走）**、native 无动作条 / hybrid 渲染 `PlanActionStrip`（黄 Build 发 build、下拉发 `setBuildModel`、canBuild=false 禁用）、**strip/卡片/下拉无可见 “Build model”/“Model” 白字但保留 `aria-label`**、**`captureSelectionForChat` 读选区→发 `addSelectionToChat`（选区落在 `data-source-line` 块→带精确行号，落在无映射块/todo→不带行号，空选区不发）**、**`MarkdownBody` 复用 transcript 同一套 `buildDecoratedHtml()`，因此 plan 预览也有同步 `hljs` 代码卡片 / copy / inline path**、**`PlanSelectionActionButton` 有选区显示/空/越界隐藏/滚动隐藏/点击 onAdd**、**`Composer` 同一 `.plan.md` 两个不同无行号选区不互相去重**、`<a>` 拦截 + DOMPurify、`mermaid` 代码块渲染成 SVG（mock）+ 失败回退保留代码块 |
| [`src/ui/webview/tests/provider.test.ts`](../../../src/ui/webview/tests/provider.test.ts)（plan build orchestration + auto-open） | `runPlanBuild` 顺序：配置非空先 `sendSetModel` 再 `sendSetPlanMode`；卡片 `setPlanMode build` 与编辑器 `buildPlan` 共用同一路径；**auto-open：`plan.create` 只登记 `planId -> path`，后续 `plan.review` 才 `ide.openWith(...,"tomcat.planPreview")`，重复 review / 后续 `plan.update` / 无 path / 未登记 planId 不再开；openWith 抛错降级 `showFile`** |
| [`src/test/suite/support/hostE2eScenario.ts`](../../../src/test/suite/support/hostE2eScenario.ts) | 真实宿主 webview streaming/diff/multi-session/ownership 通路；以及 `.plan.md` 自定义编辑器真实 resolve/webview（**`plan.create` 不会抢开，`plan.review` 后才自动弹出预览**；默认 hybrid 出全宽固定头 strip（`stripOutsideContent`、`stripInsetLeft<=2`）且**正文左侧 inset 有留白**（`bodyInsetLeft>=12`） + Preview 四态 + **`viewAsMarkdown` 打开原生文本编辑器（断言 `activeTextEditor` 变该文件）→ `viewAsPreview` 切回预览** + **Agent 外部写盘(`fs.writeFile`) + `plan.update` 事件触发预览热更新，并保持滚动阅读位置** + 建模型落配置 + **选中正文经右键命令/浮动按钮两路 → chat 出现带 `文件名:行号` 的 selection chip（源码行映射）；path3：同一 plan 两个不同「无行号」选区（todo 项）都能落 chip，锁死去重碰撞回归** + 切 native 回归无动作条） |

实际 UI 验收（本次体验优化）：

1. 用 `vite dev` 单独跑 GUI；
2. 浏览器侧注入 mock `state` 帧；
3. 验证：
   - 贴底时新消息自动跟随；
   - 上滑后停止跟随并出现 `Jump to latest`；
   - thinking 展开后位于对应 assistant 回复之前；
   - transcript 中 `action` 工具恒显、`context` 工具进折叠盒；
   - edit/write 行显示核心提供的 `+/-` 徽章、内联彩色 diff 与 `View diff` 按钮，command 行显示 `Ran + 命令` 且折叠态直接给 tail 预览；
   - 单 read 工具文件名有色、前导图标不是书本，bash 命令标题不重复且命令文本为淡黄；
   - 窄宽度下 composer 不再换行错乱。

发布注意：

- 只要 `ToolDisplay` 协议变了，就必须同时做三件事：`npm run gen:wire`、刷新 `tomcat/tests/fixtures/serve/*`、重新跑 `check:wire / serve_schema_fixture`。
- 如果要重新打可安装 `vsix` 给人体验，扩展版本号必须 bump；同版本 `vsix` 在本机 `--install-extension --force` 下未必会真正覆盖旧 UI。

---

## 8. 实现 ↔ 文件对照表

| 关注点 | 主要文件 |
|--------|----------|
| 宿主生命周期 / webview html / postState / postEvent | [`src/ui/webview/provider.ts`](../../../src/ui/webview/provider.ts) |
| 类型 / frame / intent | [`src/ui/webview/protocol.ts`](../../../src/ui/webview/protocol.ts) / [`gui/src/types.ts`](../../../gui/src/types.ts) |
| timeline 合并 / thinking & tool 历史回放 | [`src/ui/webview/state.ts`](../../../src/ui/webview/state.ts) |
| mutation diff 统计 + 结构化 diff | [`../tomcat/src/core/tools/primitive/diff.rs`](../../../../tomcat/src/core/tools/primitive/diff.rs) / [`../tomcat/src/infra/events/mod.rs`](../../../../tomcat/src/infra/events/mod.rs) → [`src/ui/webview/state.ts`](../../../src/ui/webview/state.ts) → [`gui/src/components/ToolRow.tsx`](../../../gui/src/components/ToolRow.tsx) / [`gui/src/components/DiffView.tsx`](../../../gui/src/components/DiffView.tsx) |
| transcript 原生 `View diff` 打开链路 | [`src/ui/webview/provider.ts`](../../../src/ui/webview/provider.ts) / [`src/ide/VsCodeIde.ts`](../../../src/ide/VsCodeIde.ts) |
| 自动滚动与跳底按钮 | [`gui/src/useAutoScroll.ts`](../../../gui/src/useAutoScroll.ts) / [`gui/src/App.tsx`](../../../gui/src/App.tsx) |
| transcript 分发 / action-context 冲刷 | [`gui/src/components/TranscriptView.tsx`](../../../gui/src/components/TranscriptView.tsx) |
| assistant 正文 markdown 富渲染（四次整改） | [`gui/src/components/MessageBubble.tsx`](../../../gui/src/components/MessageBubble.tsx) / [`gui/src/components/markdown/ChatMarkdown.tsx`](../../../gui/src/components/markdown/ChatMarkdown.tsx) / [`gui/src/components/markdown/markdownRuntime.ts`](../../../gui/src/components/markdown/markdownRuntime.ts) / [`gui/src/components/markdown/markdownDecorators.ts`](../../../gui/src/components/markdown/markdownDecorators.ts) / [`gui/src/components/markdown/richRenderRuntime.ts`](../../../gui/src/components/markdown/richRenderRuntime.ts) / [`gui/src/components/MarkdownBody.tsx`](../../../gui/src/components/MarkdownBody.tsx) |
| thinking UI | [`gui/src/components/ThinkingBlock.tsx`](../../../gui/src/components/ThinkingBlock.tsx) |
| 思考/上下文折叠盒 | [`gui/src/components/ThinkingGroup.tsx`](../../../gui/src/components/ThinkingGroup.tsx) |
| 类型化工具行 / disclosure 外壳 / answer 卡 | [`gui/src/components/ToolRow.tsx`](../../../gui/src/components/ToolRow.tsx) / [`gui/src/components/DisclosureCard.tsx`](../../../gui/src/components/DisclosureCard.tsx) / [`gui/src/components/TerminalOutput.tsx`](../../../gui/src/components/TerminalOutput.tsx) / [`gui/src/components/AnswerCard.tsx`](../../../gui/src/components/AnswerCard.tsx) |
| composer 响应式 | [`gui/src/components/Composer.tsx`](../../../gui/src/components/Composer.tsx) / [`gui/src/styles.css`](../../../gui/src/styles.css) |
| 手工验收辅助 no-op 宿主 | [`gui/src/main.tsx`](../../../gui/src/main.tsx) |
| `.plan.md` 自定义编辑器（host） | [`src/ui/planPreview/PlanPreviewEditorProvider.ts`](../../../src/ui/planPreview/PlanPreviewEditorProvider.ts) / [`src/ui/planPreview/planDocument.ts`](../../../src/ui/planPreview/planDocument.ts) / [`src/shared/planPreviewProtocol.ts`](../../../src/shared/planPreviewProtocol.ts) |
| plan 预览 webview（GUI） | [`gui/src/plan/PlanPreviewApp.tsx`](../../../gui/src/plan/PlanPreviewApp.tsx) / [`gui/src/plan/PlanSelectionActionButton.tsx`](../../../gui/src/plan/PlanSelectionActionButton.tsx) / [`gui/src/components/PlanActionStrip.tsx`](../../../gui/src/components/PlanActionStrip.tsx) / [`gui/src/components/TodoList.tsx`](../../../gui/src/components/TodoList.tsx) / [`gui/src/components/MarkdownBody.tsx`](../../../gui/src/components/MarkdownBody.tsx) / [`gui/src/components/PlanBuildModelSelect.tsx`](../../../gui/src/components/PlanBuildModelSelect.tsx) |
| plan 预览原生标题栏（命令 + 上下文键） | [`src/extension.ts`](../../../src/extension.ts)（5 命令 + `setContext`）/ [`package.json`](../../../package.json)（`editor/title` + `webview/context` 菜单 + `tomcat.plan.toolbarStyle`） |
| plan 选中加入聊天 | [`src/ui/webview/contextReferences.ts`](../../../src/ui/webview/contextReferences.ts)（`buildSelectionReferenceFromParts`）/ [`src/extension.ts`](../../../src/extension.ts)（`addSelectionToChat` dep + `tomcat.plan.addSelectionToChat` 命令）/ 源码行映射：[`planDocument.ts`](../../../src/ui/planPreview/planDocument.ts)（`bodyLineMap`）+ [`MarkdownBody.tsx`](../../../gui/src/components/MarkdownBody.tsx)（`data-source-line`）/ 去重修复：[`gui/src/contextReferences.ts`](../../../gui/src/contextReferences.ts)（`referenceIdentity` 无行号追加文字 hash） |
| plan build 统一入口（卡片/编辑器共用） | [`src/ui/webview/provider.ts`](../../../src/ui/webview/provider.ts)（`runPlanBuild` / `buildPlan` / `setBuildModel`）/ [`src/extension.ts`](../../../src/extension.ts)（`registerCustomEditorProvider` + `focusWebviewSurface`） |

---

## 9. 本次 UX 优化小结

本次体验优化没有重构 `TomcatMessenger`，但对 serve 的 file display 做了**小而必要**的协议扩展（新增 `ToolDisplay::File.diff`），其余主要仍在 **webview state 合并层 + React 表现层**完成：

1. 让滚动语义从“只会盲目追底”升级成“理解用户是否在看历史”；
2. 让 thinking 与 tool 在历史回放时和实时阶段保持同一语义；
3. 让 transcript 内部从“所有工具一把梭折叠”升级成“action 恒显、context 收纳、样式类型化、command/edit 统一 DisclosureCard 外壳”；
4. 让 edit diff 真相回到核心：同一份结构化 diff 同时喂给 transcript 内联彩色 diff 与原生 `View diff`，不再依赖扩展读盘时序或 git 工作区状态；
5. 把“协议改了但前端/fixture/安装包没追上”的工程风险显式制度化：`gen:wire`、serve fixture、版本 bump 必须一起做。

这意味着后续若继续迭代 webview UX，大多数样式/分组问题仍应优先在 [`state.ts`](../../../src/ui/webview/state.ts) 和 [`gui/src/**`](../../../gui/src) 内完成；但凡涉及“文件改动真相”（如 diff 行数、before/after 重建），必须优先回到核心事件层处理。

---

## 9a. Transcript assistant 正文富渲染（2026-07-19 四次整改）

> 专业：assistant 正文的 markdown 富渲染没有引入 `react-markdown` 之类全新栈，而是继续保留 `marked + DOMPurify + DOM 装饰` 这条低爆炸半径管线；四次整改只吸收 cline 的两点关键思想：**按 markdown 顶层块切分**、**`highlight.js` 静态 import + 同步上色**。这样既复用了已有代码卡片 / inline path / mermaid / DOMPurify / 测试资产，又从根上消掉了 streaming 时“先白后彩”的 FOUC 与整篇重算卡顿。
>
> 说人话：以前聊天里的代码块是“先冒出来一坨白字，过一会儿再变彩色”，而且模型每吐一个 token，前端都可能把整篇老消息重新处理一遍。现在改成了“按块冻结”——已经完成的块永远不重算，只有最后正在长的尾块会动；同时高亮颜色直接烤进 HTML，所以代码一出现就是彩色的。`mermaid` 仍然慢一点，因为它天生要异步出 SVG。

数据流一图：

```text
Serve assistant message / history replay
        │
        ▼
TranscriptView
  └─ MessageBubble
      └─ ChatMarkdown(markdown)
           ├─ splitTopLevelBlocks(markdown)              ← marked lexer, 过滤 space token
           ├─ blocks.map(raw => <ChatMarkdownBlock raw>) ← React.memo(只收 raw 单 prop)
           │      ├─ closeOpenFenceIfNeeded(raw)
           │      ├─ buildDecoratedHtml(raw)
           │      │    ├─ renderMarkdownHtml(marked)
           │      │    ├─ DOMPurify
           │      │    ├─ decorateCodeCards
           │      │    │    └─ highlightToHtml(code, language)  ← 静态 import, 同步产出 hljs span
           │      │    └─ linkifyInlineFilePaths
           │      ├─ dangerouslySetInnerHTML(首帧即成品, 无 FOUC)
           │      └─ renderMermaidBlocks(ref)            ← 仅 mermaid 异步
           └─ 父级 onClick 事件委托
                ├─ copy
                ├─ data-tc-file-path → openFile(path, line)
                └─ 普通 <a> → openLink
```

关键约束（这轮为什么这样拍板）：

1. **同步高亮必须和按块 memo 成对出现**：只把 `highlight.js` 改成同步, streaming 时仍会每 token 重高亮整篇旧消息,把之前异步防抖规避掉的 O(n²) 卡顿又请回来。现在 `splitTopLevelBlocks()` + `ChatMarkdownBlock`(只收 `raw`) 把成本限制在尾块,同步上色才真正付得起。
2. **子块只收 `raw` 一个 prop**：点击交互继续放在父级事件委托,避免函数 prop 让 `React.memo` 失效。
3. **代码高亮前移到字符串阶段**：[`richRenderRuntime.ts`](../../../gui/src/components/markdown/richRenderRuntime.ts) 模块加载时注册 `highlight.js` core + 语言; [`markdownDecorators.ts`](../../../gui/src/components/markdown/markdownDecorators.ts) 的 `decorateCodeCards()` 同步写入 `code.innerHTML` 与 `hljs` class。结果是第一帧 HTML 就自带颜色,不再依赖 `useEffect`“术后补丁”。
4. **`MarkdownBody` 与 transcript 共享同一装饰管线**：计划预览以前只有 inline path,没有代码卡片/同步高亮。现在它也走 `buildDecoratedHtml(markdown, sourceLineMap?)`,于是聊天与 plan 预览的代码卡片、copy、inline path 观感一致,同时保留 `data-source-line` 这项 plan 专属能力。
5. **`mermaid` 仍保留异步**：`mermaid.render()` 天生要依赖真实 DOM/SVG,而且包很重;这部分继续留在 `renderMermaidBlocks()` 中按块异步执行。同步的是 `highlight.js`,不是把所有富渲染都粗暴挪进 `useMemo`。
6. **聊天 CSP 已与 plan 预览对齐**：为了让 `highlight.js` / `mermaid` 分包和 SVG 内联样式都能安全工作,聊天 webview 的 CSP 已补上 `'strict-dynamic'` 与 `style-src ... 'unsafe-inline'`;正文仍先过 DOMPurify,脚本执行面没有放开。
7. **代码卡片分两形态**：带 `path[:line]` 的围栏显示 basename 头部并可点击打开;无路径围栏是 `bare` 卡片,只有右上角 copy。正文里的 `` `path:line` `` 会 linkify 成 basename chip,点击走同一套 `openFile(path, line?)`。
8. **`isStreaming` 已从富渲染链拆除**：`ChatMarkdown` / `MessageBubble` / `TranscriptView` 的 assistant 富渲染不再透传 `isStreaming`; streaming 语义只影响真正需要它的 thinking/tool 展示层。

---

## 10. Plan 预览自定义编辑器（`.plan.md`）

> 专业：这是 phase 2 里**第二个** webview 表面——不再是侧边栏的 `WebviewViewProvider`，而是一个 `vscode.CustomTextEditorProvider`（viewType `tomcat.planPreview`，`selector: *.plan.md`）。它照抄 Cursor 的 Plan 预览：自定义编辑器**恒为 Preview**（渲染正文 + 四态清单），全程**只读**（不写回 `.plan.md`），但正文**可选中**，选中的文字能通过浮动按钮或右键菜单**加入 Tomcat 聊天**（复用现有 selection 引用链路）。**没有 webview 内的 Markdown 视图**：标题栏 “...” 里的 **Markdown** 直接 `openWith(uri,"default")` 打开原生文本编辑器；原生文本编辑器的 “...” 里的 **Preview** 再 `openWith(uri,"tomcat.planPreview")` 切回——两态是两个真实编辑器，靠 `vscode.openWith` 互切，而非 webview 内部 `mode`。
>
> 说人话：打开一个 `.plan.md`，看到的不是原始 YAML，而是像 Cursor 那样把 `todos` 画成勾选清单的漂亮预览。顶部路径由 VS Code 自己的标题栏承载——省得我们再画一条重复的路径条。想看/改源码就在 “...” 里点 **Markdown**，直接进原生文本编辑器；在那儿的 “...” 里点 **Preview** 就切回漂亮预览。选中一段正文会浮现一个「Add to Tomcat Chat」小按钮（也可右键），点它就把这段文字塞进侧边栏聊天的输入框当引用。计划文件会在**审稿完成**（serve 发 `plan.review`）后自动打开，不用手点卡片，也不会在 reviewer 还没跑完时抢焦点。
>
> **临时 A/B 开关** `tomcat.plan.toolbarStyle`（enum `hybrid` | `native`，**默认 `hybrid`(B)**）：`hybrid`(B) 标题栏不出 Build/选模型图标，改在预览**顶部渲染一条全宽 `PlanActionStrip` 固定头**——它是滚动正文列**外**的独立行（不是 `sticky`），所以正文再长向下滚**永远不会把它带走**；条本身细、右对齐、半透明 + `backdrop-filter` 模糊，里面是黄色 Build（圆角 5）+ 无边框扁平模型下拉（圆角 4、无可见文字标签、自绘 chevron）；`native`(A) 把 Build/选模型做成标题栏单色图标（点开 QuickPick），正文不出内联条。两种风格下 Markdown/Preview 都收在原生 “...” 溢出菜单里。定稿后删掉另一套 + 移除该开关。

数据流一图（命令 + 上下文键 + host + webview）：

```text
serve plan.create(写盘完成) ─▶ provider.handleServeEvent ─▶ 记录 pendingPlanOpenByPlanId[planId]=path
serve plan.review(审稿完成) ─▶ provider.handleServeEvent ─▶ ide.openWith(path,"tomcat.planPreview")  (每 path 去重, 只自动开一次)
.plan.md 文档 ──vscode.openWith(uri,"tomcat.planPreview")──▶ PlanPreviewEditorProvider.resolveCustomTextEditor()
                                                                     │
原生 editor/title (命令+图标)                                         ├─ parsePlanDocument(text) 唯一解析器
   │ executeCommand                                                  │   title/overview/todos[4态]/bodyMarkdown/raw/planId/state
   ├─ tomcat.plan.build ─────────▶ provider.runBuildForActive()      ├─ buildState(text,path,{toolbarStyle})
   │                                → deps.buildPlan → focus 侧栏     │   + availableModels(sendListModels)+buildModel(配置)+canBuild(能力)
   ├─ tomcat.plan.selectBuildModel▶ showQuickPick → 写 buildModel     ├─ onDidChangeViewState  维护 active panel
   ├─ tomcat.plan.viewAsMarkdown ─▶ openWith(activePlanPath,"default")├─ onDidChangeTextDocument  用户手改/缓冲重载 → 预览热更新
   └─ tomcat.plan.viewAsPreview ──▶ openWith(activeTextUri,          ├─ serve plan.update/plan.todos → provider 桥接 → 从磁盘重读后热更新
        (原生文本编辑器上)             "tomcat.planPreview")            └─ onDidChangeConfiguration  buildModel / toolbarStyle → 回推
                                                                     │
provider.onDidChangeActivePlan ──▶ extension.ts setContext            │  postMessage(state 帧: 含 toolbarStyle)
   tomcat.plan.canBuild ──────────驱动 native Build 图标可见            ▼
                                                          PlanPreviewApp (gui/src/plan)   ← 恒渲染 Preview
                                                           .tc-plan-preview (flex col, overflow:hidden)
                                                           ├─ [hybrid] PlanActionStrip  ← flex:0 0 auto, 全宽固定头(不滚走)
                                                           └─ .tc-plan-preview__content ← flex:1, min-height:0, overflow-y:auto (唯一滚动层)
                                                               ├─ MarkdownBody(marked+DOMPurify+mermaid) → 『N To-dos』计数头 → 分割线 → TodoList(四态 SVG)
                                                               └─ 选中正文(data-source-line→精确行号) → PlanSelectionActionButton(浮动) / 右键命令
                                                                    └─ addSelectionToChat intent ─▶ deps.addSelectionToChat
                                                                         → focus 侧栏 + buildSelectionReferenceFromParts → postInsertReference → chat selection chip(文件名:行号)
```

关键实现约束：

1. **薄 Provider、纯逻辑可测**：`buildState(text, path, ui?)`（文本 + host UI 态 → state 帧）与 `handleIntent(intent, doc, postState)`（意图处理）抽成纯方法，连同 `deriveCanBuild` / `classifyPlanLink` 都不碰真实 webview panel。原生控件的活动面板机账（`onDidChangeViewState` 记 active panel、`runBuildForActive` / `getActivePlanPath` / `getActivePlanInfo`）由单测用**伪造的 `WebviewPanel`** 驱动（[`tests/stubs/vscode.ts`](../../../tests/stubs/vscode.ts) 补了 `onDidChangeTextDocument`，但仍不含 `createWebviewPanel`）。后续又补了 `refreshFromServeEvent(planId,pathHint)`，专门覆盖 Agent 外部写盘场景：触发来自 serve `plan.update`/`plan.todos`，数据源来自磁盘而不是旧 `TextDocument` 缓冲。真实 resolve/webview 由 §7 的 E2E 场景 `assertPlanPreviewCustomEditorFlow` 覆盖。
2. **唯一解析器**：`.plan.md` 的解析全部收敛在 [`planDocument.ts`](../../../src/ui/planPreview/planDocument.ts)；侧边栏卡片用的 `parsePlanFrontmatter` / `readPlanMetadata` 也委托它，`truncatePlanTitle` / `PLAN_TITLE_MAX` / `stripYamlQuotes` 一并下沉，避免两处各写。`bodyMarkdown` 在解析层就剥掉自动维护的 `## Todos Board` 段（标题在 `<!-- todos-board:auto:begin -->` 之上，剥离范围从标题行到 `end` 标记含尾随空行），避免与底部四态清单重复。
3. **Preview 顺序照抄 Cursor**：自上而下 = 渲染后的正文 → 『N To-dos』计数头 → 分割线 → 四态清单；**不渲染** `name`/`overview`（这两个字段仍解析出来给卡片等其它组件用）。四态图标为内联 SVG（pending 空心圈 / in_progress 虚线圈 / cancelled 圈+斜杠 / completed 勾选），尺寸由 `--tc-todo-icon-size` 控制。
4. **原生标题栏承载动作（照抄 Cursor 的复用思路）**：`.plan.md` 以自定义编辑器打开时，用 `when: activeCustomEditorId == 'tomcat.planPreview'` 把命令挂到原生 `editor/title`。因为原生标题栏按钮**只能单色图标**：Build/选模型仅在 `config.tomcat.plan.toolbarStyle == 'native'` 时进 `navigation` 组显示为图标（B 下由正文固定头承载）。Markdown/Preview 放非 navigation 组自动收进 “...” 溢出菜单——但它们不再是「同一编辑器里的两个 mode」，而是**互开对方编辑器**：自定义编辑器活跃时 “...” 只出 **Markdown**（`viewAsMarkdown`）；原生文本编辑器活跃且文件名匹配 `/\.plan\.md$/`（`when: resourceFilename =~ /\.plan\.md$/ && activeEditor == 'workbench.editors.files.textFileEditor'`）时 “...” 只出 **Preview**（`viewAsPreview`）。因此**不需要 `✓` 打勾、也删掉了 `viewAsX.active` 双生命令与 `tomcat.plan.mode` 上下文键**——「哪个编辑器在前台」本身就是当前态。
5. **Markdown/Preview = 两个真实编辑器互切（无 webview mode）**：`viewAsMarkdown`（[`extension.ts`](../../../src/extension.ts)）读 `provider.getActivePlanPath()` → `vscode.openWith(uri,"default")` 打开原生文本编辑器；`viewAsPreview` 读 `vscode.window.activeTextEditor?.document.uri`（校验 `.plan.md` 后缀）→ `vscode.openWith(uri,"tomcat.planPreview")` 切回自定义预览。Provider 只保留 per-panel `canBuild`（不再有 per-panel `mode`），通过 `onDidChangeActivePlan` 通知 `extension.ts` 只 `setContext tomcat.plan.canBuild`；面板失焦或销毁即清理（用 `onDidChangeViewState` 的 `active=false` 兜住「切到普通文本编辑器」）。webview 侧 `PlanPreviewApp` 恒渲染 Preview，`PlanPreviewStateSnapshot` 不再带 `mode`。
   - **自动打开（审稿后才开）**：`plan.create` 只负责暴露 `path`（宿主记 `planId -> path`），真正的打开触发点改为 `plan.review`，因为这才代表 reviewer 已经跑完、用户此时打开看到的是审过的计划。宿主在 [`provider.ts`](../../../src/ui/webview/provider.ts) 的 `handleServeEvent` 里加 `maybeAutoOpenPlanPreview`：`plan.create` 只登记 pending，`plan.review` 再查表 `ide.openWith(path,"tomcat.planPreview")`，并用 `autoOpenedPlanPaths` 按 path 去重。**禁止**在 path 刚出现 / 写盘中 / 后续 `plan.update` 时抢开（改稿不夺焦点）；打开失败降级 `ide.showFile`。
6. **全局唯一 build 模型 + Cursor 扁平下拉**：真源是 VS Code 配置 `tomcat.plan.buildModel`（`scope: application`，空=用会话当前模型）。原生 `selectBuildModel` 用 `showQuickPick`（列 availableModels + “Session default”，当前项 `$(check)`）写回配置；hybrid 内联条与侧边栏 `PlanFileCard` 共用 [`PlanBuildModelSelect`](../../../gui/src/components/PlanBuildModelSelect.tsx)；任一处改动写回配置后，`onDidChangeConfiguration` 让各处 UI 同步。**扁平化**：组件里已**删掉可见文字标签**（原来的 “Build model”/“Model” 白字），`label` prop 只喂 `<select>` 的 `aria-label`（无障碍/测试用）；扁平无边框样式落在组件独有的共享类 `.tc-plan-model-select select`（`appearance:none`、透明背景、圆角 4、自绘 chevron），strip 与卡片**一起变**，且用 `select` 后代选择器把作用域锁在原生下拉，composer 的 `.tc-topbar__trigger` 结构不同故不受影响。
7. **两个 Build 入口零差异**：卡片的 `setPlanMode {action:"build"}` 与编辑器 Build（原生命令 `runBuildForActive` 或 hybrid 内联条 intent）都汇入宿主 [`provider.ts`](../../../src/ui/webview/provider.ts) 的私有 `runPlanBuild(sessionId, planId)`——读配置 → 非空则先 `sendSetModel` 再 `sendSetPlanMode` → 刷新状态；编辑器 Build 额外先 `focusWebviewSurface()` 把侧边栏弹出聚焦，让用户立刻看到 build 进度（照抄 Cursor 体验）。**零 serve 改动**，全部复用既有 RPC。
8. **只读 + 链接不导航**：预览不写回文件（无交互写盘、无 “+ New”、无 Cursor 的 “Save to Workspace”）。正文里的 `<a>` 点击被拦截成 `openLink` intent 交宿主：`http(s)`/`mailto`/带 scheme → `env.openExternal`；仓库相对/绝对路径 → `ide.showFile`（失败兜底 `openExternal`）；纯锚点忽略。
9. **正文里的 `mermaid` 渲染成图（照抄 Cursor）**：[`MarkdownBody`](../../../gui/src/components/MarkdownBody.tsx) 在 marked+DOMPurify 消毒后的 DOM 里找 `code.language-mermaid`，**懒加载** `mermaid`（独立 chunk，无 mermaid 的计划零开销）后 `mermaid.render()` 成 SVG 替换代码块；`securityLevel:"strict"`、主题按 `body.vscode-dark` 派生，渲染失败则保留原代码块（打 `data-mermaid-error`）。为此 CSP 相较 `SettingsPanel` 放宽两处：`script-src 'nonce-…' 'strict-dynamic'`（放行由已授信脚本发起的懒加载 chunk）、`style-src … 'unsafe-inline'`（mermaid 注入 SVG 内联 `<style>` 主题；脚本仍锁死 nonce，正文仍过 DOMPurify，XSS 面不变）。
10. **E2E DOM 取证接线**：Provider 按文档 fsPath 记账 live panel，并新增测试专用 `captureDomSnapshot(path)` / `dispatchDomAction(path, action)`（经 `channel:"event"` 的 `__test.capture_dom` / `__test.dom_action` 与 webview 往返，复用 `PendingMessageTracker`）；宿主 `__testing` 暴露 `openPlanPreview` / `capturePlanPreviewDom` / `dispatchPlanPreviewDomAction` 给场景库调用。因原生标题栏没有 DOM，Build/Markdown/Preview 改用 `executeCommand('tomcat.plan.viewAsMarkdown' | 'viewAsPreview' | 'build')` 驱动、hybrid 内联条仍用 `dispatchDomAction`；**Markdown 不再查 webview 源码**——`viewAsMarkdown` 后场景改断言 `vscode.window.activeTextEditor` 变成该 `.plan.md`（原生编辑器），`viewAsPreview` 再断言预览 `bodyHasContent` 回来。DOM 快照验证正文四态 / `bodyHasContent` / `selectionButtonVisible` / `stripOutsideContent`（固定头结构不变式：strip 是 `plan-content` 的**兄弟**而非其后代）/ `stripInsetLeft`（顶栏仍铺满）/ `bodyInsetLeft`（正文确实有左右留白）/ `mermaidSvgCount`（E2E fixture 里塞一个 ```mermaid``` 块，断言懒加载渲染出 `[data-testid="plan-mermaid"] svg`，把 mermaid 出图作为回归项锁死），DOM 动作新增 `selectText{selector}` / `clickSelectionAdd` 供选中加入聊天的两路 E2E 驱动；auto-open 由 host E2E 走真实 `plan.create`→`plan.review` 路径，再由 `provider.test.ts` 覆盖重复 review / 缺 path / 降级打开等边界。这套只在 E2E 生效，不影响生产渲染。
11. **只读但可选中 → 加入聊天**：正文/清单默认 `user-select:text`（预览容器显式加，兜住 webview 默认）。两条入口最终都汇聚到同一个 `addSelectionToChat` intent：①浮动按钮 [`PlanSelectionActionButton`](../../../gui/src/plan/PlanSelectionActionButton.tsx) 监听 `selectionchange`/`mouseup`，选区非空且落在 `plan-content` 内时按选区 `getBoundingClientRect()` 定位浮现，滚动/失焦/收起即隐藏；②右键命令 `tomcat.plan.addSelectionToChat`（`menus.webview/context`，`when: webviewId == 'tomcat.planPreview'`）→ `provider.requestCaptureSelection()` 向 active 面板发 `captureSelectionForChat` 事件 → webview 读 `window.getSelection()` 回发同一 intent。host 侧 `handleIntent` 调注入的 `deps.addSelectionToChat(path,text,lineRange?)`：`focusWebviewSurface()` 取 sessionId → [`buildSelectionReferenceFromParts`](../../../src/ui/webview/contextReferences.ts)（`buildSelectionReference` 现为其薄封装）→ `postInsertReference` 渲染 chat 里的 `selection` chip。选中能力属共享正文组件，A/B 都可用。

    **行号：源码行映射（替代旧的原文 substring 尽力搜）**。旧做法把渲染后的选区文本回 `state.raw` 里 substring 匹配——正文经 marked 渲染后 `**bold**`/`` `code` `` 等内联标记已消失，匹配常失败 → chip 只带文件名、无行号。新做法照抄 VS Code 自带 markdown 预览的 `data-line`：[`planDocument.parsePlanDocument`](../../../src/ui/planPreview/planDocument.ts) 产出 `bodyLineMap`（`bodyMarkdown` 每一行 → 原文件 1-based 行号，`mapBodyLinesToRaw` 单调前向扫描，天然吸收 frontmatter 偏移与 `## Todos Board` 被剪掉后的非线性），随快照下发；[`MarkdownBody`](../../../gui/src/components/MarkdownBody.tsx) 用 `marked` 实例 + `use({ renderer })`（复用 `Renderer.prototype` 默认渲染，仅在块级首标签补 `data-source-line="<绝对行>"`，DOMPurify `ADD_ATTR` 放行）；[`PlanPreviewApp.readSelectionSourceLines`](../../../gui/src/plan/PlanPreviewApp.tsx) 读选区 `Range` 两端 `closest('[data-source-line]')` 得精确行号，落在无映射块（如 todo 清单）时返回空 → chip 退回只带文件名。这样 `**bold**` 段照样拿到稳定的 `文件名:行号`。

    **去重修复（P0「add to chat 总是失败」根因）**：chat composer 用 [`referenceIdentity`](../../../gui/src/contextReferences.ts) 去重，旧键 `kind::path::lineStart::lineEnd` 对**无行号** selection 全塌成 `selection::<path>::::`——同一 `.plan.md` 的多段无行号选区互相去重，除首个外全被静默丢弃（叠加 composer 草稿持久化后，表现为「点了没反应、一直失败」）。修复：无行号 selection 的身份追加所选文字的 FNV-1a hash，不同片段不再碰撞（完全相同的再次插入仍去重）；配合上面的源码行映射，绝大多数正文选区本就带唯一行号。
