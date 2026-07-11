# Tomcat `@` 内联上下文搜索（At-Mention Context Search）

> 适用范围：`tomcat-vscode-ext` 在富输入框里新增 **`@` 触发的内联上下文引用搜索**——用户在 Composer 中键入 `@`，弹出工作区文件/文件夹的模糊搜索下拉，选中后在光标处插入一个**上下文引用 chip**，与文字按序混排发送。
>
> 一句话定位：**`@` 不是一套新的引用系统，而是给「上下文引用」再加一个输入入口。** 现有引用系统已有三条既有入口——编辑器选区命令、`+` 选择器、`Shift` 拖拽（见 [`context-references.md`](context-references.md)）；本方案补上第四条「输入框内 `@` 搜索」，产出的仍是同一个 `ReferenceNode` → 有序 `segments`。
>
> 单一事实源：
>
> 1. `@` 触发与下拉渲染以 [`../../gui/src/components/Composer.tsx`](../../gui/src/components/Composer.tsx)、`../../gui/src/components/mentionSuggestion.ts`（新增）、`../../gui/src/components/ContextSearchDropdown.tsx`（新增）为准；
> 2. webview↔host 的 `searchContext` intent / `contextSearchResult` event 以 [`../../src/ui/webview/protocol.ts`](../../src/ui/webview/protocol.ts) 为准；
> 3. host 侧搜索实现以 `../../src/ui/webview/contextSearch.ts`（新增）为准；
> 4. 引用（reference）数据形态仍以 [`../../../tomcat/src/api/serve/types.rs`](../../../tomcat/src/api/serve/types.rs) 的 `ServeContentSegment` 为准（**本方案不改它**）。
>
> 章节编号对齐 [`ARCHITECTURE_SPEC.md`](../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)：文首导读（A.1/A.2/B）→ §1 术语 → §2 竞品调研 → §3 落地选型与实施（§3.1 决策表 + §3.2 实施点）→ §4 协议 → §5 One-Glance → §6 配置 → §7 错误模型 → §8 测试矩阵 → §9 风险 → §10 历史决策。

---

## 文首导读：方案导图集

阅读顺序（说人话）：

1. **A.1 抽象总图**：先看「谁触发、谁搜索、事实源在哪、关键分叉、最后落到哪」。
2. **A.2 具体总图**：再看落到真实文件/intent/event/服务的运行时链路。
3. **B 状态机**：最后看下拉菜单从「弹出→查询→有/无结果→插入/取消」的生命周期。

核心心智模型一句话：**`@` 只负责「找到一个文件并插入一个引用 chip」；chip 之后怎么排序、怎么落盘、怎么发给 LLM，全部是既有「上下文引用系统」的既成能力，本方案一行都不碰。**

### A.1 抽象 ASCII 总图（职责 / 事实源 / 分叉）

> 专业：`@` 能力被切成「触发—搜索—装配—插入」四段。触发点是输入框里的 `@` token；处理层是带防抖 + 陈旧丢弃的异步搜索；事实源是 host 侧的工作区文件候选集（含由文件派生的目录）；终局是把选中项装配成一个既有的 reference chip 汇入有序 `segments`。
> 说人话：这张图想先让你明白——搜索结果只是「帮你挑一个文件」，挑完之后走的还是老路（chip → segments → 落盘/回放/发送），所以真正的新增复杂度只集中在「怎么触发 + 怎么搜 + 怎么防抖丢弃」。

```text
输入触发                处理层(webview)            事实源(host)              终局
────────               ───────────────           ────────────             ────
用户在输入框打 @  ─────▶  @ 触发器 + 防抖    ──────▶ 工作区文件候选缓存   ──────▶ 下拉候选列表
  │ (query 变化)          │ searchContext          │ (findFiles + 默认排除)   │
  │                       │  {requestId, query}    │ 派生目录集(供 @folder)   │ (键盘/点击选中)
  ▼                       ▼                        │ fuzzy 排序 + 打开文件加权 ▼
                     陈旧响应丢弃  ◀──────  contextSearchResult  ◀────────  插入 reference chip
                     (latestRequestId)     {requestId, matches, truncated}   │
                                                                             ▼
                                          复用「上下文引用系统」：有序 segments
                                          →（落盘 / 历史回放 / LLM flatten 均不变）

关键分叉：
  无工作区        → 不弹菜单，一次性提示「打开文件夹后可用 @」
  无匹配          → 空态「未找到匹配文件」
  命中超 CAP      → truncated=true，底部提示「仅显示前 N 条，输入更精确关键词」
  重复引用        → referenceIdentity 命中 → 不重复插入
  requestId 过期  → 直接丢弃（避免慢查询覆盖新查询）
```

### A.2 具体 ASCII 总图（真实对象 / 运行时链路）

> 专业：把 A.1 的四段落到真实模块——TipTap `@tiptap/suggestion` 只负责「检测 `@` / 跟踪 query / 提供 command / 生命周期 / 键盘转发」；**数据不经 TipTap 的 `items()` 回环**，而是由 `App.tsx` 的 React 状态承载（发 `searchContext`、按 `requestId` 去重收 `contextSearchResult`）；下拉锚定在 composer 上方（复用既有 `.tc-session-dropdown` 定位），选中时调 suggestion 注入的 `command` 删掉 `@query` 并插入既有 `ReferenceNode`；host 的 `provider.ts` 把 intent 交给新增 `ContextSearchService`。
> 说人话：两个要点。其一，最下面那条「prompt intent（复用，无改动）」——`@` 插入的 chip 和「选区/拖拽/+」插入的 chip 是同一个东西，序列化/发送/落盘一个字都不用改。其二，**别让 TipTap 的 `items()` 去装搜索结果**——它是一次性喂渲染的，host 的结果晚一步到就更新不上（竞态）；所以数据统一放在 App 的 React 状态里，TipTap 只当「触发器 + 插入器」。

```text
gui/src/components/Composer.tsx  (TipTap 富输入框, 已存在)
  ├─ mentionSuggestion.ts (新增: @tiptap/suggestion, char:'@', allowSpaces:false)
  │     职责: 检测@ / 跟踪 query / 提供 command / 生命周期(onStart/onUpdate/onExit) / onKeyDown 转发
  │     items() 恒返回 []   ← 数据不走 TipTap，杜绝异步回环竞态
  │       onStart(props)   ─▶ isMentionOpenRef=true; setMention({command:props.command}); onQueryChange("")
  │       onUpdate(props)  ─▶ onQueryChange(props.query)
  │       onKeyDown(props) ─▶ 委托 ContextSearchDropdown.onKeyDown(↑↓/Enter/Tab/Esc → 返回 true 消费)
  │       onExit()         ─▶ isMentionOpenRef=false; 关闭下拉
  ▼
gui/src/App.tsx   (搜索数据与渲染的唯一真相)
  ├─ onQueryChange → 150ms 防抖 → postIntent("searchContext",{requestId:++seq, query, kind:"file"})
  ├─ event contextSearchResult{requestId,matches,truncated}: requestId!==latest → 丢弃; 否则 setState
  └─ 把 {matches, loading, truncated} 作为 props 喂给 ▼
gui/src/components/ContextSearchDropdown.tsx  (新增: 锚定 composer 上方, 复用 .tc-session-dropdown 定位)
  │  键盘导航 / loading / empty / truncated 提示; 行内复用 ReferenceChip 图标+label+description
  └─ 选中项 ─▶ mention.command(match)     // TipTap suggestion 注入的 command
                 ▼
              suggestion.command({editor, range, props:match}):
                 if editorHasReference(match.reference) → 仅 deleteRange(range)  // 去重, 幂等
                 else editor.chain().focus().deleteRange(range)                  // 删掉 "@query"
                        .insertContent([ReferenceNode(match.reference), " "])    // 插入既有原子 chip
                 ▼ (onUpdate)
              serializeComposerDocument() → segments  ── 既有, 无改动 ──▶ prompt intent(params.segments)
                                                                     └──▶ serve（协议/落盘/flatten 全不变）
──────────────────────────────────────────────────────────────────────────────────────────
src/ui/webview/provider.ts   handleIntent: case "searchContext"
  └─ 取消上一个 CancellationTokenSource → ContextSearchService.search(query, kind, limit, token)
        ▼
src/ui/webview/contextSearch.ts  (新增)
  ├─ ensureCache(): workspace.findFiles("**/*", /*exclude=*/undefined→默认排除, MAX_FILES, token)
  │                 + FileSystemWatcher(create/delete) 增量失效
  ├─ deriveDirectories(files): 从文件路径抽唯一父目录集（供 @folder）
  ├─ fuzzyRank(candidates, query): 子序列打分 + 边界/basename 加权 + 打开编辑器加权
  ├─ buildFileReference(uri,{isDirectory})   ← 复用 contextReferences.ts（零重复）
  └─ provider.postEvent(contextSearchResult { requestId, query, matches, truncated })
```

