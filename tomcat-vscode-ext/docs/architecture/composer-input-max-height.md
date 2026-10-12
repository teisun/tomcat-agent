# Composer 输入框高度上限（按聊天栏比例封顶）

> 适用范围：`tomcat-vscode-ext` webview 底部 Composer（Chat / Plan 共用）的 TipTap 输入区，限制最大高度，超限后在输入框内滚动。
>
> 一句话定位：**输入框最高大约占「聊天栏」的 28%（略低于聊天栏高度的 1/3），窗口变大变小时比例不变；短内容仍用现有 `min-height: 84px`。**
>
> 单一事实源（实现时以这些为准）：
>
> 1. 布局壳：[`../../gui/src/App.tsx`](../../gui/src/App.tsx) 的 `.tc-shell` / `.tc-stream-shell` / `<Composer />`
> 2. 样式：[`../../gui/src/styles.css`](../../gui/src/styles.css) 的 `.tc-composer__editor`、`.tc-shell--question-pending .tc-composer__editor`
> 3. 编辑器：[`../../gui/src/components/Composer.tsx`](../../gui/src/components/Composer.tsx) — TipTap `editorProps.attributes.class = "tc-composer__editor"` + `data-testid="composer-input"`
>
> **本方案只改 CSS（最多加一条 container 声明），不改协议、不改 TipTap 扩展、不引入 ResizeObserver。**

---

## 全景（先看图）

```text
┌─ .tc-shell  (height:100%; 新建 container → 100cqh = 本栏高度) ─────────┐
│  SessionBar / Todo / 附件条 …（chrome，不参与「输入 vs 转录」心智）      │
│                                                                         │
│  ┌─ .tc-stream-shell  (flex:1; min-height:0) ─────────────────────────┐ │
│  │  转录区（被挤压时变矮）                                              │ │
│  └─────────────────────────────────────────────────────────────────────┘ │
│                                                                         │
│  ┌─ .tc-composer  (flex:0 0 auto) ────────────────────────────────────┐ │
│  │  notices / chips                                                    │ │
│  │  ┌─ .tc-composer__editor  (ProseMirror contenteditable) ─────────┐ │ │
│  │  │  min-height: 84px                                              │ │ │
│  │  │  max-height: 28cqh   ← 封顶：略低于 shell 的 1/3               │ │ │
│  │  │  overflow-y: auto    ← 超限在输入框内滚，caret 仍能 scrollIntoView │ │
│  │  └────────────────────────────────────────────────────────────────┘ │ │
│  │  工具条 Mode / Model / Send（不受 max-height 裁切）                 │ │
│  └─────────────────────────────────────────────────────────────────────┘ │
└─────────────────────────────────────────────────────────────────────────┘

比例心智（忽略 chrome 时）：
  设聊天栏可用高 = H，输入封顶高 = C
  想要 C / (H − C) 略小于 1/2  ⇒  C 略小于 H/3  ⇒  取 C ≈ 0.28H（28cqh）
```

---

## 1. 需求背景

用户希望 Composer 输入区表现接近 Cursor：输入框有一个**随面板高度变化的最大高度**，大约比「转录区高度」的一半再矮一点；窗口 / 侧栏拉高拉矮时这个比例保持稳定；内容超过上限时，**在输入框内部滚动**，而不是把转录区挤没。

现状（实现前核对过）：

| 选择器 | 现状 |
| --- | --- |
| `.tc-composer__editor` | `min-height: 84px`；**无** `max-height`；**无** `overflow-y` → 随内容无限长高 |
| `.tc-composer` | `flex: 0 0 auto` → 不收缩，转录区被挤 |
| `.tc-stream-shell` | `flex: 1 1 0; min-height: 0` → 正确吃剩余空间 |
| `.tc-shell--question-pending .tc-composer__editor` | 已有特例：`min-height: 3em; max-height: min(20vh, 12em); overflow-y: auto` |
| `Composer.tsx` | **无** `scrollHeight` autosize JS；高度靠 contentEditable 自然增高 |

