| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| tomcat | 2026-07-18 16:28 +0800 | DONE | feature/transcript-rich-render | — |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** TranscriptView 富渲染落地：assistant 正文走 `ChatMarkdown`（`marked + DOMPurify`），多行代码/ASCII 围栏裱成代码卡片，围栏标题与正文 `` `path:line` `` 可点击打开并定位行；`openFile` 协议新增可选 `line`，贯穿到 `VsCodeIde.showFile(path, line?)`；Rust system prompt 新增 `SystemOutputConventions`，约束模型稳定产出可点击格式。@2026-07-18
- [✓] **[P0]** 真机闪烁/缺卡片整改收口：根因是先画半成品 HTML、再在 `useEffect` 里改真实 DOM，React 重设 `innerHTML` 会抹掉卡片/path chip。现改为在 `useMemo` 里离屏烘焙成品结构（`closeOpenFence -> marked -> DOMPurify -> detached div -> decorate/linkify -> innerHTML`）；`highlight.js`/`mermaid` 留在 `useEffect` 做幂等纯增强。thinking 回退为弱化纯文本 `<pre>`（不解析 markdown、不生成 thinking 内 clickable path）；保留 ToolRow `FileChip` 与助手正文点击打开。@2026-07-18
- [✓] **[P0]** 回归门禁：GUI focused（首帧即有 code-card/copy/clickable-path；thinking 为 `<pre>`）+ host E2E `assertTranscriptRichRenderingFlow`（copy、两帧 DOM 稳定、点击 openFile、thinking 纯文本边界）+ `npm run lint` / `test:unit` / 全量 `test:e2e:vscode-devhost` / Rust prompt focused / `package:vsix` 全绿。@2026-07-18

### 🔌 INTERFACE (接口变更)
- 前端新增 `ChatMarkdown` / `markdownRuntime` / `codeFence` / `inlinePath`；assistant 正文富渲染；thinking 为弱化纯文本 `<pre>`。
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
