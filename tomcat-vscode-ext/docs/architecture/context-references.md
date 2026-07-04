# Tomcat 上下文引用：选区 Add-to-Chat 与文件拖拽

> 适用范围：`tomcat-vscode-ext` 新增两类“上下文引用”能力：
>
> 1. 编辑器选区通过 `Add to Tomcat Chat` 注入到聊天输入框；
> 2. 文件/文件夹通过智能 `+` 与 `Shift` 拖拽注入到聊天输入框；
> 3. 两者都以**内联原子 chip + 有序 segments** 进入 transcript、webview UI 与 LLM payload。
>
> 单一事实源：
>
> 1. host↔serve 线协议以 [`../../src/serveClient/wire.d.ts`](../../src/serveClient/wire.d.ts) 为准；
> 2. host 生命周期与事件转发以 [`../../src/ui/webview/provider.ts`](../../src/ui/webview/provider.ts) 为准；
> 3. webview 状态收敛以 [`../../src/ui/webview/state.ts`](../../src/ui/webview/state.ts) 为准；
> 4. 富输入框与 chip 渲染以 [`../../gui/src/components/Composer.tsx`](../../gui/src/components/Composer.tsx)、[`../../gui/src/components/ReferenceChip.tsx`](../../gui/src/components/ReferenceChip.tsx)、[`../../gui/src/components/MessageBubble.tsx`](../../gui/src/components/MessageBubble.tsx) 为准；
> 5. VS Code 入口（命令 / CodeLens / 快捷键）以 [`../../src/extension.ts`](../../src/extension.ts) 与 [`../../package.json`](../../package.json) 为准。

---

## 1. 先说人话

这次不是“给输入框多加两个附件按钮”。

真正要解决的是：**用户给 LLM 的上下文，不能只是一坨最后拼起来的纯文本；它必须保留前后顺序、来源、显示形态、回放能力。**

用户看到的是：

```text
帮我看这个问题 [selection chip] 然后再参考 [file chip] 给方案
```

系统内部也必须保留这个顺序，而不是偷偷变成：

```text
text = "帮我看这个问题 然后再参考 给方案"
references = [selection, file]
```

因为后一种结构已经丢了“引用插在句子中间哪个位置”这个语义。

所以本方案的核心不是按钮，也不是拖拽，而是：

```text
输入内容 = 有序 segments

segments = [
  text("帮我看这个问题 "),
  reference(selection),
  text(" 然后再参考 "),
  reference(file),
  text(" 给方案")
]
```

一句话总结：

**UI 看见的是 chip，协议传的是 ordered segments，落盘存的是 structured content，发给 LLM 时再按模型能力做 flatten。**

---

## 2. 目标与非目标

### 2.1 目标

1. 选中文本后，用户可以通过 CodeLens / 右键菜单 / 快捷键把选区放进聊天输入框。
2. 用户可以点 `+` 选择任意文件/文件夹；图片/PDF 进附件，其他文件/文件夹进上下文 chip。
3. 用户按住 `Shift` 从 VS Code Explorer 拖工作区文件到输入框时，行为与 `+` 共用同一套分类逻辑。
4. 输入框里的引用是可见、可 hover、可删除、可回放的。
5. transcript 历史回放后，用户消息仍然保持引用 chip，而不是退化成一整段纯文本。
6. 发给 LLM 时，引用顺序必须与用户输入顺序一致。

### 2.2 非目标

1. 不在这次里绕过 VS Code webview 沙箱；侧边栏 webview 的**非 Shift 拖拽**和**外部文件直接拖入取路径**都不是扩展能修的。
2. 不在这次里做 `@` 内联搜索；本期可靠入口是智能 `+`，不是再引入一套异步搜索系统。
3. 不在这次里引入富文本格式化能力；composer 仍然只是“纯文本 + 原子 chip”。
4. 不在这次里把 Completions 变成真正多模态；Completions 仍是 text-only，引用只会被 flatten 成文本。

---

## 3. 总体结构

### 3.1 总图