Chat 与 Plan **共用同一个** `<Composer />`（`App.tsx`），因此一条 CSS 规则覆盖两种模式。

---

## 2. 解决方案

### 2.1 方案讲解

把问题想成量水杯：聊天栏是杯子总高度 `H`，输入框是底下那截 `C`，转录区是上面那截 `H−C`。用户要「下面那截略矮于上面那截的一半」，算一下就是 **`C` 略小于 `H/3`**。

普通 CSS 百分比 `max-height: 28%` **不能直接用**：百分比要相对「有确定高度的包含块」，而 `.tc-composer` / surface 都是 `height: auto`（跟着内容长），百分比会失效或算错。

因此：

1. 在已有确定高度的 `.tc-shell`（`height: 100%`）上声明 **CSS 容器查询**：`container-type: size`。
2. 输入框用 **`max-height: 28cqh`**（`cqh` = 该容器高度的 1%），窗口一变，上限跟着变，**零 JS**。
3. 同时加 **`overflow-y: auto`**，超限在 **`.tc-composer__editor`（ProseMirror 节点自己）** 内滚动——TipTap 把 `class` / `data-testid` 直接挂在 contentEditable 上，caret 的 scroll-into-view 找得到这个滚动祖先。

为什么不搞 ResizeObserver：本布局不需要量「转录区像素再反写 style」。容器查询直接表达「相对聊天壳的稳定比例」；VS Code Chat 用 JS 是因为 Monaco 必须吃显式像素高度，我们是 contentEditable，到 `max-height` 自然停再滚即可。

类比：OpenCode TUI 默认就是 `terminalHeight / 3`（见调研）；我们用 `28cqh` 做同一数学在 webview 里的 CSS 版，并略收一点以贴近「比一半转录再矮一点」。

### 2.2 UI 效果图

**短输入（默认）**

```text
Before / After 相同：
┌────────────────────────────┐
│ 转录 …                     │
│                            │
├────────────────────────────┤
│  [  空输入 / 一两行  ]     │  ← 高度 ≈ 84px（min-height）
│  Mode  Model        Send   │
└────────────────────────────┘
```

**长输入（封顶 + 内滚）**

```text
Before（坏）：输入无限长高，转录被挤成一条缝
┌────────────────────────────┐
│ 转录 …（几乎看不见）        │
├────────────────────────────┤
│  很长很长很长…             │
│  …继续长…                  │
│  …继续长…                  │
│  Mode  Model        Send   │
└────────────────────────────┘

After（好）：输入停在 ~28% shell，内部出现滚动条
┌────────────────────────────┐
│ 转录仍占大部分              │
│ …                          │
├────────────────────────────┤
│  很长很长…          ▲      │
│  …（内部可滚）…     █      │  ← max-height: 28cqh
│  …可见窗口…         ▼      │
│  Mode  Model        Send   │
└────────────────────────────┘
```

**窗口变矮 / 变高**

```text
矮窗口：28cqh 变小；若 28cqh < 84px，浏览器按规范把有效上限抬到 min-height（不塌）
高窗口：28cqh 变大，仍保持「输入 ≈ 壳高的 28%」
侧栏拉宽不影响高度公式（我们约束的是高度轴）
```

**待回答问题（question-pending）**

```text
壳上多了一块 pending 面板；输入框用更紧的上限：
  max-height: min(20vh, 12em, 28cqh)   ← 三者取更小，给问答面板让路
```

### 2.3 关键决策清单

1. **封顶公式取 `28cqh`（相对 `.tc-shell`），表达「略小于 H/3」。**
   <small>文件：`gui/src/styles.css` → `.tc-composer__editor`。改前：无 max-height。改后：`max-height: 28cqh`。边界：只限制编辑器，不含工具条 / notices；Chat+Plan 共用。</small>

