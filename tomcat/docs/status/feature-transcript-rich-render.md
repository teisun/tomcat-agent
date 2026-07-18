| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| tomcat | 2026-07-18 19:21 +0800 | DONE | feature/transcript-rich-render | — |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** TranscriptView 富渲染二次整改：聊天里“完全无高亮 / 无 mermaid”的真根因不是 `highlight.js` 主题或 markdown 结构，而是**聊天 webview 的 CSP 比计划预览少了 `'strict-dynamic'` 与 `'unsafe-inline'`**；`ChatMarkdown` / `markdownRuntime` 里的 lazy `import()` 分包（`highlight.js` core/语言包与 `mermaid`）在 transcript 中被浏览器拦下，所以卡片结构能出来，但颜色和 SVG 永远出不来。现已把聊天 CSP 对齐到计划预览：放行动态分包与 mermaid 内联样式；同时把代码块 UI 收到最简：**不再显示语言标签**，只有“带文件路径的围栏”才渲染头部（文件图标 + basename，hover/title=完整相对路径，点击打开定位行，右侧纯图标 copy），无路径围栏统一为 `bare` 块（无头部，copy 浮在内容区右上角）；正文内联路径显示 basename；`openFile` 失败改为 toast，不再向 transcript 追加 error 红条。门禁同步补强：GUI/provider focused tests 覆盖 basename/title/copy-feedback/toast，host E2E `assertTranscriptRichRenderingFlow` 升级为断言真实 `hljs-*` token span、mermaid `<svg>`、`bare` 块与坏链点击不污染 transcript。@2026-07-18
- [✓] **[P0]** TranscriptView 富渲染落地：assistant 正文走 `ChatMarkdown`（`marked + DOMPurify`），多行代码/ASCII 围栏裱成代码卡片，围栏标题与正文 `` `path:line` `` 可点击打开并定位行；`openFile` 协议新增可选 `line`，贯穿到 `VsCodeIde.showFile(path, line?)`；Rust system prompt 新增 `SystemOutputConventions`，约束模型稳定产出可点击格式。@2026-07-18
- [✓] **[P0]** 真机闪烁/缺卡片整改收口：根因是先画半成品 HTML、再在 `useEffect` 里改真实 DOM，React 重设 `innerHTML` 会抹掉卡片/path chip。现改为在 `useMemo` 里离屏烘焙成品结构（`closeOpenFence -> marked -> DOMPurify -> detached div -> decorate/linkify -> innerHTML`）；`highlight.js`/`mermaid` 留在 `useEffect` 做幂等纯增强。thinking 回退为弱化纯文本 `<pre>`（不解析 markdown、不生成 thinking 内 clickable path）；保留 ToolRow `FileChip` 与助手正文点击打开。@2026-07-18
- [✓] **[P0]** 回归门禁：GUI focused（首帧即有 code-card/copy/clickable-path；thinking 为 `<pre>`）+ host E2E `assertTranscriptRichRenderingFlow`（copy、两帧 DOM 稳定、点击 openFile、thinking 纯文本边界）+ `npm run lint` / `test:unit` / 全量 `test:e2e:vscode-devhost` / Rust prompt focused / `package:vsix` 全绿。@2026-07-18

### 🔌 INTERFACE (接口变更)
- 前端新增 `ChatMarkdown` / `markdownRuntime` / `codeFence` / `inlinePath`；assistant 正文富渲染；**代码围栏不再显示语言标签**：有文件路径时显示 basename 头部并可点击打开，无路径时为 `bare` 块 + 内容区右上角浮动 copy；正文内联路径显示 basename，`title` 保留完整相对路径；thinking 为弱化纯文本 `<pre>`。
- `openFile.data.line?: number`；`VsCodeIde.showFile(path, line?)` → `selection + revealRange`。
- `PromptKey::SystemOutputConventions` 插入系统提示词链（`ToolInstructions(20) -> OutputConventions(21) -> ParallelTools(22)`）。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |

### 集成说明
- 本分支任务已收口，State=DONE，准备合入 `develop`。
- 验证摘要：`npm run lint`、`npm run test:unit`（ext+gui）、Rust prompt focused（output_conventions / load order / prompt_size_budget）、全量 `npm run test:e2e:vscode-devhost`、`npm run package:vsix` 通过。
- 相关历史叙事亦回填 `docs/status/develop.md`；旧分支文档 `feature-transcript-ui-and-checkpoints.md` 同步更正“thinking 也走 ChatMarkdown”的过时口径。