### B. 状态机（下拉菜单生命周期）

> 专业：下拉是一个「Closed → Querying →（Results | Empty）→ Closed」的有限状态机，唯一并发风险是慢查询乱序返回，用 `requestId==latest` 门控。Enter/↑/↓/Esc 在 open 态被 suggestion 插件消费，**不**触达 Composer 的「Enter 发送」keydown（见 §3.2.3 门控）。
> 说人话：几态几迁一目了然——打 `@` 就进「查询中」，结果回来分「有/没有」，选中或按 Esc 就关。真正要小心的就一件事：结果乱序回来时别让旧结果盖掉新结果。

```text
                       输入@(边界合法)
      ┌────────┐  ────────────────────▶  ┌──────────────┐  result(latest,非空)  ┌───────────┐
      │ Closed │                          │  Querying    │ ─────────────────────▶ │  Results  │
      │(菜单关)│  ◀───── Esc/空格/失焦 ──── │(loading, 已发│ ◀── query 变(防抖重发)  │(可键盘导航)│
      └───┬────┘         /删到@前          │ searchContext)│                        └────┬──────┘
          │                                └──────┬───────┘  result(latest,空)           │ Enter/Tab/点击
          │ 输入@ 但无工作区                       │        ┌──────────┐                  ▼
          ▼                                        └───────▶│  Empty   │           deleteRange(@query)
   一次性提示"打开文件夹后可用@"                             │(未找到)  │           + 插入 chip + 去重
   (不进 Querying)                                          └────┬─────┘                  │
                                                                 │ Esc/空格               ▼
                                                                 └──────────────────▶  Closed
       result(requestId != latest) 在任意 open 态 → 丢弃(状态不变)
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
|----------|------|----------|--------|--------|
| Closed | 输入 `@` 且前一字符为行首/空白、有工作区 | Querying | 发 `searchContext(req0)`，显示 loading | 打 `@` 就弹菜单并发第一枪查询 |
| Closed | 输入 `@` 但无工作区 | Closed | 一次性 `showWarningMessage` 提示 | 没打开文件夹时 `@` 不弹菜单 |
| Querying | query 变化（继续打字/退格） | Querying | 防抖 150ms 后发 `searchContext(req+1)` | 每改一次词就重查，旧查询作废 |
| Querying | `contextSearchResult(req==latest, 非空)` | Results | 渲染候选，高亮第 0 项 | 结果回来就显示列表 |
| Querying | `contextSearchResult(req==latest, 空)` | Empty | 显示「未找到匹配文件」 | 没匹配显示空态 |
| Querying/Results/Empty | `contextSearchResult(req!=latest)` | 不变 | 丢弃该响应 | 过期响应直接扔掉 |
| Results | ↑ / ↓ | Results | 移动高亮项 | 键盘选择 |
| Results | Enter / Tab / 点击 | Closed | `deleteRange(@query)` + 插入 `ReferenceNode` + 去重 | 选中就把 `@query` 换成 chip |
| 任意 open | Esc / 空格 / 失焦 / 删除到 `@` 之前 | Closed | 关闭菜单，不改文本 | 取消，不产出引用 |

---

## §1 术语统一

> 每条术语给「语义 / 数据载体 / 行为约束」。与既有 [`context-references.md`](context-references.md) 术语保持一致，不引入同义词。

| 术语 | 语义 | 数据载体 | 行为约束 | 说人话 |
|------|------|----------|----------|--------|
| `@` 触发 (at-trigger) | 在输入框光标前出现 `@` 且满足边界条件时开启搜索下拉 | `@tiptap/suggestion` 的 `char:'@'` + `allowSpaces:false` | `@` 前须为行首或空白；`@` 与光标间不得含空白；IME 组字期间不误触发 | 打个 `@` 就弹菜单，但得在「词的开头」打 |
| 建议下拉 (ContextSearchDropdown) | 展示候选、可键盘导航的受控浮层 | React 组件，锚定 composer 上方（复用 `.tc-session-dropdown` 静态定位） | open 态独占 ↑/↓/Enter/Tab/Esc；数据由 App 状态驱动（不经 TipTap `items()`）；样式走 bundle CSS | 那个弹出来的候选框 |
| 上下文引用 chip (reference chip) | 输入框里代表一个文件/目录/选区的原子药丸 | TipTap `ReferenceNode`（`atom:true`）+ `ServeContextReference` | `@` 选中后**复用**此节点；不新建节点类型 | 选中文件后插进去的那个小方块 |
| 有序 segments | 文本与引用按输入顺序交织的数组 | `ServeContentSegment[]`（`text` \| `reference`） | 顺序即语义；`@` 不改变其结构 | 一句话里「字 + 引用」的先后顺序 |
| `searchContext` intent | webview→host 的搜索请求 | `WebviewIntent`（`protocol.ts`） | 携带单调递增 `requestId`；host 收到后取消上一个搜索 | 前端喊 host「帮我搜这个词」 |
| `contextSearchResult` event | host→webview 的搜索结果 | `HostEventFrameContent`（`protocol.ts`） | 回带同一 `requestId`；前端按 `latestRequestId` 丢弃过期 | host 把搜到的文件送回来 |
| 候选缓存 (candidate cache) | host 侧一次性拉取的工作区文件+派生目录集 | `contextSearch.ts` 内 `vscode.Uri[]` + `Set<string>` | 由 `FileSystemWatcher` 增量失效；受 `MAX_FILES` 上限保护 | host 先把工作区文件列一遍存着，别每次敲键都扫盘 |
| 陈旧响应丢弃 (stale-drop) | 慢查询晚到时不覆盖新查询结果 | webview `latestRequestId` | `requestId !== latest` 即丢弃 | 打字快时，旧结果回来晚了就作废 |
| 防抖 (debounce) | 连续输入时合并搜索请求 | `DEBOUNCE_MS`（默认 150ms） | 停止输入 150ms 后才发请求 | 别每敲一个字母就查一次 |
| 截断标注 (truncated) | 命中超过上限的显式信号 | `contextSearchResult.truncated:boolean` | 为 `true` 时下拉底部提示 | 结果太多，只给你看前 N 条 |

「行首/空白」钉死指代：指光标处 `@` 的**前一个字符**是段首（`@` 位于 ProseMirror 文本块起始）或为 ASCII 空白（空格 / Tab / 换行）。

---

## §2 竞品 / 选型对比（调研）

> 本节钉住「读过 5 家竞品后的调研材料」，为 §3 决策提供证据链；已定稿的取舍矩阵在 §3.1。

### 2.1 形态分类

`@` 搜索在业界有两个正交维度：**输入层形态**（富编辑器原子节点 vs textarea+高亮 vs Monaco 补全）与 **引用内容归宿**（内联保序 vs 前置/后置展开）。

```text
输入层形态                              引用内容归宿
──────────                             ──────────
富编辑器原子节点 chip                    内联保序（顺序即语义）
  Continue(TipTap), Tomcat(TipTap)        Tomcat（本方案，segments）
                                          VS Code（inline 引用带 range）
textarea + 透明高亮层
  Cline(<mark> 叠加)                     内容前置/后置到消息
                                          Continue（contextItems 前置）
Monaco 编辑器 + 原生补全                    Cline（<file_content> 追加末尾）
  VS Code Chat（扩展拿不到）               OpenCode（服务端 Read 展开为 synthetic text）
                                          Codex（@=路径文本，模型自行 read）
终端 TUI 输入 + 异步搜索
  Codex(nucleo), OpenCode(fff/ripgrep)