2. **在 `.tc-shell` 上启用 `container-type: size`（可加 `container-name: tc-shell`），作为唯一高度参照。**
   <small>文件：`gui/src/styles.css` → `.tc-shell`。改前：仅 flex 列布局。改后：成为 size 容器，供 `cqh` 解析。边界：不新建 DOM 包裹层；不把参照改成 `vh` / 整屏。</small>

3. **超限滚动容器就是 `.tc-composer__editor` 自身：`overflow-y: auto`（建议顺带 `overscroll-behavior: contain`）。**
   <small>文件：`styles.css` + DOM 来自 `Composer.tsx` `editorProps.attributes`。改前：无 overflow。改后：ProseMirror 节点自滚。边界：不把 overflow 加在 `.tc-composer` / surface，避免 caret scroll-into-view 找错祖先。</small>

4. **保留 `min-height: 84px`；短内容不收缩。短窗口靠 CSS「min > max 时抬高 max」自然兜底，不写额外 JS。**
   <small>选择器：`.tc-composer__editor`。改前/改后 min-height 不变。边界：不把 min 改成随 cqh 浮动。</small>

5. **question-pending 与比例封顶用 `min()` 合成，取更紧者；保留更矮的 `min-height: 3em`。**
   <small>选择器：`.tc-shell--question-pending .tc-composer__editor`。改前：`max-height: min(20vh, 12em)`。改后：`max-height: min(20vh, 12em, 28cqh)`。边界：不删 pending 特例；不单独再引入一套 JS。</small>

6. **明确拒绝 ResizeObserver / 量转录区设 style；明确拒绝纯 `vh` 与无容器的 `%`。**
   <small>理由见 §2.4。若将来改成 Monaco 式必须像素布局，才允许推翻。</small>

7. **不改 `Composer.tsx` 逻辑、不改 App 结构、不改协议。**
   <small>范围：仅 `styles.css`（必要时一句注释）。测试可改 `Composer.test.tsx` / `App.test.tsx`。</small>

### 2.4 调研结论

至少查了 3 个同级实现 + 本机 Cursor 产物；结论如下。

| 参考 | 证据（路径 + 符号/选择器） | 相对什么封顶 | 超限是否内滚 | 对我们的含义 |
| --- | --- | --- | --- | --- |
| **OpenCode**（最同构数学） | `opencode/packages/tui/src/component/prompt/index.tsx` → `maxHeight` memo：`tuiConfig.prompt?.max_height ?? Math.max(6, Math.floor(dimensions().height / 3))`；配置 schema：`packages/tui/src/config/index.tsx` `prompt.max_height` | **终端高的 1/3**（可配置） | textarea 视口内滚 | **直接印证 C≈H/3**；我们用 `28cqh` 做「略小」版 |
| **Continue**（同为 TipTap webview） | `continue/gui/src/components/mainInput/TipTapEditor/TipTapEditor.tsx` → `EditorContent` `className={... max-h-[70vh] ...}` + `overflow-y-scroll` | **视口 `70vh`** | 是（包在 EditorContent） | **反例**：侧栏聊天用 `vh` 会跟整屏走，不跟面板走 → 我们不用 |
| **Cline** | `cline/apps/vscode/webview-ui/src/components/chat/ChatTextArea.tsx` → `<DynamicTextArea maxRows={10} minRows={3} />` | **固定约 10 行** | 是（textarea） | 行数封顶稳定但不跟面板比例；窗口很高时偏矮 |
| **VS Code Chat** | `vscode/.../chatInputPart.ts`：`INPUT_EDITOR_MAX_HEIGHT = 250`；`setMaxHeight` / `_effectiveInputEditorMaxHeight`；`chatWidget.ts` `layoutChatWidgetForInputHeight` | 默认 **固定 250px**，布局层可再灌预算 | Monaco 编辑器内滚 | 固定 px + 重布局 JS；Monaco 需要像素，我们不需要照搬 |
| **cc-fork-01（Claude Code UI）** | `cc-fork-01/src/components/FullscreenLayout.tsx`：底部槽 `maxHeight="50%"`；`PromptInput.tsx` 注释 `Bottom slot has maxHeight="50%"` | **整屏底部槽 50%**（含 footer） | TUI/Ink 视口 | 「半屏给底栏」≠「输入 / 转录 = 1/2」；我们按用户公式用 ~1/3 |
| **Codex TUI** | `codex-rs/tui/src/bottom_pane/chat_composer.rs` `desired_height*`；`textarea.rs` `desired_height` = 折行行数 | 随内容要高，由底栏布局吃掉 | 有滚动状态 | 终端布局，无 CSS 面板比例，仅作对照 |
| **Cursor（本机 app）** | `Cursor.app/.../workbench.desktop.main.js`：默认 `--prompt-input-editor-max-height: 200px`；`.ui-prompt-input-editor__content{max-height:var(--prompt-input-editor-max-height)}`；可把 CSS 变量设成动态 px | 默认 **固定 200px**，可被 JS 覆盖 | ProseMirror / content 上有 max-height | 用户要的是「像 Cursor 那样有封顶且可读」，不是照抄 200px；**比例语义以 OpenCode / 用户公式为准** |

