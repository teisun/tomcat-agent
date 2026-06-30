# Tomcat VSCode 扩展 · Phase 2 · 05 Webview UI 架构与实现细节

> 总览见 [`../tomcat-vscode-extension-phase2.md`](../tomcat-vscode-extension-phase2.md)；Stage B 的落地选型与分层基线见 [`03-stage-b-webview.md`](03-stage-b-webview.md)；协议/运行时字段表见 [`04-protocol-runtime.md`](04-protocol-runtime.md)。
> transcript 稳定 id / reload 切回错乱的专项 companion 见 [`../webview-transcript-stable-id-upsert.md`](../webview-transcript-stable-id-upsert.md)；本文仍以“当前实现是怎么跑的”为主。
> 本文不是“想做什么”的方案文，而是“已经如何实现”的实现文：事实源以 [`gui/src/**`](../../../gui/src) 与 [`src/ui/webview/**`](../../../src/ui/webview) 为准。
> 外部参考仓库（仅作实现思路来源，不进本仓）：`/Users/yankeben/workspace/cline`、`/Users/yankeben/workspace/continue`。

---

## 1. 定位

> 专业：Phase 2 Stage B 已经把 Tomcat 的 webview 落成“宿主 provider + typed postMessage 协议 + React GUI + timeline 状态机”的四段式结构。本文补齐实现层细节，尤其覆盖本次 UX 优化后新增的自动滚动、thinking 排序、工具卡折叠与 composer 响应式布局。
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
   │  │  ├─ ToolCallCard
   │  │  ├─ ApprovalCard
   │  │  └─ PlanFileCard
   │  └─ Jump to latest button
   ├─ ActivePlanStrip
   ├─ AttachmentChips
   └─ Composer