```

### 2.2 竞品横向对比

| 竞品 | 输入层 | `@` 触发/搜索 | 搜索后端 | 引用归宿 | 我们借鉴的点 | 说人话 |
|------|--------|---------------|----------|----------|---------------|--------|
| **Continue** | TipTap + 自研 Mention 原子节点 | `@tiptap/suggestion` + tippy 浮层；submenu 预载 + 客户端 MiniSearch | `FileContextProvider.loadSubmenuItems` walkDir 预载（`core/context/providers/FileContextProvider.ts`） | contextItems **前置**到 user message，文本内留 `@label` | **编辑器与 Suggestion 范式**、打开文件加权排序、插入去重 | 和我们输入框同款(TipTap)，@ 用官方 suggestion 插件 |
| **Cline** | `textarea` + 透明 `<mark>` 高亮层 | `shouldShowContextMenu()` 正则判定 + `ContextMenu` 组件（`webview-ui/.../context-mentions.ts`） | gRPC `searchFiles` → ripgrep `--files` + `fzf`（`src/services/search/file-search.ts`） | `parseMentions` 内联占位 + `<file_content>` **追加末尾** | **陈旧响应丢弃(`latestSearchTokenRef`)**、fzf 排序、`pendingInsertions` 队列 | textarea 高亮做法，搜索走后端 ripgrep |
| **Codex** | 终端 TUI `ChatComposer` | 每键 `sync_popups()` → `AppEvent::StartFileSearch`（`tui/src/bottom_pane/chat_composer.rs`） | `codex-file-search` = `nucleo` + `ignore`（`file-search/src/lib.rs`），非 ripgrep | `@`=**路径文本**（不预读内容），有序 `Vec<UserInput>` | **session 式异步搜索 + `pending_query` 陈旧防护**、有序 input 模型 | @ 只塞路径，模型自己去读 |
| **OpenCode** | TS TUI（OpenTUI）/ Web(contenteditable) | `mentionTriggerIndex` + `fs.find`（`packages/tui/.../autocomplete.tsx`） | `fff`/ripgrep（`core/src/filesystem/search.ts`） | 服务端 `resolveUserPart` 用 **Read 工具展开**成 synthetic text parts | **行号三写**（标签`#s-e` + URL query + source 偏移）、服务端统一展开 | @ 选完，服务器再把文件读出来拼进去 |
| **VS Code Chat** | Monaco `CodeEditorWidget`（`vscode-chat-input` scheme） | 注册 `CompletionItemProvider`（`@`/`#`/`/`），`chatInputCompletions.ts` | `searchFilesAndFolders`（内部 search service） | inline 引用带 `range`（`ChatRequestDynamicVariablePart`）+ 药丸区 attachment 双轨 | **双坐标(offset/range)保序思想**；确认**扩展 API 拿不到**原生补全 | 官方 Chat 用 Monaco+原生补全，插件够不着 |

### 2.3 为什么选我们这条路（要点，详见 §3.1）

1. **输入层不用重造**：Tomcat 输入框已是 TipTap（`Composer.tsx`），且已有原子 `ReferenceNode`——与 Continue 同构，`@` 直接加官方 `suggestion` 插件即可，成本最低。
2. **VS Code 原生补全走不通**：调研确认扩展无法向 `vscode-chat-input` scheme 注册补全（也拿不到 `IContentWidget` 浮动药丸），必须在自有 webview 里自研，因此不考虑「接入原生」。
3. **引用归宿已是最优解**：我们已有「内联保序 segments」（比 Continue 前置、Cline 末尾追加、OpenCode 服务端展开都更忠实顺序），`@` 复用它即可，**不引入第二套展开逻辑**。
4. **搜索后端不引二进制**：Cline/OpenCode spawn ripgrep、Codex 用 nucleo crate；我们在 host 有 `vscode.workspace.findFiles`（原生、跨平台、尊重排除配置），首版够用且零二进制分发。
5. **并发正确性有成熟先例**：Cline `latestSearchTokenRef` / Codex `pending_query` 都证明「requestId + 陈旧丢弃」是异步搜索的正解，直接采纳。

---

## §3 落地选型与实施（已定稿）

### §3.1 落地选型决策表（七列「维度上的取舍」）

> 每行一个可辩驳分叉；`决策` 列给裁决句，`取自` 至少 1 条本仓 + 1 条外部证据，`说人话` 讲背景 + 解法。

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| **R1 触发机制** | `@` 弹菜单靠什么？ | 采用 `@tiptap/suggestion` 插件 + 自研受控 React 下拉；拒绝自研 keydown/正则光标解析、拒绝接入 VS Code 原生补全 | 本仓 `gui/src/components/Composer.tsx`（`ReferenceNode` atom + `useEditor`）；外部 continue `gui/src/components/mainInput/TipTapEditor/extensions/Mention.ts`（Suggestion）、vscode `src/vs/workbench/contrib/chat/browser/widget/input/editor/chatInputCompletions.ts`（扩展够不着） | 设计：输入框已是 TipTap，加一个 Suggestion 插件负责「触发 + 生命周期 + 键盘转发 + 用 `command` 把 `@query` 替换为**既有** `ReferenceNode`」，**数据不经其 `items()`**（见 R3，避免异步回环竞态）；理由：光标/原子节点/IME/撤销由 ProseMirror 内建处理，官方机制稳、增量小 | 自研 keydown 解析（Cline `webview-ui/src/utils/context-mentions.ts:shouldShowContextMenu` 的 textarea 方案）——拒因：我们不是 textarea，手算 `@token` 边界会与 ProseMirror 选区/IME 打架；VS Code 原生补全——拒因：扩展无法向 `vscode-chat-input` 注册 `CompletionItemProvider`（vscode 调研确认） | 输入框本来就是 TipTap，加官方 @ 插件即可；别自己算光标，也别指望官方 Chat 的 @ |
| **R2 搜索后端** | 文件从哪搜、怎么排？ | 采用 host `vscode.workspace.findFiles` + 缓存 + 进程内轻量 fuzzy；拒绝 spawn ripgrep 二进制、拒绝全量索引/embedding | 本仓 `src/ui/webview/provider.ts`（host 已持 vscode API 与 `showOpenDialog`）；外部 cline `apps/vscode/src/services/search/file-search.ts`（ripgrep+fzf）、codex `codex-rs/file-search/src/lib.rs`（nucleo+ignore）、continue `core/context/providers/FileContextProvider.ts`（walkDir 预载+MiniSearch） | 设计：host 懒建工作区文件缓存（`findFiles("**/*")` + 默认排除）+ 由文件派生目录，按 query 在内存 fuzzy 排序并对打开文件加权；理由：`findFiles` 原生尊重 `files.exclude`/`search.exclude`、跨平台零二进制，webview 无 fs 权限只能靠 host | ripgrep spawn（Cline/OpenCode）——拒因：需分发/管理多平台二进制，超出首版必要；全量索引/embedding（Continue `@codebase`）——拒因：启动与内存成本高，`@file` 首版用不到语义检索 | 用 VS Code 自带的找文件 API（尊重你的忽略配置），先缓存再内存里排；不塞 ripgrep、不建索引 |
| **R3 通道与并发** | 请求/响应怎么走、慢查询乱序怎么办？ | 新增 `searchContext` intent + `contextSearchResult` event（带 `requestId`），前端 `latestRequestId` 丢弃过期 + 150ms 防抖；拒绝同步阻塞 RPC、拒绝无防抖每键直查 | 本仓 `src/ui/webview/protocol.ts`（`WebviewIntent`/`HostEventFrameContent` 双通道、`insertReference` 事件）；外部 cline `apps/vscode/webview-ui/src/components/chat/ChatTextArea.tsx:latestSearchTokenRef`、codex `codex-rs/tui/src/file_search.rs:pending_query` | 设计：沿用既有单向双通道，加 `requestId` 关联；结果落 `App` 的 React 状态、经 props 下发下拉（**不塞进 TipTap `items()` 的一次性返回值**），使晚到结果能触发重渲染；理由：与现有 `pickContext`/`resolveDrop` 通道同构，天然支持取消与乱序丢弃 | 同步 `messenger.request`——拒因：搜索需可取消、可乱序丢弃，同步语义不匹配；把结果放进 TipTap `items()` 返回值——拒因：`items()` 是一次性喂渲染，事件晚到不会重渲染，产生竞态；无防抖直查（Continue 靠内存 MiniSearch 可行）——拒因：我们查 host 磁盘，需防抖降载 | 沿用老的「前端发消息、后端回事件」；每次查询编号，慢的旧结果回来就扔；打字太快先等 150ms；结果放 React 状态里，别塞进 TipTap 那个一次性入口 |
| **R4 数据模型** | `@` 产出什么、要不要新结构？ | 复用现有 `ServeContentSegment(reference)` + TipTap `ReferenceNode` + `referenceIdentity` 去重 + `buildFileReference` 构造，`@` 选中经 `suggestion.command`（`deleteRange(@query)` + `insertContent(ReferenceNode)`）插入与前三入口**同形**的 chip，**零新增 wire/后端改动**；拒绝 Cline 式 `@/path` 文本 token、拒绝 VS Code inline/attachment 双轨 | 本仓 `tomcat/src/api/serve/types.rs`（`ServeContentSegment`/`ServeContextReference`）+ `gui/src/components/Composer.tsx:serializeComposerDocument`；外部 cline `apps/vscode/src/core/mentions/index.ts:parseMentions`（文本 token+末尾展开）、vscode `src/vs/workbench/contrib/chat/common/requestParser/chatParserTypes.ts`（inline vs attachment 双轨） | 设计：`@` 是引用系统的第四个输入入口（前三：选区命令、`+`、拖拽），产出同一 file 引用 chip；理由：落盘/回放/LLM flatten 已实现且经测试，复用即正确，协议零扩散 | Cline 文本 `@/path` + 正则 + `<file_content>` 末尾展开——拒因：丢结构、需额外展开阶段、破坏保序；VS Code inline/attachment 双轨 + range——拒因：我们已用有序 segments 统一表达，双轨是多余复杂度 | @ 选完插的就是「选区/拖拽」同款 chip；后面发送落盘那套完全不动，等于白捡 |
| **R5 引用类型范围** | 首版 `@` 支持哪些类型？ | 首版 `@file` + `@folder`（`kind:"file"` 引用）；`@symbol` 列 P2；拒绝首版 `@problems`/`@terminal`/`@git` | 本仓 `src/ui/webview/contextReferences.ts:buildFileReference`；外部 cline `webview-ui/src/utils/context-mentions.ts:ContextMenuOptionType`（file/folder/problems/terminal/url/git）、continue providers（file/code/docs/codebase） | 设计：`@` 产既有 file 引用，目录以尾部 `/` 区分；理由：与现有 reference kind（`selection`\|`file`）对齐，最小闭环即交付可用能力 | 首版做 `@problems`/`@terminal`/`@git`（Cline）——拒因：它们不产「文件引用 chip」，语义/落盘形态不同，属另一类上下文，另立项；`@symbol`——非拒绝，排期 P2（依赖 `DocumentSymbolProvider`） | 先把「@ 找文件/文件夹」做扎实；诊断/终端/git 那些是另一码事，以后再说 |
| **R6 行号内联语法** | 要不要支持 `@path#L10-20`？ | 首版不解析 `#`/`:` 行号，行号引用由既有「选区 Add-to-Chat」入口覆盖；拒绝首版实现行号解析 | 本仓 `src/ui/webview/contextReferences.ts:buildSelectionReference`（选区已带 1-based 行号）；外部 opencode `packages/tui/src/component/prompt/autocomplete.tsx:extractLineRange`（`#start-end`）、vscode `chatInputCompletions.ts`（`#file:name:10-20`） | 设计：`@` 只产整文件/目录引用；理由：行号场景已有选区入口，避免解析 `#` 的复杂度与歧义 | OpenCode/VSCode 的 `#`/`:` 行号内联——拒因：首版收益低、解析成本与歧义高（`#` 与文件名/锚点冲突，见 OpenCode `#` 需 URL 编码注记），P2 再评估 | 想带行号？先选中代码用「加入聊天」；@ 暂时只给整文件 |
| **R7 浮层定位/CSP** | 下拉怎么定位、会不会踩 CSP？ | 下拉**锚定 composer 上方**（复用既有 `.tc-session-dropdown` 静态定位），非光标浮动定位；拒绝 tippy.js/floating-ui（注入运行时 `<style>` 标签），拒绝光标 `clientRect` 浮动定位 | 本仓 `src/ui/webview/provider.ts:getHtml`（严格 CSP：`style-src ${cspSource}` 无 `unsafe-inline`）+ 既有 `.tc-session-dropdown`/`.tc-model-dropdown` 均 composer 锚定；外部 continue `gui/src/components/mainInput/TipTapEditor/utils/getSuggestion.ts`（tippy.js） | 设计：下拉像既有模型/模式下拉一样锚定 composer 上方、样式全进 bundle CSS，无需任何动态行内样式；理由：窄侧栏里光标浮动易溢出裁剪，composer 锚定更稳且与既有 UI 一致；CSP 关键澄清——`style-src` 拦的是 `<style>` 标签与 HTML `style` 属性，**不拦** React/CSSOM 的 `element.style` 赋值（`context-references.md §11.2` 同源结论），故即便未来要动态定位也安全，tippy 之所以被拦是因它注入 `<style>` 标签 | tippy.js（Continue）/floating-ui——拒因：注入运行时 `<style>` 标签与严格 CSP 冲突，且额外依赖增大 bundle；光标 `clientRect` 浮动定位——拒因：窄侧栏易溢出裁剪、还要处理翻转，收益不抵复杂度 | 下拉就照现有「模型/模式下拉」的样子贴在输入框上方，样式全写死在 CSS 里；CSP 拦的是 `<style>` 标签不是 React 设的样式，所以别用 tippy 那种会写 `<style>` 的库就行 |