```text
VS Code 编辑器 / Explorer / 外部拖拽
        │
        ├─ 选区入口
        │   ├─ command: tomcat.addSelectionToChat
        │   ├─ CodeLens
        │   ├─ editor/context
        │   └─ keybinding
        │
        └─ 文件入口
            ├─ composer "+" -> pickContext intent
            └─ Shift + Explorer drop -> resolveDrop intent
                         │
                         ▼
┌─ src/extension.ts / src/ui/webview/provider.ts ─────────────────────────────┐
│ 1. classifyPickedUri(uri): 图片/PDF -> 附件, 其他 -> 上下文引用             │
│ 2. 引用 -> postInsertReference；附件 -> pendingAttachments + postState       │
└────────────────────────────────────┬─────────────────────────────────────────┘
                                     ▼
┌─ gui/src/components/Composer.tsx (TipTap) ──────────────────────────────────┐
│ 1. reference node 以内联原子 chip 显示                                      │
│ 2. 文本 + 引用共同形成 ordered segments                                      │
│ 3. submit 时把 draft 序列化成 segments + projection text                    │
└────────────────────────────────────┬─────────────────────────────────────────┘
                                     ▼
┌─ src/ui/webview/provider.ts ─────────────────────────────────────────────────┐
│ send prompt(params.segments, text, attachments)                             │
└────────────────────────────────────┬─────────────────────────────────────────┘
                                     ▼
┌─ tomcat serve / transcript / LLM bridge ────────────────────────────────────┐
│ 1. transcript 落 structured content                                         │
│ 2. Responses / Completions 按能力 flatten                                   │
└────────────────────────────────────┬─────────────────────────────────────────┘
                                     ▼
┌─ src/ui/webview/state.ts / MessageBubble.tsx ───────────────────────────────┐
│ 历史回放时把 input_reference 重新还原成 UI chip                               │
└──────────────────────────────────────────────────────────────────────────────┘
```

### 3.2 为什么必须是 ordered segments

```text
用户真实输入：
  "先看 " + [选区A] + " 再结合 " + [文件B] + " 给建议"

如果拆成：
  text = "先看  再结合  给建议"
  refs = [A, B]

那 LLM 永远不知道 A 和 B 分别插在哪。
```

所以本方案把“顺序”作为第一原则，而不是把引用当作 sidecar metadata。

---

## 4. 数据模型

### 4.1 前端 reference 形态

前端统一使用 `WebviewReference`：

```text
selection:
  {
    type: "reference",
    kind: "selection",
    path,       // 工作区内用相对路径，工作区外回退绝对路径
    label,      // 例：app.ts:12-18
    lineStart,
    lineEnd,
    text        // 选区快照
  }

file:
  {
    type: "reference",
    kind: "file",
    path,       // 工作区内用相对路径，工作区外回退绝对路径
    label       // 文件名（目录保留尾部 /）
  }
```

具体构造逻辑见 [`../../src/ui/webview/contextReferences.ts`](../../src/ui/webview/contextReferences.ts)。

### 4.2 消息 segments

前端到 host 使用 `ServeContentSegment` / `WebviewMessageSegment`：

```text
type Segment =
  | { type: "text"; text: string }
  | { type: "reference"; kind: "selection" | "file"; ... }
```

关键点：

1. `text` 与 `reference` 是同层级兄弟节点；
2. 顺序就是语义；
3. user message 的 `text` 只是 projection，真正权威内容是 `segments`。

### 4.3 transcript 落盘形态

落盘不是“外挂一组 references”，而是 message `content` 内部直接 interleave：

```text
[
  { "type": "input_text", "text": "Inspect " },
  {
    "type": "input_reference",
    "ref_kind": "selection",
    "path": "src/app.ts",
    "label": "app.ts:3-5",
    "line_start": 3,
    "line_end": 5,
    "text": "const answer = 42;"
  },
  { "type": "input_text", "text": " please" }
]
```

说人话：**transcript 现在记住的不是“这句话里提过哪些引用”，而是“这句话是怎么被用户拼出来的”。**

---

## 5. 触发链路

