# Tomcat VSCode 扩展 · Phase 2 · 05 Webview UI 架构与实现细节

> 总览见 [`../tomcat-vscode-extension-phase2.md`](../tomcat-vscode-extension-phase2.md)；Stage B 的落地选型与分层基线见 [`03-stage-b-webview.md`](03-stage-b-webview.md)；协议/运行时字段表见 [`04-protocol-runtime.md`](04-protocol-runtime.md)。
> transcript 稳定 id / reload 切回错乱的专项 companion 见 [`../webview-transcript-stable-id-upsert.md`](../webview-transcript-stable-id-upsert.md)；本文仍以“当前实现是怎么跑的”为主。
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
   └─ Composer
```

核心文件与职责：

| 文件 | 职责 | 关键点 |
|------|------|--------|
| [`gui/src/main.tsx`](../../../gui/src/main.tsx) | 挂载 React root，拿 `acquireVsCodeApi()` | 无宿主时回退到 no-op `vscodeApi`，方便 `vite` 独立调试。 |
| [`gui/src/App.tsx`](../../../gui/src/App.tsx) | 接 `state` 帧、发 intent、组装整个页面 | 统一处理 `ready` / `prompt` / `setModel` / `setPlanMode` / `openFile` / `openDiff` 等 transcript 相关意图，并内置 DOM snapshot 埋点。 |
| [`gui/src/components/SessionBar.tsx`](../../../gui/src/components/SessionBar.tsx) | 顶部会话选择栏 | 下拉显示 `sessionId + isCurrent/owner/busy` 元信息；右侧 `New / Refresh / Close`。 |
| [`gui/src/components/TranscriptView.tsx`](../../../gui/src/components/TranscriptView.tsx) | timeline 分发器 + assistant-response 二次分层 | 先按 `message / thinking / tool / approval / plan` 5 种一等项分发，再把 assistant 回复内部拆成 `action` 恒显行和 `context` 折叠盒；单个无 thinking 的 context 工具会直接扁平渲染。 |
| [`gui/src/components/ThinkingBlock.tsx`](../../../gui/src/components/ThinkingBlock.tsx) | thinking 折叠卡 | 默认折叠；流式时标题显示 `Thinking...` 脉冲动画。 |
| [`gui/src/components/ThinkingGroup.tsx`](../../../gui/src/components/ThinkingGroup.tsx) | “思考/上下文”折叠盒 | 只容纳 thinking + context/other 工具，默认收起；只有确实还挂着工具时才采信 `summaryTitle`，避免和独立 action 行重复。 |
| [`gui/src/components/ToolRow.tsx`](../../../gui/src/components/ToolRow.tsx) | 统一工具行 | 用 `toolCategory()` 把工具分成 `edit / command / answer / context / other`，再决定图标、徽章、扁平样式、展开规则与内容体。 |
| [`gui/src/components/DisclosureCard.tsx`](../../../gui/src/components/DisclosureCard.tsx) | 内容无关的折叠外壳 | 只管 header / preview / expanded body / 左侧状态条，不关心里面是 terminal 还是 diff。 |
| [`gui/src/components/TerminalOutput.tsx`](../../../gui/src/components/TerminalOutput.tsx) | 命令输出体 | 负责等宽输出渲染与 `tail(n)` 预览。 |
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
                │     └─ transcript 内联彩色 diff（preview tail5 / expand 50vh）
                │
                └─ openDiff intent
                      └─ provider.ts reconstruct before(ctx+del) / after(ctx+add)
                            └─ VsCodeIde.openReconstructedDiff()
                                  └─ vscode.diff(tomcat-diff://left, tomcat-diff://right)
```

补充约束：

- `diff` 是可选字段：大文件超阈值时核心只发 `added/removed`，不发 `diff`；
- 这时 transcript 仍显示 `+N/-M` 徽章，但 `DiffView` 退化为“文件过大，仅显示统计”，`View diff` 按钮隐藏；
- 因为宿主不再自己重算 diff，所以没有“文件已经写完，再回头读 before 导致左栏空白/`+0 -0`”的竞态。

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
4. **只有 user message 数量增加**时才重置跟随，工具/notice/thinking 不会把用户强行拉回底部；
5. `App.tsx` 在 `userHasScrolled=true` 时显示 `Jump to latest` 向下箭头图标按钮（保留 `scroll-to-bottom` test id，弱化视觉重量）。

为什么不用虚拟列表：

- 当前 webview transcript 规模小；
- 相比引入 `react-virtuoso`，这一版更容易和现有 DOM、测试、宿主 DOM snapshot 机制对齐；
- 但交互语义（上滑暂停、底部恢复）已经对齐 `cline/continue`。

### 5.2 Thinking：独立卡片，不并入 assistant 气泡