### §3.2 实施点（已闭环）

> 与 §3.1 一对多映射：R1/R3/R7 主要落 P2（GUI），R2 主要落 P1（host），R4/R5/R6 是「复用/收敛」约束贯穿三阶段。验收锚点见 §8。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
|--------|----------------------|--------------------------|------------------|--------|
| **P1 协议 + host 搜索服务** | `searchContext` intent、`contextSearchResult` event、`ContextSearchMatch` 类型与校验器；`ContextSearchService`（findFiles 缓存 + watcher 失效 + 目录派生 + fuzzy 排序 + CAP 截断）；env 开关 | `src/ui/webview/protocol.ts`（intent/event/类型/`isWebviewIntent`）；`src/ui/webview/contextSearch.ts`（新增 `ContextSearchService`）；`src/ui/webview/provider.ts`（`case "searchContext"` + `postEvent(contextSearchResult)`）；复用 `src/ui/webview/contextReferences.ts:buildFileReference` | `protocol.test.ts::searchContext/contextSearchResult 校验`；`contextSearch.test.ts::fuzzy/cache/dir/exclude/cap`；`provider.test.ts::searchContext→result+取消` | 先把「后端能搜、协议能传」这条打通 |
| **P2 GUI `@` 触发 + 下拉** | TipTap `@` suggestion 配置；`ContextSearchDropdown`（键盘导航/loading/empty/truncated）；`App` 侧 requestId 去重 + 防抖 + 事件路由；选中插入既有 chip + 去重 | `gui/src/components/mentionSuggestion.ts`（新增，`@tiptap/suggestion` 配置工厂）；`gui/src/components/ContextSearchDropdown.tsx`（新增）；`gui/src/components/Composer.tsx`（挂载 suggestion、接线 `onQueryChange`/`onOpen`/`onClose`/`getKeyHandler`）；`gui/src/App.tsx`（intent/event 接线、`latestRequestId`、`matches` 状态下发下拉）；复用 `gui/src/components/ReferenceChip.tsx` | `ContextSearchDropdown.test.tsx::键盘/选中/空态/截断`；`Composer.test.tsx::@触发/去重/IME/Enter门控/debounce/stale`；`App.test.tsx::requestId 去重+事件路由` | 把「打 @ 弹框、选中插 chip」在前端做出来 |
| **P3 打磨 + 验收** | Enter/箭头/Esc 门控（open 态不触发发送）；无工作区提示；截断/空态文案；E2E 全链路；文档（本文 + 更新 `context-references.md` 非目标）；CSP 复核 | `gui/src/components/Composer.tsx`（keydown 门控 `isMentionOpenRef`）；`src/test/suite/support/hostE2eScenario.ts`（`@` 搜索场景）；`docs/architecture/at-mention-context-search.md`（本文）+ `docs/architecture/context-references.md §2.2`（非目标第 2 条）更新 | `hostE2eScenario.ts::@file 搜索→chip→发送→回放`（`E2E-ATMENTION-001`）；`npm run lint` + `test:unit` + `verify:vsix` | 收边角：键盘不打架、没工作区不崩、文档同步 |

#### §3.2.1 P1 — host 搜索服务技术要点

> 专业：`ContextSearchService` 持一份懒加载的工作区文件缓存，`search()` 每次先 `ensureCache()`，再对「文件 ∪ 派生目录」做子序列 fuzzy 打分与截断，最后用 `buildFileReference` 装配成 `ContextSearchMatch[]`。缓存由 `FileSystemWatcher` 的 create/delete 事件增量失效；每次新查询取消上一次的 `CancellationTokenSource`。
> 说人话：host 先把工作区文件列一遍存内存（别每次敲键都扫盘），你打字它就在内存里挑最像的几条送回去；文件增删了就顺手更新缓存。

```text
provider.ts case "searchContext"({requestId,query,kind})
   │  取消上一个 tokenSource；新建 tokenSource
   ▼
ContextSearchService.search(query, kind, limit=LIMIT, token)
   ├─ ensureCache():
   │     if 无缓存 → files = await workspace.findFiles("**/*", /*exclude*/undefined, MAX_FILES, token)
   │                 dirs  = deriveDirectories(files)   // 唯一父目录集
   │     watcher.onDidCreate/onDidDelete → 标记缓存 dirty（下次 ensureCache 重建/增量）
   ├─ candidates = kind==="file" ? [...files, ...dirs] : files
   ├─ scored = candidates
   │     .map(uri => ({uri, score: fuzzyScore(relPath(uri), query)}))
   │     .filter(score>0).sort(降序; 同分 → 打开编辑器优先 → 路径短优先 → 字典序)
   │     truncated = scored.length > limit
   │     top = scored.slice(0, limit)
   └─ matches = top.map(({uri}) => ({
   │       reference: buildFileReference(uri, {isDirectory: dirs.has(uri)}),   // 复用 contextReferences.ts
   │       description: 相对父目录 }))
   └─ provider.postEvent(contextSearchResult{requestId, query, matches, truncated})
```