```

核心文件与职责：

| 文件 | 职责 | 关键点 |
|------|------|--------|
| [`gui/src/main.tsx`](../../../gui/src/main.tsx) | 挂载 React root，拿 `acquireVsCodeApi()` | 无宿主时回退到 no-op `vscodeApi`，方便 `vite` 独立调试。 |
| [`gui/src/App.tsx`](../../../gui/src/App.tsx) | 接 `state` 帧、发 intent、组装整个页面 | 统一处理 `ready` / `prompt` / `setModel` / `setPlanMode` / `applyEdit` / `openDiff` 等意图。 |
| [`gui/src/components/SessionBar.tsx`](../../../gui/src/components/SessionBar.tsx) | 顶部会话选择栏 | 下拉显示 `sessionId + isCurrent/owner/busy` 元信息；右侧 `New / Refresh / Close`。 |
| [`gui/src/components/TranscriptView.tsx`](../../../gui/src/components/TranscriptView.tsx) | timeline 分发器 | 按 `message / thinking / tool / approval / plan` 5 种一等项渲染。 |
| [`gui/src/components/ThinkingBlock.tsx`](../../../gui/src/components/ThinkingBlock.tsx) | thinking 折叠卡 | 默认折叠；流式时标题显示 `Thinking...` 脉冲动画。 |
| [`gui/src/components/ToolCallCard.tsx`](../../../gui/src/components/ToolCallCard.tsx) | 工具结果卡 | complete 默认折叠，running/error 默认展开，展开区限高滚动。 |
| [`gui/src/components/ApprovalCard.tsx`](../../../gui/src/components/ApprovalCard.tsx) | AskQuestion 审批卡 | 直接把宿主 `control_request.ask_question` 渲染成按钮组。 |
| [`gui/src/components/ActivePlanStrip.tsx`](../../../gui/src/components/ActivePlanStrip.tsx) | 当前计划条 | 只在 `planning / pending / executing` 可见；`Build` 按钮单独放这里。 |
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
   - `handleWebviewMessage()`：把 GUI intent 路由到 `newSession / switchSession / prompt / setModel / setPlanMode / applyEdit / openDiff / pickAttachment` 等宿主动作。
   - `handleServeEvent()`：每来一条 `ServeEvent`，先更新 `stateStore`，再同时发 `event` 增量帧和 `state` 快照帧。
   - `postState()` / `postEvent()`：前者发送 `WebviewStateSnapshot`，后者发送 `HostEventFrameContent`。

2. [`src/ui/webview/protocol.ts`](../../../src/ui/webview/protocol.ts)
   - 定义 `HostToWebviewFrame` 与 `WebviewIntent`。
   - 提供 `isWebviewIntent()` 做宿主入站校验。
   - 维持 GUI 与宿主共享的 `WebviewTimelineItem`、`WebviewToolStatus` 等类型。

3. [`gui/src/App.tsx`](../../../gui/src/App.tsx)
   - 收 `channel: "state"`：整份覆盖到 React `state`。
   - 收 `channel: "event"`：当前只消费 `__test.capture_dom` 这类测试事件；正常渲染依赖宿主同步后的 `state` 快照。
   - 发 intent：统一走 `postIntent()`，保持消息 ID 生成和 frame 形态一致。

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
| `tool` | 实时 `tool_execution_*`；历史 `role:tool` | `ToolCallCard` | 历史工具结果不再降级成 notice。 |
| `approval` | `control_request.ask_question` | `ApprovalCard` | 宿主 resolve 后 `resolved=true`，UI 自动消失。 |
| `plan` | `plan.*` 事件 | `PlanFileCard` | transcript 内保留 plan 文件足迹；顶部另有 active strip。 |

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

---

## 5. 关键交互细节

### 5.1 自动滚动：只在“该跟随时”跟随

实现位于 [`gui/src/useAutoScroll.ts`](../../../gui/src/useAutoScroll.ts)。

机制：

1. `ResizeObserver` 观察滚动容器及其直接子节点；
2. 用户仍贴底时，内容变高就 `scrollTop = scrollHeight`；
3. `scroll` 监听根据 `|scrollHeight - scrollTop - clientHeight| < 2` 判断是否贴底；
4. **只有 user message 数量增加**时才重置跟随，工具/notice/thinking 不会把用户强行拉回底部；
5. `App.tsx` 在 `userHasScrolled=true` 时显示 `Jump to latest` 按钮。

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

### 5.3 工具卡：历史与实时统一的折叠模型

[`gui/src/components/ToolCallCard.tsx`](../../../gui/src/components/ToolCallCard.tsx) 的交互规则：

- `complete && !isError`：默认折叠；
- `running / streaming / isError`：默认展开；
- 如果用户手动点过，就尊重用户当前展开态，不再被状态自动覆盖；
- 展开区 `max-height: 280px; overflow:auto`，专门处理技能正文、长文本工具输出。

这使得三类场景统一成一套 UI：

```text
历史 role:tool           -> 可折叠工具卡
实时 tool_execution_end  -> 可折叠工具卡
短系统 notice            -> 仍是 MessageBubble(kind:notice)
```

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
- 本次新增的交互动效也留在 CSS：`tc-thinking-pulse`、`tc-tool-spin`。

---

## 7. 测试与验收

自动化测试：

| 文件 | 覆盖点 |
|------|--------|
| [`gui/src/useAutoScroll.test.tsx`](../../../gui/src/useAutoScroll.test.tsx) | 贴底跟随、上滑暂停、session 切换与 user message 重置 |
| [`gui/src/App.test.tsx`](../../../gui/src/App.test.tsx) | thinking 折叠、工具卡默认折叠/展开、composer 意图发送 |
| [`src/ui/webview/tests/dual_channel.test.ts`](../../../src/ui/webview/tests/dual_channel.test.ts) | thinking 在 assistant 前、历史 `role:tool` → 工具卡、历史/实时去重 |
| [`src/test/suite/support/hostE2eScenario.ts`](../../../src/test/suite/support/hostE2eScenario.ts) | 真实宿主 webview streaming/diff/multi-session/ownership 通路 |

实际 UI 验收（本次体验优化）：

1. 用 `vite dev` 单独跑 GUI；
2. 浏览器侧注入 mock `state` 帧；
3. 验证：
   - 贴底时新消息自动跟随；
   - 上滑后停止跟随并出现 `Jump to latest`；
   - thinking 展开后位于对应 assistant 回复之前；
   - 工具卡默认折叠/运行中展开/长文本限高滚动；
   - 窄宽度下 composer 不再换行错乱。

---

## 8. 实现 ↔ 文件对照表

| 关注点 | 主要文件 |
|--------|----------|
| 宿主生命周期 / webview html / postState / postEvent | [`src/ui/webview/provider.ts`](../../../src/ui/webview/provider.ts) |
| 类型 / frame / intent | [`src/ui/webview/protocol.ts`](../../../src/ui/webview/protocol.ts) / [`gui/src/types.ts`](../../../gui/src/types.ts) |
| timeline 合并 / thinking & tool 历史回放 | [`src/ui/webview/state.ts`](../../../src/ui/webview/state.ts) |
| 自动滚动与跳底按钮 | [`gui/src/useAutoScroll.ts`](../../../gui/src/useAutoScroll.ts) / [`gui/src/App.tsx`](../../../gui/src/App.tsx) |
| transcript 分发 | [`gui/src/components/TranscriptView.tsx`](../../../gui/src/components/TranscriptView.tsx) |
| thinking UI | [`gui/src/components/ThinkingBlock.tsx`](../../../gui/src/components/ThinkingBlock.tsx) |
| 工具卡折叠 | [`gui/src/components/ToolCallCard.tsx`](../../../gui/src/components/ToolCallCard.tsx) |
| composer 响应式 | [`gui/src/components/Composer.tsx`](../../../gui/src/components/Composer.tsx) / [`gui/src/styles.css`](../../../gui/src/styles.css) |
| 手工验收辅助 no-op 宿主 | [`gui/src/main.tsx`](../../../gui/src/main.tsx) |

---

## 9. 本次 UX 优化小结

本次体验优化没有改动 `TomcatMessenger` 或 serve 协议，而是在 **webview state 合并层 + React 表现层**完成：

1. 让滚动语义从“只会盲目追底”升级成“理解用户是否在看历史”；
2. 让 thinking 与 tool 在历史回放时和实时阶段保持同一语义；
3. 让工具结果与 composer 的 UI 结构更接近 VS Code Chat，而不是单纯把字段堆出来。

这意味着后续若继续迭代 webview UX，优先应该继续在 [`state.ts`](../../../src/ui/webview/state.ts) 和 [`gui/src/**`](../../../gui/src) 内完成，而不是回头修改 bridge/core 层。