**反例汇总（不要学）：**

- Continue 的 `max-h-[70vh]` —— 侧栏场景比例漂移。
- 无 `container-type` 的 `max-height: 28%` —— 包含块高度不定，不可靠。
- Cline 纯 `maxRows` —— 不随面板变高变矮。
- 为 contentEditable 强行上 ResizeObserver —— 重复发明 cqh，还要防布局抖动。

**何时应推翻本决策：**

1. **目标比例改成「输入 ≈ 转录的一半」且必须精确扣除 SessionBar / Todo / 附件条**，而实机 QA 证明相对整个 `.tc-shell` 的 `28cqh` 视觉偏差过大 → 改为在「stream + composer」外包一层仅含二者的 body 容器再挂 `cqh`，或改用极薄的 layout 测量（仍优先 CSS）。
2. **引擎不支持 container query height**（当前 `engines.vscode: ^1.104.0`，Chromium 足够；若支持矩阵下沉到极老宿主）→ 再考虑 CSS 变量 + 一次 `ResizeObserver` 写 `--tc-shell-height`。
3. **输入控件换成 Monaco / 必须显式像素高度的编辑器** → 改走 VS Code `setMaxHeight` 路线。
4. **产品改口要固定 px（如 Cursor 默认 200px）或固定行数** → 删 cqh，改常量。

---

## 3. 测试用例清单

设计原则：这是 **CSS 封顶**，优先扩展现有 GUI 单测（`getComputedStyle` + 固定壳高），不上大型 E2E。

### 用例设计

1. **短输入保持 min-height**
   空或一两行：`.tc-composer__editor` 计算高度 ≥ 84px，且无明显内部滚动（`scrollHeight <= clientHeight` 或接近）。

2. **长输入封顶并内滚**
   向 `data-testid="composer-input"` 写入很多段落后：`getComputedStyle(editor).maxHeight` 解析为有限值；`overflowY` 为 `auto`/`scroll`；`scrollHeight > clientHeight`；壳上转录区 `.tc-stream-shell` 仍有可观高度（不被吃光）。

3. **改壳高时比例跟随**
   给 `.tc-shell` 设两种明确像素高度（例如 600px / 900px），长输入下 `editor.clientHeight` 约等于 `0.28 * shellClientHeight`（允许小数与 padding 误差，例如 ±4px 或相对误差 &lt; 5%）。