fuzzy 打分（进程内、零依赖）：子序列匹配基础分 + 连续命中加成 + 词边界/`basename` 命中加权 + 大小写匹配加成 + 打开编辑器（`window.visibleTextEditors` / `workspace.textDocuments`）加权——借鉴 Continue `calculateFileSortPriority` 与 Codex `fuzzy-match` 的 subsequence 思路。

已知取舍：目录由文件路径**派生**，故**空目录（不含任何文件）不会出现在 `@folder` 候选**中。这是有意为之——空目录作为上下文引用无实际内容、价值极低，且避免为此单独遍历目录树；需要引用空目录的极少数场景可用 `+`/拖拽入口兜底。列入 §9 已知限制。

#### §3.2.2 P2 — GUI `@` 触发与插入技术要点

> 专业：`mentionSuggestion.ts` 导出一个 `@tiptap/suggestion` 配置工厂，`char:'@'`、`allowSpaces:false`。**关键设计：数据不经 `items()`**——`items()` 恒返回 `[]`，suggestion 只承担「触发 / 跟踪 query / 提供 `command` / 生命周期 / `onKeyDown` 转发」；`render()` 的 `onStart/onUpdate` 把 `query` 抛给 App（防抖后发 `searchContext`），`onExit` 关闭下拉；真正的 `matches` 由 App 的 React 状态承载、以 props 下发 `ContextSearchDropdown`；`command({editor,range,props})` 删除 `@query` 范围并插入既有 `ReferenceNode`。
> 说人话：为什么不让 TipTap 的 `items()` 去装结果？因为它是「问一次答一次、答完就渲染」的一次性通道，而我们的结果要绕一圈去 host，等它回来 `items()` 早交完卷了——晚到的结果就更新不上（竞态）。所以让 TipTap 只干两件事：「发现你打了 `@` 并把 query 交出来」「你选中后把 `@foo` 换成引用小方块」；中间的搜索和列表渲染，全交给 App 的 React 状态自己管。

```text
mentionSuggestion({ onQueryChange, onOpen, onClose, getKeyHandler }):
  { char: "@", allowSpaces: false, startOfLine: false,  // 默认 allowedPrefixes(空白前缀)：行首/空白后才触发
    items: () => [],                       // 数据不走 TipTap，杜绝一次性返回值竞态
    command: ({ editor, range, props: match }) => {      // props 即下拉选中的 match
       if (editorHasReference(editor, match.reference)) { editor.commands.deleteRange(range); return; }  // 去重, 幂等
       editor.chain().focus().deleteRange(range)         // 删掉 "@query"
             .insertContent([{type:"reference", attrs:match.reference}, {type:"text", text:" "}]).run();
    },
    render: () => ({
       onStart:  (props) => onOpen(props.command),       // isMentionOpenRef=true; 存 command 供下拉选中回调
       onUpdate: (props) => onQueryChange(props.query),  // query 变 → App 防抖发 searchContext
       onKeyDown:(props) => getKeyHandler()?.(props.event) ?? false,  // ↑↓/Enter/Tab/Esc 委托下拉, 返回 true 消费
       onExit:   ()      => onClose(),                   // isMentionOpenRef=false; 清空/关闭
    }) }
```

App 侧接线（数据与渲染的唯一真相）：
- `onQueryChange(query)` → 150ms 防抖 → `postIntent("searchContext",{requestId:++seq, query, kind:"file", sessionId})`，并置 `loading=true`。
- 收 `contextSearchResult(requestId, matches, truncated)`：`if (requestId !== latestRequestId) return;` 否则 `setState({matches, truncated, loading:false})`。
- `<ContextSearchDropdown>` 读 `{matches, loading, truncated}` 渲染，选中项调 `onOpen` 存下的 `command(match)`，由 suggestion 完成 `deleteRange + insert`。
- `getKeyHandler()` 返回下拉的 `onKeyDown`，让 suggestion 在 open 态把 ↑↓/Enter/Tab/Esc 转给下拉消费。

#### §3.2.3 P3 — Enter 门控与边角技术要点

> 专业：Composer 现有 `handleDOMEvents.keydown` 在 `Enter && !shift && !isComposing && canPrompt` 时发送（见 `Composer.tsx` 现状）。**为什么必须门控**：ProseMirror 的事件顺序是「先跑 `editorProps.handleDOMEvents.keydown`（我们的发送逻辑），再跑插件的 `handleKeyDown`（suggestion 的选中逻辑）」——若不拦，下拉开着时 Enter 会被发送逻辑先吃掉，suggestion 根本轮不到。故新增 `isMentionOpenRef`：keydown 头一行判其为 `true` 就 `return false`（放行），让事件继续流到 suggestion 的 `onKeyDown`。该 ref 在 suggestion `render().onStart` 置 `true`、`onExit` 置 `false`（即 §3.2.2 的 `onOpen/onClose`）。
> 说人话：平时按回车是「发送」；但菜单开着时按回车得是「选中这条」。而 ProseMirror 会先问「发送逻辑要不要处理这个回车」——所以我们得在发送逻辑第一句就说「菜单开着吗？开着我不处理」，把回车让给菜单。这个「菜单开着吗」的开关，在菜单弹出时打开、关闭时复位。

```text
Composer keydown（handleDOMEvents.keydown，先于 suggestion 的 handleKeyDown 执行）:
   if (isMentionOpenRef.current) return false;   // 放行 → 交给 suggestion 的 onKeyDown（Enter=选中）
   if (Enter && !shift && !isComposing && canPrompt) { preventDefault(); onSubmit(); return true; }

isMentionOpenRef 切换点: suggestion render().onStart → true;  onExit → false
```

其余边角：无工作区时 host 直接回空 + 前端一次性 `showWarningMessage`；`truncated` 时下拉底部渲染提示行；`@` 触发与 `compositionstart/end` 复用既有 `isComposingRef` 守卫，IME 组字期不误触发。

### §3.3 UI 设计（视觉稿）

> 专业：`@` 的 UI **对齐 VS Code 建议/补全控件**（`chatInputCompletions.ts` 的 `@` 补全 + `HighlightedLabel`）：一个锚定输入框上方、向上展开的候选下拉，定位/主题与既有 `Mode ▾`/`Model ▾` 下拉（`.tc-session-dropdown`）同源。候选行 = `[文件类型图标] 主文案(basename，命中子串高亮) …… 次文案(相对路径，右对齐灰字)`；**选中态 = 整行背景色**（`var(--vscode-list-activeSelectionBackground)`）而非 `>`/箭头前缀；四态（Loading / Results / Empty / Truncated）与 §B 状态机一一对应。
> 说人话：这个下拉长得就跟 VS Code / Cursor 里打 `@` 弹出来的那个一样——左边一个文件类型小图标，中间是文件名（你打的字在文件名里会**高亮加粗**），右边灰色小字是它的完整相对路径（用来区分重名文件）。当前选中的那一行是**整行涂上背景色**（上下键移动时背景跟着走），而不是在行首摆个 `>` 箭头。选中后 `@App` 那几个字就变成一个小方块 chip，跟你用 `+`/拖拽加进去的一模一样。

**（1）整体布局与位置**（下拉锚定输入框上方、向上展开；行内 = 图标 + 命中高亮文件名 + 右对齐灰色路径）

```text
┌───────────────────────────── Tomcat 侧栏 webview ─────────────────────────────┐
│  … 对话消息区（上方）…                                                          │
│                                                                                │
│  ┌────────────── 建议下拉（锚定输入框上方 · 向上展开）───────────────────────────┐│
│  │╔═════════════════════════════════════════════════════════════════════════╗ ││
│  │║ ◆ «App».tsx                       tomcat-vscode-ext/gui/src/App.tsx       ║ ││ ◀ 选中行 = 整行背景色
│  │╚═════════════════════════════════════════════════════════════════════════╝ ││   (list.activeSelectionBackground,
│  │  ◆ «app»ly.rs                      tomcat/src/core/compaction/apply.rs      ││    非 ">" 前缀)
│  │  ◆ «App».test.tsx                  tomcat-vscode-ext/gui/src/App.test.tsx   ││
│  │  ◆ «app»end.rs                     tomcat/src/infra/config/append.rs        ││
│  │  ▣ components/                     tomcat-vscode-ext/gui/src/components      ││ (目录: 图标+尾部 /)
│  │ ─────────────────────────────────────────────────────────────────────────  ││
│  │ 仅显示前 20 条 · ↑↓ 选择 · ↵/Tab 选中 · Esc 关闭                             ││ ← 页脚(截断+图例)
│  └──────────────────────────────────────────────────────────────────────────────┘│
│  ┌──────────────────────────────────────────────────────────────────────────────┐│
│  │ 帮我看下 @App|                                                                 ││ ← 输入框("@App"=查询串,未定型)
│  └──────────────────────────────────────────────────────────────────────────────┘│
│   [+] | Mode ▾ | Model ▾ | Effort ▾                              [ 发送 ]       │  ← 既有底部工具条(不改)
└──────────────────────────────────────────────────────────────────────────────────┘

图例：◆ = 文件图标（v1 通用 codicon，文件类型图标列 P2）  ·  ▣ = 目录  ·  «App» = 命中子串高亮(加粗)  ·  右侧灰字 = 相对路径
```