thinking 仍保持独立卡，而不是嵌入 assistant 气泡，原因有三：

1. 与 `state.ts` 的 timeline 一等项模型一致；
2. 历史与实时更容易复用同一条渲染路径；
3. 后续若引入更多 reasoning 元信息（耗时、脱敏、可复制等）更容易扩展。

[`gui/src/components/TranscriptView.tsx`](../../../gui/src/components/TranscriptView.tsx) 会在 `busy=true` 时找出最后一个 thinking 节点，把它标记为 `isStreaming`，供 [`ThinkingBlock.tsx`](../../../gui/src/components/ThinkingBlock.tsx) 做标题动画。

### 5.3 Transcript 工具行：标题恒显，结果体按类型折叠

> 专业：这次优化后，Tomcat 不再把所有工具都画成同一种 `ToolCallCard`。统一入口仍是 [`gui/src/components/ToolRow.tsx`](../../../gui/src/components/ToolRow.tsx)，但视觉与展开规则由 `toolCategory()` 驱动。
>
> 说人话：用户第一眼应该先看到“发生了什么”，再决定要不要看细节；不是先看到一堆长得一样的白字卡片。

视觉/交互模型：

```text
ToolRow
  ├─ edit/write
  │    └─ DisclosureCard
  │         ├─ header  = 动词 + FileChip + +N/-N 徽章 + View diff
  │         ├─ preview = DiffView.tail(5)
  │         └─ body    = DiffView
  ├─ command
  │    └─ DisclosureCard
  │         ├─ header  = Ran + 命令
  │         ├─ preview = TerminalOutput.tail(5)
  │         └─ body    = TerminalOutput
  ├─ answer  -> 直接挂 AnswerCard，始终展开
  └─ context -> 极简单行；单条 read/search 可直接扁平直出
```

各类规则：

1. **edit**
   - 只认 `edit / write / hashline_edit`，与 transcript 内部的 mutation 语义对齐。
   - diff 徽章与逐行 diff 都来自核心 `ToolDisplay::File.added/removed/diff`，经 [`src/serveClient/wire.d.ts`](../../../src/serveClient/wire.d.ts) → [`src/ui/webview/state.ts`](../../../src/ui/webview/state.ts) 直达 GUI。
   - 有 `diff` 时，`ToolRow` 会装配 `DisclosureCard(body=DiffView)`：折叠态只看尾部 5 行，展开态看完整结构化 diff（最大半屏、高度内滚动）。
   - `toolCallId + diff` 同时存在时，卡片右上角显示 `View diff` 图标按钮；点击发 `openDiff` intent。
   - 宿主 [`src/ui/webview/provider.ts`](../../../src/ui/webview/provider.ts) 会按 `ctx+del` 重建 before、按 `ctx+add` 重建 after，再通过 [`src/ide/VsCodeIde.ts`](../../../src/ide/VsCodeIde.ts) 复用既有 `tomcat-diff://` + `vscode.diff` 原生链路打开 diff 编辑器。
   - 大文件拿不到 `diff` 时，仍保留 `+N/-M` 徽章，但 `DiffView` 只显示 fallback 提示，`View diff` 自动隐藏。

2. **command**
   - `bash / shell / execute_command` 作为 standalone action 行常驻。
   - 用 `DisclosureCard(header=Ran + 命令, preview/body=TerminalOutput)` 统一折叠行为。
   - `complete && !isError` 默认折叠；`running / isError` 默认展开。
   - 折叠态不是“什么都不看见”，而是直接给尾部 5 行 preview；展开态上/下/左/右都可滚动，避免长命令输出把 transcript 拉爆。

3. **answer**
   - `ask_question` 的已回答态不再躲在折叠体里，而是直接渲染 [`AnswerCard.tsx`](../../../gui/src/components/AnswerCard.tsx)。
   - 这和待回答态 [`ApprovalCard.tsx`](../../../gui/src/components/ApprovalCard.tsx) 形成前后两段：前者让用户答题，后者保留 transcript 证据。

4. **context / other**
   - `read/search/web_*` 等保持小图标 + 描述色的一行摘要。
   - 连续多个会被 `ThinkingGroup` 收纳，避免 transcript 变成工具日志墙；单个无 thinking 的 context 工具直接扁平显示，保留 `FileChip` 与配色。
   - `read / read_file` 前导图标改成 `codicon-eye`，避免和 Markdown `FileChip` 的书本图标撞语义。

还有三个实现细节很关键：