### 5.1 选区 Add-to-Chat

```text
用户选中文本
  │
  ├─ CodeLens / 右键 / 快捷键 -> tomcat.addSelectionToChat
  │
  ▼
extension.ts
  │  buildSelectionReference(editor)
  │  focusWebviewSurface()
  │  webviewProvider.postInsertReference(sessionId, reference)
  ▼
provider.ts -> event(insertReference)
  ▼
App.tsx
  │  收到 insertReference
  │  若 composer 已 ready -> 直接 insert
  │  否则进入 pendingInsertions 队列
  ▼
Composer.tsx
  │  TipTap 插入一个原子 reference node
  ▼
用户看到 chip
```

这里有三个用户入口，但底层只有一个命令和一套 reference 构造逻辑，所以不会出现“CodeLens 插进去的格式”和“右键插进去的格式”不一致。

### 5.2 文件/文件夹入口：智能 `+` 与 `Shift` 拖拽共用同一分类器

先说结论：**一个 URI 最终是附件还是上下文引用，不取决于入口，而取决于它是什么。**

```text
共享分类器 classifyPickedUri(uri) [host]
  ├─ 目录                    -> reference
  ├─ 图片(.png/.jpg/.gif/...) -> attachment(kind=image, base64)
  ├─ PDF                     -> attachment(kind=file, base64)
  └─ 其他文件(.ts/.md/...)    -> reference
```

两个入口都只做“拿到 URI”，然后把决定权交给 host：

```text
入口 A: "+"
  Composer.tsx -> pickContext
               -> provider.ts showOpenDialog(任意文件/文件夹)
               -> classifyPickedUri(uri)

入口 B: Shift 拖拽
  Composer.tsx -> extractDropUris(dataTransfer)
               -> resolveDrop
               -> provider.ts classifyPickedUri(uri)
```

最终分流：

```text
reference  -> buildFileReference(...) -> postInsertReference(sessionId, reference)
attachment -> readPendingAttachment(...) -> pendingAttachments[] -> postState()
```

这带来三个直接结果：

1. `+` 现在是**可靠主入口**。它能选任意文件/文件夹，也能处理工作区外文件，因为 `showOpenDialog` 在 host 侧能拿到绝对路径。
2. 拖图片不再变成“废引用”。拖拽和 `+` 复用同一分类器后，图片/PDF 会走附件通道，代码/目录走上下文通道。
3. 附件限制不会再误伤普通文件引用。非图片/非 PDF 文件现在根本不进 `parse_attachment_part`，而是走 `ServeContentSegment -> ContextReference`。

#### 为什么拖拽仍然必须按住 `Shift`

这不是我们故意设计得别扭，而是 VS Code 对侧边栏 webview 的平台护栏：

```text
整窗发生拖拽时:
  没按 Shift -> webview iframe pointerEvents = none
  按住 Shift -> webview iframe pointerEvents = auto
```

对应到现象就是：

```text
没按 Shift
  -> iframe 收不到 dragenter / dragover / drop
  -> composer 不可能高亮
  -> resolveDrop 也不可能触发
```

这个切换由 VS Code 内部的 `WebviewWindowDragMonitor` 做，扩展侧没有可关闭的开关，也没有“webview 最外层 drop API”可以绕过去。

#### 为什么外部文件拖不进来，但 `+` 可以

```text
Explorer(工作区内) 拖拽:
  text/uri-list 里有 file:///...    -> webview 能解析到 URI

Finder/桌面/别的 App 拖拽:
  沙箱把 file.path / text/uri-list 抹掉 -> webview 拿不到路径
```

而“上下文引用”本质上必须拿到路径，才能后续 `read` / `open` / hover 展示。所以：

1. **工作区内文件**：可以按住 `Shift` 走拖拽，也可以走 `+`；
2. **工作区外文件/文件夹**：只能走 `+`；
3. **我们不会尝试从 webview 最外层绕过沙箱**，因为 VS Code 没给扩展开放那条 API。

---

## 6. 富输入框实现

### 6.1 为什么选 TipTap