**（2）下拉四态**（与 §B 状态机对应；选中一律用背景色，无 `>`；无工作区不弹下拉）

```text
Results（有匹配·选中=背景色）                   Loading（已发查询·等结果）
╭────────────────────────────────────────╮    ╭────────────────────────────────────────╮
│╔══════════════════════════════════════╗│    │   搜索中…                                │
│║ ◆ «App».tsx      …/gui/src/App.tsx    ║│    ╰────────────────────────────────────────╯
│╚══════════════════════════════════════╝│
│  ◆ «app»ly.rs     …/compaction/apply.rs │    Empty（无匹配）
│  ▣ components/    …/gui/src              │    ╭────────────────────────────────────────╮
╰────────────────────────────────────────╯    │   未找到匹配文件                          │
                                               ╰────────────────────────────────────────╯
Truncated（命中过多·只显示前 N）                无工作区（不弹下拉，改用一次性提示条）
╭────────────────────────────────────────╮    ╭────────────────────────────────────────╮
│╔══════════════════════════════════════╗│    │  [!] 打开文件夹后可用 @                   │
│║ ◆ «index».ts     …/src/index.ts       ║│    ╰────────────────────────────────────────╯
│╚══════════════════════════════════════╝│
│  ◆ «index».test.ts …/src/tests/index…   │
│ ──────────────────────────────────────  │
│ 仅显示前 20 条，输入更精确关键词          │
╰────────────────────────────────────────╯
```

**（3）候选行结构**（图标 + 命中高亮文件名 + 右对齐灰色路径；选中=整行背景色）

```text
选中态：整行背景色（非 ">"）
╔══════════════════════════════════════════════════════════════════════════════╗
║ ◆   «App».tsx                                 tomcat-vscode-ext/gui/src/App.tsx ║
╚═╤════╤═════════════════════════════════════════════════════════╤══════════════╝
  │    │                                                          │
 图标   主文案 (basename)                                   次文案 (相对路径)
 file/  «…» = 命中子串高亮                                   右对齐 · 灰色 · 仅用于区分重名
 folder (加粗 + var(--vscode-list-highlightForeground))     (对应 VS Code CompletionItem.detail)
 (v1 通用 目录：文件名自带尾部 "/"（如 components/）
  codicon;
  类型图标 P2)

背景色令牌：选中 var(--vscode-list-activeSelectionBackground) · 悬停 var(--vscode-list-hoverBackground)
           —— 与既有 .tc-session-item--active 同源；不使用 ">"/箭头前缀（对齐 VS Code 列表控件）
```

**（4）选中：`@查询串` → 引用 chip**（产出与 `+`/拖拽/选区完全同款的 `ReferenceNode`）

```text
选中前（输入框里是纯文本）:   帮我看下 @App|
                                     └────── 纯文本 "@App"（查询串, 可继续改/删）

按 ↵ 选中 App.tsx 后:         帮我看下 [ ◆ App.tsx  x ]|
                                     └────── 原子 chip（ReferenceNode, 可点 x 删除）
                                             与 +/拖拽/选区 同源, 发送/落盘/回放零差异
```

**（5）键盘图例 · 视觉令牌（对齐 VS Code）· 设计取舍**

```text
键盘：
  ↑ / ↓        移动选中 —— 背景色随之移动（无 ">" 标记）
  ↵ / Tab      选中当前项 → 变 chip（open 态被 suggestion 消费, 不触发「发送」, 见 §3.2.3）
  Esc          关闭下拉（不产出引用, 保留已输入的 "@查询串" 文本）
  空格 / 删到@前 取消触发（回到普通文本）

视觉令牌（复用 VS Code 注入 webview 的 --vscode-* 变量, 自动随主题明暗变化）：
  选中背景  var(--vscode-list-activeSelectionBackground)   ← 与 .tc-session-item--active 同源
  悬停背景  var(--vscode-list-hoverBackground)
  命中高亮  var(--vscode-list-highlightForeground) + 加粗   ← 对应 VS Code HighlightedLabel
  路径次文案 var(--vscode-descriptionForeground)（灰、右对齐）
  参考实现：vscode `src/vs/base/browser/ui/highlightedlabel/highlightedLabel.ts`（命中高亮）、
            `src/vs/platform/theme/common/colors/listColors.ts`（选中背景/命中前景色令牌）、
            `chatInputCompletions.ts`（@ 补全行 = 图标 + label + detail(路径)）

设计取舍（少即是多）：
  · 选中=整行背景色, 不用 ">"/箭头前缀 → 与 VS Code 列表/建议控件完全一致, 零违和
  · 图标：v1 用通用 file/folder 图标（codicon）；文件类型专属图标（seti）列 P2,
          避免首版为图标引入 file-icon-theme 依赖（与「不引二进制/重依赖」一脉相承）
  · 命中高亮 + 右对齐灰色路径 → 快速扫读 + 区分重名, 信息密度恰到好处
  · 复用 .tc-session-dropdown + --vscode-* 令牌 → 与 Mode/Model/Effort 下拉一致, 零学习成本、零额外维护
```

---

## §4 协议（webview ↔ host）

> 本方案**只**新增 webview↔host 的一条 intent + 一条 event，**不改** host↔serve wire 协议（`wire.d.ts` / `serve.schema.json` 均不动）。引用（reference）的最终形态仍以 `tomcat/src/api/serve/types.rs:ServeContentSegment` 为单一事实源；本节新增结构的单一事实源是 `src/ui/webview/protocol.ts`。
> 说人话：新增的只是「前端问、后端答」的搜索小协议；搜到文件后塞进消息用的还是老结构，所以后端/落盘/schema 全不用动。

### 4.1 `searchContext` intent（webview → host）

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `type` | `"searchContext"` | 是 | — | 恒定 | intent 判别式 | 固定值 |
| `messageId` | string | 是 | — | 所有 intent | 既有信封字段 | 消息 ID |
| `data.requestId` | string | 是 | — | 每次查询 | 单调递增；用于陈旧丢弃与结果关联 | 这次查询的编号 |
| `data.query` | string | 是 | `""` | 每次查询 | `@` 后到光标的文本；空串代表刚打 `@` | 你输入的关键词 |
| `data.kind` | `"file"` | 否 | `"file"` | 预留扩展 | 首版仅 `file`；`symbol` 为 P2 | 搜什么类型（先只有文件） |
| `data.sessionId` | string \| null | 否 | null | 多会话 | 归属会话；缺省用当前活跃会话 | 属于哪个会话 |

### 4.2 `contextSearchResult` event（host → webview）

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
|------|-----------|------|--------|----------|------|--------|
| `type` | `"contextSearchResult"` | 是 | — | 恒定 | event 判别式 | 固定值 |
| `requestId` | string | 是 | — | 结果关联 | 回带请求的 `requestId`；前端据此丢弃过期 | 对应哪次查询 |
| `query` | string | 是 | — | 校验 | 回带请求 query，便于前端二次核对 | 当时查的词 |
| `matches` | `ContextSearchMatch[]` | 是 | `[]` | 结果集 | 已按分数降序、已截断到 limit | 命中列表 |
| `truncated` | boolean | 否 | false | 命中超上限 | `true` → 下拉提示「仅显示前 N 条」 | 是否被截断 |

### 4.3 `ContextSearchMatch` 子结构

```text
interface ContextSearchMatch {
  reference: WebviewReference;   // == Extract<ServeContentSegment,{type:"reference"}>，kind:"file"
  description?: string | null;   // 相对父目录，作下拉副标题；不参与引用本体
}

// WebviewReference（既有，未改）：
//   { type:"reference", kind:"file", path:string, label:string,
//     lineStart?:null, lineEnd?:null, text?:null }
```

`reference` 直接是既有 `WebviewReference`（`protocol.ts`），因此选中后可无转换地插入 `ReferenceNode` 并汇入 `segments`——这是「零协议扩散」的关键。

### 4.4 调用样例