- 结果体仍是懒挂载，展开前不进 DOM，减少长输出的布局压力。
- thinking-only 残组不会继续拿 `summaryTitle` 当折叠标题，避免命令标题在 action 行和折叠头各出现一次。
- `DisclosureCard` 是内容无关外壳，terminal / diff 细节全部留给 `TerminalOutput` / `DiffView`；这比在一个万能组件里堆 `mode` 开关更稳。

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
| [`gui/src/App.test.tsx`](../../../gui/src/App.test.tsx) | composer/DOM snapshot 埋点接线、跳底箭头按钮与 transcript snapshot 指标 |
| [`gui/src/components/DisclosureCard.test.tsx`](../../../gui/src/components/DisclosureCard.test.tsx) | 折叠/展开外壳、preview/body 切换 |
| [`gui/src/components/DiffView.test.tsx`](../../../gui/src/components/DiffView.test.tsx) | 行号列、加删底色、长 context 折叠、大文件 fallback |
| [`gui/src/components/ToolRow.test.tsx`](../../../gui/src/components/ToolRow.test.tsx) | edit diff 徽章 + View diff 按钮、command disclosure、answer/context 渲染语义、read 图标去重 |
| [`gui/src/components/TranscriptView.partition.test.ts`](../../../gui/src/components/TranscriptView.partition.test.ts) | assistant-response 冲刷算法（context/action 交错边界） |
| [`gui/src/components/TranscriptView.test.tsx`](../../../gui/src/components/TranscriptView.test.tsx) | 单 context 工具直出、action/context 分层 |
| [`gui/src/components/ThinkingGroup.test.tsx`](../../../gui/src/components/ThinkingGroup.test.tsx) | thinking-only 残组不复用 `summaryTitle` |
| [`src/ui/webview/tests/dual_channel.test.ts`](../../../src/ui/webview/tests/dual_channel.test.ts) | thinking 在 assistant 前、历史 `role:tool` → 工具卡、历史/实时去重 |
| [`src/ui/webview/tests/provider.test.ts`](../../../src/ui/webview/tests/provider.test.ts) | mutation 工具结束后从 `display.added/removed/diff` 注入 `diffStat/tool.diff`，以及 `openDiff -> ide.openReconstructedDiff` 路由 |
| [`src/ide/tests/diff_apply_edit.test.ts`](../../../src/ide/tests/diff_apply_edit.test.ts) | `openReconstructedDiff()` 复用原生虚拟文档 diff 链路 |
| [`src/test/suite/support/hostE2eScenario.ts`](../../../src/test/suite/support/hostE2eScenario.ts) | 真实宿主 webview streaming/diff/multi-session/ownership 通路 |

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
| thinking UI | [`gui/src/components/ThinkingBlock.tsx`](../../../gui/src/components/ThinkingBlock.tsx) |
| 思考/上下文折叠盒 | [`gui/src/components/ThinkingGroup.tsx`](../../../gui/src/components/ThinkingGroup.tsx) |
| 类型化工具行 / disclosure 外壳 / answer 卡 | [`gui/src/components/ToolRow.tsx`](../../../gui/src/components/ToolRow.tsx) / [`gui/src/components/DisclosureCard.tsx`](../../../gui/src/components/DisclosureCard.tsx) / [`gui/src/components/TerminalOutput.tsx`](../../../gui/src/components/TerminalOutput.tsx) / [`gui/src/components/AnswerCard.tsx`](../../../gui/src/components/AnswerCard.tsx) |
| composer 响应式 | [`gui/src/components/Composer.tsx`](../../../gui/src/components/Composer.tsx) / [`gui/src/styles.css`](../../../gui/src/styles.css) |
| 手工验收辅助 no-op 宿主 | [`gui/src/main.tsx`](../../../gui/src/main.tsx) |

---

## 9. 本次 UX 优化小结

本次体验优化没有重构 `TomcatMessenger`，但对 serve 的 file display 做了**小而必要**的协议扩展（新增 `ToolDisplay::File.diff`），其余主要仍在 **webview state 合并层 + React 表现层**完成：

1. 让滚动语义从“只会盲目追底”升级成“理解用户是否在看历史”；
2. 让 thinking 与 tool 在历史回放时和实时阶段保持同一语义；
3. 让 transcript 内部从“所有工具一把梭折叠”升级成“action 恒显、context 收纳、样式类型化、command/edit 统一 DisclosureCard 外壳”；
4. 让 edit diff 真相回到核心：同一份结构化 diff 同时喂给 transcript 内联彩色 diff 与原生 `View diff`，不再依赖扩展读盘时序或 git 工作区状态；
5. 把“协议改了但前端/fixture/安装包没追上”的工程风险显式制度化：`gen:wire`、serve fixture、版本 bump 必须一起做。

这意味着后续若继续迭代 webview UX，大多数样式/分组问题仍应优先在 [`state.ts`](../../../src/ui/webview/state.ts) 和 [`gui/src/**`](../../../gui/src) 内完成；但凡涉及“文件改动真相”（如 diff 行数、before/after 重建），必须优先回到核心事件层处理。