因为这次不是普通 `<textarea>` 能优雅完成的需求。

我们需要同时满足：

1. 文本与引用交错排列；
2. 引用是原子 chip，光标不能把它拆烂；
3. 可以删除单个 chip；
4. 可以序列化成 ordered segments；
5. 输入法、粘贴、Enter/Shift+Enter、拖拽都还得正常。

`TipTap/ProseMirror` 正好擅长这种“文本中夹原子节点”的场景。

### 6.2 composer 内部模型

```text
TipTap doc
  └─ paragraph
      ├─ text("Inspect ")
      ├─ reference(selection)
      ├─ text(" carefully")
      └─ reference(file)
```

提交前，`serializeComposerDocument()` 把这棵编辑树拍平为：

```text
segments[] + projection text
```

其中：

1. `segments` 给协议和 transcript 用；
2. `text` 只是 projection，方便旧接口、日志与某些 UI 文案。

---

## 7. provider / state / UI 如何闭环

### 7.1 provider 的职责

[`../../src/ui/webview/provider.ts`](../../src/ui/webview/provider.ts) 负责三件事：

1. 接 intent：`prompt`、`pickContext`、`resolveDrop`、`retryUserMessage` 等；
2. 在 host 侧统一做 `classifyPickedUri` 分流；
3. 引用走 `insertReference`，附件走 `pendingAttachments`，提交时再把前端 `segments` 透传给 serve。

ASCII 图：

```text
webview intent
   │
   ├─ prompt      -> messenger.request({ text, params: { segments } })
   ├─ pickContext -> showOpenDialog -> classifyPickedUri -> reference|attachment
   ├─ resolveDrop -> classifyPickedUri -> reference|attachment
   └─ retry       -> 复用失败消息上的 segments 再发一次
```

### 7.2 state store 的职责

[`../../src/ui/webview/state.ts`](../../src/ui/webview/state.ts) 负责“历史回放时把结构还原回来”。

它做的事情不是“把 message.content 变成一整段字符串”，而是：

```text
input_text       -> text segment
input_reference  -> reference segment
input_image/file -> attachment placeholder text
```

于是 reload 后用户仍然看到 chip，而不是“app.ts:3-5”这类普通文本。

### 7.3 UI 渲染职责

1. [`../../gui/src/components/ReferenceChip.tsx`](../../gui/src/components/ReferenceChip.tsx)
   负责一个 chip 的图标、label、hover title、remove 按钮。
2. [`../../gui/src/components/Composer.tsx`](../../gui/src/components/Composer.tsx)
   负责可编辑态 chip。
3. [`../../gui/src/components/MessageBubble.tsx`](../../gui/src/components/MessageBubble.tsx)
   负责历史消息态 chip。

所以“待发送的 chip”和“历史回放的 chip”视觉一致，但数据来源不同：

```text
待发送：来自 TipTap doc
历史回放：来自 transcript content -> state.ts -> segments
```

---

## 8. 发给 LLM 时是什么样

### 8.1 原则

**对 LLM 来说，引用不是 UI 装饰，而是 prompt 正文的一部分。**

所以发送时必须保序。

### 8.2 Responses / Completions 的处理

后端主逻辑在 Rust 侧完成，但扩展必须理解它的后果：

```text
前端发：
  text + reference + text + reference

后端内部：
  InputText / InputReference / InputText / InputReference

最终：
  Responses API   -> flatten 成单个 input_text（真附件仍可单独保留）
  Completions API -> flatten 成单个 string（图片/PDF 这类真多模态仍拒绝）
```

也就是说：

1. **顺序不会丢**；
2. **UI chip 不会直接原样发给模型**；
3. **发给模型的是引用的标准文本投影**。

说人话：模型看到的是“带文件路径/行号的上下文正文”，不是“一个前端组件对象”。

---

## 9. 细节策略

### 9.1 选区 label 与 hover

规则：

```text
chip label   = basename:lineStart-lineEnd
hover title  = path:lineStart-lineEnd
```

例子：

```text
label = app.ts:12-18
title = src/app.ts:12-18
```