```jsonc
// webview → host：用户打了 "@comp"
{
  "type": "searchContext",
  "messageId": "intent-1731400000000-ab12cd34",
  "data": { "requestId": "7", "query": "comp", "kind": "file", "sessionId": "sess_01" }
}

// host → webview：命中 2 条（示意；实际经 event 信封 channel:"event" 包裹）
{
  "type": "contextSearchResult",
  "requestId": "7",
  "query": "comp",
  "truncated": false,
  "matches": [
    { "reference": { "type": "reference", "kind": "file", "path": "gui/src/components/Composer.tsx", "label": "Composer.tsx" },
      "description": "gui/src/components" },
    { "reference": { "type": "reference", "kind": "file", "path": "gui/src/components/ReferenceChip.tsx", "label": "ReferenceChip.tsx" },
      "description": "gui/src/components" }
  ]
}
```

### 4.5 事件信封与校验

- `contextSearchResult` 作为 `HostEventFrameContent` 的新成员，走既有 `HostToWebviewFrame{channel:"event"}` 通道（`protocol.ts`）。
- `searchContext` 在 `isWebviewIntent()` 新增分支校验：`isString(data.requestId) && isString(data.query) && (data.kind===undefined||data.kind==="file")`。
- 前端对入站 `contextSearchResult` 做结构校验：`matches` 为数组且每项 `reference` 通过既有 `isWebviewReferenceShape()`（`protocol.ts`），过滤非法项后再渲染。

---

## §5 文件职责总览（One-Glance Map）

> 阅读顺序：自上而下就是一次 `@` 搜索的真实调用链——GUI 触发 → host 搜索 → 结果回灌 → 复用既有引用系统发送。**本方案改动 0 个 `*.rs` 文件**：serve 协议、transcript 落盘、LLM flatten 全部复用 `context-references`，故 One-Glance 只含 TS 节点，并在末尾显式标注 Rust 侧「未改」。
> 说人话：新增的东西只在两块——输入框上半段（GUI）和 host 搜索服务；下半段（发送/落盘/发模型）是老代码，一个字不动。

```text
┌─ gui/src/components/Composer.tsx ────────────────────────────────────────┐
│ 【已存在, 小改】useEditor 挂载 mentionSuggestion；接线 App 的搜索回调      │
│  - keydown 门控：isMentionOpenRef → open 态 return false（不发送）         │
│  - 复用 ReferenceNode(atom) / serializeComposerDocument / editorHasReference│
└───────────────┬───────────────────────────────────────────────────────────┘
                │ onOpen(command)/onQueryChange(query)/onClose()  (数据不经 items)
                ▼
┌─ gui/src/components/mentionSuggestion.ts 【新增】────────────────────────────┐
│  @tiptap/suggestion 配置工厂：char:'@', allowSpaces:false                   │
│  items()恒返回[]；render.onStart/onUpdate 抛 query；onKeyDown 委托下拉      │
│  command({range,props:match}): deleteRange(@query) + insertContent(chip)   │
└───────────────┬───────────────────────────────────────────────────────────┘
                │ query（生命周期回调）           ▲ command(match)（下拉选中回灌）
                ▼                                │
┌─ gui/src/App.tsx（数据与渲染唯一真相 / hub）───────────────────────────────┐
│ 【已存在, 小改】onQueryChange → 150ms 防抖 → postIntent("searchContext",…)  │
│  - postIntent("searchContext",{requestId:++seq,query,kind})                │
│  - latestRequestId：contextSearchResult.requestId !== latest → 丢弃         │
│  - setState({matches,loading,truncated})                                    │
└──────┬──────────────────────────────────────────────┬──────────────────────┘
       │ intent ▼ / event ▲ (与 host)                  │ props{matches,loading,truncated} ▼ / onSelect ▲
       │                                               ▼
       │        ┌─ gui/src/components/ContextSearchDropdown.tsx 【新增】──────────────┐
       │        │  锚定 composer 上方(复用 .tc-session-dropdown 静态定位)            │
       │        │  键盘导航(↑↓/Enter/Tab/Esc)、loading、empty、truncated 提示        │
       │        │  行渲染复用 ReferenceChip 图标 + label + description(相对目录)      │
       │        │  onSelect(match) → mention.command(match)（回灌 suggestion 插入）  │
       │        └─────────────────────────────────────────────────────────────────────┘
       ▼
┌─ src/ui/webview/protocol.ts ───────────────────────────────────────────────┐
│ 【已存在, 小改】WebviewIntent += searchContext；HostEventFrameContent +=     │
│  contextSearchResult；ContextSearchMatch 类型；isWebviewIntent 新增分支      │
└───────────────┬───────────────────────────────────────────────────────────┘
                ▼
┌─ src/ui/webview/provider.ts ───────────────────────────────────────────────┐
│ 【已存在, 小改】handleIntent: case "searchContext"                          │
│  - 取消上一个 CancellationTokenSource；调 ContextSearchService.search       │
│  - postEvent(contextSearchResult{requestId,matches,truncated})              │
└───────────────┬───────────────────────────────────────────────────────────┘
                ▼
┌─ src/ui/webview/contextSearch.ts 【新增】──────────────────────────────────┐
│ ContextSearchService                                                        │
│  - ensureCache(): workspace.findFiles("**/*",undefined,MAX_FILES,token)     │
│  - FileSystemWatcher(onDidCreate/onDidDelete) 增量失效                       │
│  - deriveDirectories(files) 供 @folder；fuzzyRank + 打开编辑器加权 + CAP     │
│  - buildFileReference(uri,{isDirectory}) ← 复用 contextReferences.ts        │
│  配套：src/ui/webview/tests/contextSearch.test.ts                           │
└───────────────┬───────────────────────────────────────────────────────────┘
                ▼
┌─ src/ui/webview/contextReferences.ts 【复用, 未改】────────────────────────┐
│ buildFileReference / resolveUriToFileReference（selection 入口不涉及）      │
└─────────────────────────────────────────────────────────────────────────────┘

▼ 选中插入后（既有链路，全部未改）：
┌─ 引用汇入既有「上下文引用系统」（见 context-references.md）───────────────┐
│ ReferenceNode → serializeComposerDocument → segments → prompt intent →      │
│ serve build_user_message → transcript 落盘 → 历史回放 → LLM flatten         │
│ 【未改签名 / 依赖既有实现】tomcat/src/api/serve/{types,commands}.rs、        │
│  tomcat/src/core/llm/types.rs 等 Rust 文件 —— 本方案 0 改动                 │
└─────────────────────────────────────────────────────────────────────────────┘
```

---

## §6 配置与环境变量

> 总则：**env > VS Code 设置 > 默认**。这些开关主要用于调优大仓库表现与测试注入；日常无需设置。

| 变量 / 设置 | 取值 | 含义 | 优先级 | 说人话 |
|-------------|------|------|--------|--------|
| `TOMCAT_CONTEXT_SEARCH_MAX_FILES` | 整数（默认 `20000`） | 候选缓存文件数上限，超出即停止收集并置 `truncated` | env（最高） | 大仓库最多缓存这么多文件，防爆内存 |
| `tomcat.contextSearch.maxFiles` | 整数 | 同上，VS Code 设置形态 | config | 设置界面里也能调 |
| `TOMCAT_CONTEXT_SEARCH_LIMIT` | 整数（默认 `20`） | 单次返回给下拉的最大命中数 | env | 下拉最多显示几条 |
| `TOMCAT_CONTEXT_SEARCH_DEBOUNCE_MS` | 整数（默认 `150`） | 前端搜索防抖窗口 | env | 打字停多久才真正发查询 |
| `TOMCAT_CONTEXT_SEARCH_DISABLE` | `1` / `true` | 关闭 `@` 触发（回退到仅 `+`/拖拽/选区） | env | 出问题时的总开关 |

`findFiles` 的排除口径不额外配置：传 `exclude=undefined` 即复用用户的 `files.exclude` + `search.exclude`，与 VS Code Quick Open 行为一致（尊重用户既有忽略规则）。

---

## §7 错误模型 / 截断 / 警告

> 所有异常都归一化为「不阻塞输入」的结局：`@` 搜索失败最多是「没结果 + 一句提示」，绝不因搜索问题卡住用户打字或发送。

```text
无工作区文件夹      → host 回 matches:[]；前端一次性 showWarningMessage("打开文件夹后可用 @")，不弹菜单
无匹配              → host 回 matches:[]（truncated:false）；下拉显示空态「未找到匹配文件」
命中超 CAP          → host 截断到 LIMIT + truncated:true；下拉底部提示「仅显示前 N 条，输入更精确关键词」
缓存超 MAX_FILES    → 停止收集 + truncated:true（同上提示），已收集部分仍可搜
findFiles 抛错/取消 → host catch → 回 matches:[]（被取消的请求因 requestId 过期而被前端丢弃，无副作用）
重复引用            → command 阶段 editorHasReference 命中 → 删 @query 但不插 chip（幂等）
陈旧响应            → 前端 requestId !== latest → 静默丢弃（不渲染、不报错）
host 内部异常       → 记 console.error + 回空结果；不 throw 到 intent 边界（避免打断 webview 消息循环）
```