4. **question-pending 取更紧上限**
   挂上 `.tc-shell--question-pending` 后，`max-height` 等于 `min(20vh, 12em, 28cqh)` 的计算结果（可用 `getComputedStyle` 断言不超过 `12em` 与 `28cqh` 中较小者）；`min-height` 仍为 pending 的 `3em` 语义（或计算值不大于常态 84px 策略——以 CSS 最终值为准）。

5. **Chat / Plan 同一规则**
   不新增第二套 Composer；现有模式切换单测下，编辑器节点仍是同一个 `.tc-composer__editor` 选择器（断言 class / testid 即可，无需两套高度逻辑）。

6. **回归：不破坏 pending 类名切换**
   保留 / 顺手对齐 `App.test.tsx` 里已有 `.tc-shell--question-pending` 挂载与卸载断言。

### 测试文件路径

| 动作 | 路径 |
| --- | --- |
| **新增或扩展** | `tomcat-vscode-ext/gui/src/components/Composer.test.tsx` — 长文本 + `getComputedStyle` / scroll 断言（组件级最合适） |
| **扩展** | `tomcat-vscode-ext/gui/src/App.test.tsx` — 壳高变化或 pending 合成（若组件测不便挂完整 `.tc-shell`） |
| **不新增** | 不为该 CSS 单独开 VSIX E2E；手动侧栏拉高拉矮目视一次即可 |

实现提示：jsdom 对 `cqh` 支持可能不完整。若单测环境算不出 cqh，可：

- 在测试里给 `.tc-shell` 注入明确 `height` 并 mock / 跳过精确像素，改为断言 **样式表规则存在**（读取 `styles.css` 或检查 `overflow-y` / class 契约）；或
- 用 Playwright/webview 轻量验收（仅当团队已有同类 CSS 测法）。

以仓库现有 `Composer.test.tsx` / `App.test.tsx` 风格为准，避免新测试框架。

---

## 4. 建议落地的具体 CSS（实现时照抄后微调注释）

```css
/* gui/src/styles.css */

.tc-shell {
  /* 已有：display:flex; flex-direction:column; height:100%; ... */
  container-type: size;
  container-name: tc-shell;
}

.tc-composer__editor {
  /* 已有：min-height: 84px; ... */
  max-height: 28cqh; /* 略小于 H/3，使 C/(H−C) 略小于 1/2 */
  overflow-y: auto;
  overscroll-behavior: contain;
}

.tc-shell--question-pending .tc-composer__editor {
  min-height: 3em;
  max-height: min(20vh, 12em, 28cqh); /* 更紧者优先，给问答面板让路 */
  overflow-y: auto;
}
```

### 明确不改

- `Composer.tsx` 的 TipTap 配置、提交/草稿逻辑
- `.tc-composer` 的 `flex: 0 0 auto`（封顶后自然不再无限长高）
- `.tc-stream-shell` 的 flex 吃剩余空间
- 协议 / Rust / host

### 实现顺序建议

1. 改 `styles.css`（shell container + editor max-height/overflow + pending `min()`）
2. 本地侧栏：短输入 / 贴长文 / 拉高拉矮 / 触发 pending 各看一眼
3. 补 `Composer.test.tsx`（能测多少测多少）
4. 若 QA 觉得 28% 略矮或略高，只改一个数字（`28cqh` ↔ `30cqh`），不动结构

---

## 5. 给实现者的不确定项（需人眼确认）

1. **`28cqh` vs `30cqh`**：数学上 30%≈H/3，28% 更贴「略小于一半转录」；以实机侧栏观感二选一，不要同时引入两套机制。
2. **参照是整个 `.tc-shell` 还是「stream+composer」子树**：本方案选壳（零 DOM 改动）。若 SessionBar + Todo + 附件条很高，觉得输入相对转录偏矮，再加 body 包裹层。
3. **jsdom 是否能测 cqh**：测不动就退化为契约断言 + 一次手动 QA，不要为测而引入 ResizeObserver 污染产品代码。