这样做的原因很简单：

1. label 要短，避免把 composer 撑爆；
2. hover 要完整，避免用户不知道它到底指向哪。

### 9.2 文件 label 与 hover

规则：

```text
chip label  -> 一律用文件名（目录保留尾部 /）
工作区内    -> hover / path 用相对路径
工作区外    -> hover / path 回落到绝对路径
```

### 9.3 大选区护栏

选区快照不是无限大。

[`../../src/ui/webview/contextReferences.ts`](../../src/ui/webview/contextReferences.ts) 对选区文本做截断，避免用户一次选几万行导致 prompt 爆炸。

这条规则是产品护栏，不是“怕麻烦”。

---

## 10. 验证闭环

### 10.1 单测

已覆盖的关键点：

1. `protocol.test.ts`：`segments` / `resolveDrop` / `insertReference` 结构校验；
2. `state.test.ts` / `dual_channel.test.ts`：历史消息中的 `input_reference` 回放成 segments；
3. `provider.test.ts`：`classifyPickedUri`、`pickContext` 选框参数、图片/PDF/目录/普通文件分流；
4. `webview_provider_flow.test.ts`：prompt `segments` 透传、`pickContext` 混选分流、drop 归一、manifest 合同；
5. `Composer.test.tsx`：引用插入去重、拖拽 URI 解析、`Shift` hint 三态、editor drop 抑制双处理；
5. `MessageBubble.test.tsx` / `ReferenceChip.test.tsx`：历史 chip 渲染与 hover title；
6. `App.test.tsx`：`insertReference` 事件进入 composer，reference-only prompt 可发送。

### 10.2 Host E2E

已新增两条真实宿主链路：

1. **选区命令链路**：真实编辑器选区 -> `tomcat.addSelectionToChat` -> webview chip -> 点击发送 -> reload 后历史回放仍是 chip；
2. **文件入口链路**：`showOpenDialog` stub 驱动 `pickContext` -> 图片进附件、代码/文件夹进 chip；
3. **文件拖拽链路**：`resolveDrop` 意图 -> 图片进附件、文件/目录进 chip，并验证重复 drop 不双插。

### 10.3 验收命令

开发期至少要看这几类信号：

```text
npm run lint
npm run test:unit
TOMCAT_E2E_GREP='editor selections|dropped file references' npm run test:e2e:webview-devhost
```

---

## 11. 风险与取舍

### 11.1 TipTap 体积

富输入框引入了更重的前端编辑器依赖，生产包体会比原来的 `<textarea>` 大。

这是有意识的 trade-off：

```text
换来的能力：
  内联原子 chip
  有序 segments
  删除 / 去重 / 回放一致

付出的代价：
  bundle 更大
  editor 测试需要额外 DOM 几何桩
```

这次选择的判断标准不是“最轻”，而是“能不能把语义做对”。

### 11.2 webview 拖拽有平台边界

这里的脏，不只是 MIME 杂，而是**平台直接拦事件/抹路径**：

1. `WebviewWindowDragMonitor` 会在非 `Shift` 拖拽时让 iframe `pointerEvents = none`；
2. 外部文件拖入 webview 时，沙箱不会把真实路径交给页面；
3. 所以我们必须把“拖拽解析”和“host 分类”拆开，并保留 `+` 作为稳定兜底入口。

### 11.3 Completions 仍非真多模态

这不是这次功能的 bug，而是模型通道能力边界。

本次保证的是：

1. 选区 / 文件引用的**文本语义**不会丢；
2. 真图片 / PDF 等附件能力仍按各自通道限制处理。

---

## 12. 一句话结论

这套方案把“引用”从一个 UI 小功能，真正落成了一个**跨 VS Code 入口、webview editor、host 协议、transcript 落盘、LLM payload** 的统一结构：

```text
用户操作 -> reference chip -> ordered segments -> transcript content -> LLM flatten
```

所以它不是“把文件路径塞进输入框”这么简单，而是让 Tomcat 第一次真正拥有了**可排序、可回放、可解释的上下文引用系统**。