无「致命 Err」路径：`@` 是可选增强入口，任何失败都退化为「当前查询无结果」，用户仍可用 `+`/拖拽/选区，或继续打字发送。

---

## §8 测试矩阵（验收）

> §3.1 / §3.2 的每条可观察交付，在此都能找到锁死它的测试或机制。状态列：`✅ 日期` / `PENDING` / `阻塞于 X`。

| 维度 | 用例 / 编号 | 状态 | 说人话 |
|------|-------------|------|--------|
| 单元(协议) | `src/ui/webview/tests/protocol.test.ts`：`searchContext` intent 校验（缺 requestId/query 拒绝）、`contextSearchResult` 结构校验、非法 match 过滤 | ✅ 2026-07-11 | 协议字段该拒的拒、该收的收 |
| 单元(host 搜索) | `src/ui/webview/tests/contextSearch.test.ts`：fuzzy 排序（basename/边界/打开文件加权）、目录派生、`files.exclude` 生效、CAP 截断置 `truncated`、缓存 watcher 失效 | ✅ 2026-07-11 | 搜得准、排得对、忽略规则生效、超量截断 |
| 单元(provider) | `src/ui/webview/tests/provider.test.ts`：`searchContext` → `contextSearchResult` 事件、新 query 取消旧 `CancellationTokenSource`、无工作区回空 | ✅ 2026-07-11 | 后端能搜能取消 |
| 单元(GUI 触发) | `gui/src/components/Composer.test.tsx`：`@` 行首/空白边界触发、IME 组字不误触发、Enter 门控（open 态不发送）、去重不重复插、防抖/陈旧丢弃 | ✅ 2026-07-11 | 打 @ 弹框、回车不误发、重复不插 |
| 单元(GUI 下拉) | `gui/src/components/ContextSearchDropdown.test.tsx`：↑↓/Enter/Tab/Esc 导航、loading/empty/truncated 三态、点击选中插 chip | ✅ 2026-07-11 | 下拉键盘鼠标都能用 |
| 单元(App) | `gui/src/App.test.tsx`：`requestId` 去重（旧结果丢弃）、event→下拉数据路由、reference-only 可发送 | ✅ 2026-07-11 | 结果乱序不串台 |
| 集成 | `src/ui/webview/tests/webview_provider_flow.test.ts`：`@` 搜索 → 选中 → `segments` 透传 → 与 `+`/拖拽产出的引用同形 | ✅ 2026-07-11 | 整条前后端串起来测 |
| E2E | `E2E-ATMENTION-001`（`src/test/suite/support/hostE2eScenario.ts`）：真实工作区 `@` 搜文件 → inline chip → 发送 → reload 历史回放仍是 chip | ✅ 2026-07-11 | 用户真实操作全链路 |
| E2E | `E2E-ATMENTION-002`：`@` 搜目录（尾部 `/`）→ chip；无工作区 `@` 提示不崩 | ✅ 2026-07-11 | 目录与空态兜底 |
| 关键承诺 | R4「零后端改动」：`cargo test` 全绿；git diff 不含 `*.rs`、`wire.d.ts`、`serve.schema.json`；serve schema fixture 无 diff | ✅ 2026-07-11 | 保证真没碰后端 |
| 关键承诺 | R7「CSP 不破」：`verify:vsix` + 手工在严格 CSP 下打 `@` 无控制台 CSP 报错 | ✅ 2026-07-11 | 下拉不踩 CSP |
| 文档 | 本文定稿 + `context-references.md §2.2`（非目标第 2 条）同步更新（`@` 由本方案立项覆盖） | ✅ 2026-07-11 | 文档与代码不两张皮 |

---

## §9 风险与应对

| 风险 | 影响 | 应对（具体动作） | 说人话 |
|------|------|--------------------|--------|
| 大仓库 `findFiles("**/*")` 慢 / 占内存 | 中 | 懒加载 + `MAX_FILES=20000` 上限（超量 `truncated`）+ `FileSystemWatcher` 增量失效 + 每查询 `CancellationTokenSource` 取消旧查询；env 可调 | 文件太多就设上限、缓存、可取消，别每次扫全盘 |
| 慢查询乱序返回覆盖新结果 | 中 | `requestId` 单调递增，前端 `latestRequestId` 丢弃过期；host 取消上一个 token（借鉴 Cline `latestSearchTokenRef` / Codex `pending_query`） | 打字快时旧结果回来晚了直接扔 |
| 下拉浮层触发 CSP 拦截 | 中 | 不用 tippy/floating-ui（它们注入运行时 `<style>` 标签，被 `style-src ${cspSource}` 拦）；下拉锚定 composer、样式全进 bundle CSS，无动态行内样式；澄清：CSP 不拦 React/CSSOM 的 `element.style` 赋值；`verify:vsix` + 手工复核无 CSP 报错 | 不用会注入 `<style>` 标签的浮层库；React 设的样式 CSP 不管 |
| `@folder` 搜不到空目录 | 低 | 目录由文件路径派生，空目录不出现在候选（§3.2.1 已知取舍）；空目录引用价值低，需要时用 `+`/拖拽兜底 | 完全空的文件夹 `@` 搜不到，用 + 号加 |
| `@` 触发与中文 IME 冲突 | 中 | 复用既有 `isComposingRef`（`compositionstart/end`）守卫，组字期不触发 suggestion，不误发送 | 中文输入时打 @ 不乱弹、回车不误发 |
| Enter 在下拉开时误发送消息 | 高 | `isMentionOpenRef` 门控：open 态 keydown 直接 `return false` 交 suggestion 消费（§3.2.3） | 菜单开着按回车是选中，不是发送 |
| `@` 引用与 `+`/拖拽产出不一致 | 中 | 统一走 `buildFileReference` + `ReferenceNode`；`referenceIdentity` 跨入口去重；集成测试断言同形 | 三个入口插进去的 chip 必须一模一样 |
| 无工作区 / 多根工作区路径歧义 | 低 | 无工作区回空 + 提示；多根用 `asRelativePath`（既有 `contextReferences.ts` 口径），必要时 label 带根名 | 没打开文件夹给提示；多根用相对路径 |
| 后端被误改导致协议扩散 | 中 | 关键承诺测试：git diff 不含 `*.rs`、`wire.d.ts`、`serve.schema.json`；serve schema fixture 无 diff（§8） | 用测试兜住「真没碰后端」 |
| 缓存与磁盘不同步（新建文件搜不到） | 低 | `FileSystemWatcher` create/delete 失效缓存；兜底可加 TTL 定时重建（P2） | 新建的文件也能很快搜到 |

---

## §10 历史决策 / 跨文档修订

### 10.1 已被取代的结论

- ~~「`@` 内联搜索本期不做（需异步搜索子系统，后续单独立项）」~~ → **本方案即为该立项**。原判断见 `.cursor/plans/fix_webview_file_drag_drop_fec1e54b.plan.md` L118 与 `context-references.md §2.2`（非目标第 2 条）；当时因「拖拽修复」范围收敛而延后，现独立立项落地。
- ~~「引用只能靠 `+` / 拖拽 / 选区三入口」~~ → **否**：新增第四入口「输入框 `@` 搜索」，但**刻意不新增数据模型**——`@` 产出与前三者同形的 file 引用（R4）。

### 10.2 跨文档修订

- [`context-references.md`](context-references.md) **§2.2 非目标第 2 条**已同步更新为：「`@` 内联搜索已由 [`at-mention-context-search.md`](at-mention-context-search.md) 立项覆盖，`+`/拖拽/选区仍为并存入口」。
- 参与者（participant）前端不涉及 `@` 搜索（无富输入框）；本方案仅作用于 webview 前端，participant 行为不变，无需修订其文档。

---

## 一句话总结

`@` 功能的本质不是「再造一套上下文系统」，而是**给已经跑通的「上下文引用系统」补上一个输入入口**：TipTap `@tiptap/suggestion` 负责触发与插入，host `ContextSearchService`（`findFiles` 缓存 + 内存 fuzzy）负责搜索，`requestId` + 防抖负责并发正确性——选中之后，chip → 有序 `segments` → 落盘 / 回放 / LLM flatten 这条老路一个字不改。于是整个特性被收敛在 **webview↔host 一条 intent + 一条 event + 一个前端下拉 + 一个 host 搜索服务** 内，零后端改动、零协议扩散，这正是「少即是多」在架构上的兑现。
