# Tomcat VSCode 扩展 · Phase 2 · 03 Stage B：自建 React Webview 富前端

> 总览见 [`../tomcat-vscode-extension-phase2.md`](../tomcat-vscode-extension-phase2.md)（含定位、阅读顺序与文首导图集）。
> 本文对应 [`ARCHITECTURE_SPEC.md`](../../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) 的 **§3 落地选型与实施**（Stage B 部分）：§3.1 七列决策表（SB1–SB8）+ §3.2 五列实施点表。
> 竞品实证见 [`01-scope-and-research.md`](01-scope-and-research.md) §2.3；webview/serve 协议字段表见 [`04-protocol-runtime.md`](04-protocol-runtime.md) §4。
> 单一事实源：桥接核心 `tomcat-vscode-ext/src/serveClient/TomcatMessenger.ts`（Phase 1，**不改**）；IDE 能力 `tomcat-vscode-ext/src/ide/VsCodeIde.ts`（Phase 1，复用）；serve 协议 `tomcat/src/api/serve/types.rs`。
> 外部参考仓库（位于 `/Users/yankeben/workspace/`，仅作证据引用）：`cline/`、`continue/`。

---

## 3. Stage B：在同一桥接核心上自建 Webview，并与 participant 并存

> 专业：Stage B 是「纯前端增量」阶段。新增层 1-B（webview 前端：宿主 provider + typed 协议 + React GUI），复用层 2 桥接核心与 Stage A 落地的 serve 命令；participant 与 webview 默认并存，**共享同一项目 scope 会话池**（复用 `tomcat code` 的归组/last-active），单条 live 会话单前端归属。
> 说人话：底层一行不改，只多画一套漂亮 UI；它和原生聊天同时在线，看的是同一份项目会话，默认接着上次那条。

### 3.0 Stage B 一图概览（含并存）

```text
        ┌─────────────────────── 扩展进程（Node/TS）───────────────────────┐
        │ 前端 A：@tomcat participant       前端 B：自建 Webview            │
        │   (Phase1 + Stage A slash)          ui/webview/provider.ts (CSP)  │
        │        │                            ui/webview/protocol.ts        │
        │        │                            gui/ (React + Vite)           │
        │     owner: thread→sid          owner: tab→sid                  │
        │        └────────────┬───────────────┘                            │
        │            sessionId→owner(frontend) 归属表（防双驱动）           │
        │                     ▼                                            │
        │        serveClient/TomcatMessenger.ts（Phase1 桥接核心，单实例）  │
        └─────────────────────┬────────────────────────────────────────────┘
                              │ 单个 tomcat serve --stdio（并发多会话）
                              ▼
        tomcat serve：同一项目 scope 会话池（sessions.json 按 session_key 归组）
          list_sessions(scope=disk) → [sid1*,sid2,sid3]（*=current/last-active）
          两前端枚举同一份；激活某 sid 成 live slot 后单前端归属，另一前端只读/提示
```

> 导读：关键有两条——**两个前端共用一个 `TomcatMessenger` 单实例 + 一个 serve 进程**，且**看的是同一项目 scope 会话池**（与 `tomcat code` 共写 `sessions.json`）。区别于"各搞各的"：历史**枚举共享**、默认指向 last-active；但同一条会话一旦被某前端**激活成 live**，归属该前端（serve `slot.is_busy` 硬保护 + 扩展侧归属表软约束），另一前端对它只读或提示冲突，避免双驱动抢 busy/turn。

### 3.1 决策表（SB1–SB8）

> 列含义：维度｜关切｜决策｜取自（本仓 + 外部证据）｜入选理由｜未入选 + 拒因｜说人话。

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|----------------|--------|
| **SB1** 前端形态 | 富交互前端怎么搭 | 自建 **React Webview**：`contributes.views` + `WebviewViewProvider`（稳定）；VSCode 只提供宿主 API，不提供 UI 框架 | 本仓 Phase 1 `ide/VsCodeIde.ts` 可复用；cline `WebviewProvider.ts`（注入+CSP）、continue `extensions/vscode/src/webviewProtocol.ts` | 稳定可上架；UI 完全自控；桥接核心复用；与 `cline` / `continue` 的成熟路径同构 | ① fork `vscode-copilot-chat`：体量/许可/商标 + 招牌 UI 靠 proposed + 身份门禁（Phase 1 §10 已否决）→ 拒；② 继续堆原生 part：富交互（内联 diff/思考块/多会话 tab）表达力不足 → 拒 | VSCode 只给容器，不送框架；所以我们自己在 Webview 里跑 React。 |
| **SB2** 与 participant 关系 | 两个前端如何共处、会话怎么分 | **默认并存**；共享桥接核心 + 单 serve；**共享同一项目 scope 会话池**（枚举共享、默认 last-active），单条 live 会话单前端归属；`tomcat.ui` 可选关其一 | 用户已确认 both_default + shared_pool；本仓 `TomcatMessenger`（单实例多会话）、`scope.rs:49`/`session_impl.rs:380`（scope 归组+列举）、serve `registry`（并发会话+`is_busy`） | 不浪费 Phase 1 成果；复刻 `tomcat code` 项目归组 UX；后端原生并发会话；`is_busy` 兜底防抢占 | ① 二选一（webview 取代 participant）：丢已交付原生入口 → 拒；② 完全隔离命名空间（各自 new_session、互不可见）：webview 看不到项目历史，丢用户要的归组 → 拒；③ 两前端同时驱动同一 live 会话：抢 busy/turn → 拒，故单活跃归属 | 两入口都留、看同一份项目会话；同一条对话不许俩面板一起发。 |
| **SB3** webview↔host 协议 | 消息通道用什么格式 | typed postMessage `{messageId,done,content}`（学 continue），按需流式 | continue `webviewProtocol.ts`、`IpcMessenger.ts:37`（`{done}` 流）；cline ProtoBus 对照 | 与 serve NDJSON「一问一流答」同构；依赖最小；TS 类型直通 | cline 的 gRPC-over-postMessage（proto 编译 + ProtoBus）：引入 proto 工具链与代码生成，对单扩展偏重 → 拒 | 用最轻的"带 id 的消息+流式分块"，不上 gRPC 那套。 |
| **SB4** 渲染数据流 | 流式如何高效渲染 + 保证与 vscode chat 同款富 UI | **双通道**：全量 `state` 快照 + `event` 透传（**原样携带 Phase 1 `WireEvent`**，含 `thinking_delta`/`tool`/`ask_question`） | cline `state.proto`(subscribeToState) + `ui.proto`(`ClineMessage.partial`)、`task/index.ts`(sendPartialMessage)；本仓直接复用 Phase 1 事件词汇 | 复用 participant 同源事件 → 思考/工具/审批/diff 体验等价；只 patch 不重画；背压只丢可重建 delta | 另造一套 UI 语义：重复造轮子且与 participant 漂移 → 拒；每次全量重渲染：大会话卡顿 → 拒 | 一条管整体快照（重连用），一条把后端事件原样传进来逐条画，所以富 UI 全有。 |
| **SB5** CSP / 资源加载 | webview 安全与静态资源 | nonce + `asWebviewUri` + `localResourceRoots` + `retainContextWhenHidden` | cline `WebviewProvider.ts:101`(getNonce)、`:113`(CSP `script-src 'nonce-…'`) | 满足 webview 安全基线；隐藏不丢状态；与 cline 生产写法一致 | 放开 CSP（`unsafe-inline`/无 nonce）：安全审查不过、上架风险 → 拒 | 按 cline 那套安全头加载脚本，藏起来也不丢状态。 |
| **SB6** diff / 编辑落地 | webview 里"看 diff + 改文件" | 复用 Phase 1 `ide/VsCodeIde.ts`：虚拟只读文档 + `vscode.diff` + `WorkspaceEdit` + 装饰；可选 continue 垂直流式 diff | 本仓 `ide/VsCodeIde.ts`（Phase 1 R5 已实现）；cline `VscodeDiffViewProvider.ts`；continue `VerticalDiffManager` | "看 diff"与"真改"同一份改动；Phase 1 已测；不重写 | webview 内自渲 diff 再回写：绕开 VSCode 编辑栈、与编辑器状态割裂 → 拒 | 改文件还走 Phase 1 那套稳的，webview 只触发它。 |
| **SB7** 富 UI 控件 | 模型/plan/多会话怎么呈现 | 模型下拉(`set_model`) + plan 开关(`set_plan_mode`) + **多会话 tab**(`new/switch/close_session`) | 本仓 serve `types.rs`(new/switch/close_session 现成)、Stage A `set_plan_mode`/`list_models`；cline `togglePlanActMode`(单 task)、continue `ModeSelect.tsx` | 复用 Stage A 协议；多会话是 Tomcat 相对 cline 的优势 | 照搬 cline 单活跃 task：放弃 Tomcat 并发多会话能力 → 拒 | 顶部下拉选模型、开关切 plan、多个标签开多个会话。 |
| **SB8** 前端构建 / 打包 | React 产物如何进 VSIX | **React+Vite** 独立构建，产物纳入 VSIX；更新打包暂存清单 | 本仓 `scripts/package-vsix.ts`（Phase 1 暂存式打包）、`.vscodeignore`；`cline` `webview-ui/vite.config.ts`、`continue` `gui/vite.config.ts` | 与 cline/continue 同款（gui 独立构建）；打包脚本已是暂存式，扩展成本低 | 把 gui 源码塞进扩展 tsconfig 一起编：bundler/CSS/JSX 配置打架 → 拒 | gui 单独用 Vite 打包，产物拷进扩展包里。 |

### 3.1.1 补充裁决：Webview 应用框架

> 专业：VSCode 对第三方扩展提供的是 **Webview 宿主 API**，例如 `WebviewViewProvider`、`postMessage`、`asWebviewUri`、`localResourceRoots`、CSP/nonce、`retainContextWhenHidden`。这些 API 解决的是"如何把一个前端页面安全嵌进 VSCode 并与扩展宿主通信"，**不提供 React/Vue/Svelte 之类 UI 框架**。因此 Stage B 的页面框架、构建工具与前端目录布局必须由 Tomcat 自行选型。本次裁决结合本仓 `SB1/SB3/SB8` 约束，以及 `cline` / `continue` 两个成熟 VSCode Webview 项目的实证。
> 说人话：VSCode 只给我们一个"能装网页的容器 + 一条通信管道"，不会送一个官方前端框架。Tomcat 得自己决定这个网页是用 React 还是别的来写。

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|----------------|--------|
| **FW1** Webview 页面框架 | Webview UI 用什么写；VSCode 有没有自带框架 | **Tomcat Stage B 采 `React + Vite` 独立 GUI sidecar；扩展宿主继续走现有 TS 构建与 `WebviewViewProvider`。开发实现直接参考 `cline` 的 provider/CSP/gui 构建方式与 `continue` 的 typed `postMessage` / gui-host 拆分；不走 Electron 独立桌面壳。** | `cline/apps/vscode/webview-ui/package.json`、`cline/apps/vscode/webview-ui/vite.config.ts`、`cline/apps/vscode/src/hosts/vscode/VscodeWebviewProvider.ts`；`continue/gui/package.json`、`continue/gui/vite.config.ts`、`continue/extensions/vscode/src/ContinueGUIWebviewViewProvider.ts`、`continue/extensions/vscode/src/webviewProtocol.ts` | `cline` 与 `continue` 都验证了"VSCode Webview + React + Vite + 独立 gui 包 + host provider 注入资源"这条路线；与本方案已拍板的 typed postMessage、CSP、安全装载、打包入 VSIX 完全同构；React 最适合消息流、工具卡、审批卡、会话 tab 这类状态化 UI | ① **Electron / 独立桌面壳**：我们做的是 VSCode 扩展，不是独立 App；额外引入 Electron 只会增加进程、打包、验收与调试复杂度，且与 `contributes.views` 形态不匹配 → 拒。② **纯 HTML + 原生 JS 不上框架**：初期能跑，但 `thinking` / `tool` / `approval` / `multisession` 状态管理、组件复用和 UI 自动化成本更高 → 拒。③ **Vue / Svelte 等其他框架**：技术上可行，但当前缺少本仓既有经验与直接可复用的竞品实证，收益不明显 → 本期不选。 | 不是"VSCode 给了一个 React 框架"，而是"VSCode 给了一个 iframe 容器，我们自己在里面跑 React 应用"；`cline` 和 `continue` 都是这么做的，所以 Tomcat 直接走同一路线最稳。 |

开发参考边界：

- **参考 `cline`**：`WebviewProvider` 的 HTML 注入、CSP/nonce、`retainContextWhenHidden`、`gui/` 独立 Vite 构建、打包后资源注入方式。
- **参考 `continue`**：typed `postMessage` / `webviewProtocol`、`IdeMessenger` 风格的宿主-前端通信、`gui/` 与扩展宿主拆分、开发态/打包态资源切换。
- **明确不照搬**：`cline` 的 gRPC/proto 总线、额外 Electron 壳、以及与本仓阶段边界不匹配的重型状态层。

> 说人话：框架这件事就别再犹豫了，结论就是"**Webview 用 React + Vite，宿主继续是 VSCode 扩展**"。后面写代码时，provider/CSP/资源加载学 `cline`，协议和 gui/host 拆分学 `continue`，但不要把它们的重型通信栈和额外壳子一股脑搬进来。

### 3.2 实施点（五列）

> 列含义：实施点｜交付范围｜主要代码落点（扩展侧 TS）｜验收锚点（→ [`04-protocol-runtime.md`](04-protocol-runtime.md) §8）｜说人话。

| 实施点 | 交付范围 | 主要代码落点 | 验收锚点 | 说人话 |
|--------|----------|--------------|----------|--------|
| **E-B1** webview 宿主 | 侧栏视图注册 + HTML 注入 + CSP nonce + 资源 URI | `package.json` `contributes.views`；新增 `src/ui/webview/provider.ts`（`WebviewViewProvider`，学 cline `WebviewProvider.ts`） | `T2B-WEBVIEW-E2E` | 把自建面板挂进侧栏，能安全加载脚本。 |
| **E-B2** 宿主协议 | typed postMessage `{messageId,done,content}` 编解码 + 路由 | 新增 `src/ui/webview/protocol.ts`（学 continue `webviewProtocol.ts`） | `T2B-PROTO-UNIT` | 面板和插件之间收发消息的那层。 |
| **E-B3** React GUI | 聊天流渲染 + 工具卡 + 审批 + 思考块 | 新增 `gui/`（React + Vite，独立 `package.json`/`tsconfig`） | `T2B-GUI-UNIT` | 真正的界面：消息、工具、审批长这样。 |
| **E-B4** 双通道适配 | 桥接事件 → webview 帧：重连给 `state` 快照 + 逐条 `event` 透传 Phase 1 `WireEvent`（含 thinking/tool/审批） | `src/ui/webview/*`（订阅 `messenger.onEvent` 原样转 `event`；初始化/切换/重连时聚合 `state`） | `T2B-STREAM-UNIT` | 重连给整份快照，平时把后端事件原样传给界面（富 UI 同源）。 |
| **E-B5** diff 复用 | webview 触发"看 diff/应用编辑"走 Phase 1 IDE | 复用 `src/ide/VsCodeIde.ts`（**不改**）；webview 仅发意图帧 | `T2B-DIFF-E2E` | 面板上点"应用"，底层还是 Phase 1 那套改文件。 |
| **E-B6** 富 UI 控件 | 模型下拉 / plan 开关 / 多会话 tab | `gui/` 组件 + `protocol.ts` → `set_model`/`set_plan_mode`/`new\|switch\|close_session` | `T2B-MULTISESSION-E2E` | 顶部三件套：选模型、切 plan、开多个会话。 |
| **E-B7** 会话池与归属 | webview 启动拉项目 scope 历史(`list_sessions{scope:"disk"}`)渲染 tab、默认选 last-active(`isCurrent`)；激活会话登记 owner，非 owner 只读/提示 | `src/ui/webview/*` + `sessionId→owner(frontend)` 归属表；约定见 [`04`](04-protocol-runtime.md) §6 | `T2B-OWNERSHIP-INT` | webview 列本项目历史、默认接上次那条；同一会话不许俩面板一起发。 |
| **E-B8** 打包 | gui 产物纳入 VSIX；CSP 用打包后资源；`engines.vscode` 维持稳定基线 | `scripts/package-vsix.ts`（暂存清单 += `gui/dist`）、`.vscodeignore` | `T2B-PKG-SMOKE` | 把界面产物打进扩展包，装上能直接用。 |
| **E-B0** 桥接复用（零改动验证） | `TomcatMessenger` 同时服务两前端，核心不改 | `src/serveClient/TomcatMessenger.ts`（仅 Stage A 已加的 `sendSetPlanMode`/`sendListModels`） | `T2B-BRIDGE-REUSE` | 证明底层真没改、两个前端共用。 |

---

## 3.3 分层与复用（不动桥接核心）

> 专业：沿用 Phase 1 三层 + Continue 三层映射。Stage B 只在「层 1」增 webview 子层，「层 2」桥接核心保持 UI 无关，「层 3」serve 复用 Stage A 落地的命令。
> 说人话：新代码全在 UI 层，越往下越不动。

```text
层 1 UI            participant（A，已有）        webview（B，新增 ui/webview/* + gui/）
                        ╲                          ╱
层 2 桥接核心            TomcatMessenger（Phase 1，单实例，UI 无关，不改）
                                   │
层 3 Tomcat serve      Stage A 落地命令（set_plan_mode/list_models/get_state.planState/plan.*）
```

新增文件（扩展侧）：

- `src/ui/webview/provider.ts`：`WebviewViewProvider` 实现；`resolveWebviewView` 里设 `webview.options.localResourceRoots`、`enableScripts`、`retainContextWhenHidden`，注入带 nonce 的 HTML（学 cline `WebviewProvider.ts:95/113`）。
- `src/ui/webview/protocol.ts`：typed postMessage 协议（学 continue `webviewProtocol.ts`），帧形态见 [`04`](04-protocol-runtime.md) §4。
- `gui/`：React + Vite 独立工程，产物 `gui/dist` 由 `asWebviewUri` 引用。

## 3.4 双通道 postMessage 协议（全量 state 快照 + Phase 1 事件透传）

> 专业：webview 渲染采两条通道——`state`（全量视图快照：初始化/会话切换/重连时幂等覆盖）与 `event`（**原样透传 Phase 1 `WireEvent`**，逐条增量渲染）。**不另造 UI 语义**：`event` 通道携带与 participant `render.ts` 同一套事件词汇（含 `thinking_delta`/`tool_execution_*`/`ask_question`/`plan.*`），故 webview 与 vscode chat **同等富 UI 体验**（思考块/工具卡/审批卡/diff/子代理/用量）。背压时只丢可重建的 delta 帧（`message_update`），`state`/生命周期/审批帧必达（对齐 Phase 1 R9）。字段表与样例见 [`04-protocol-runtime.md`](04-protocol-runtime.md) §4.4。
> 说人话：一条管"整体应该长啥样"（重连用），一条把后端事件**原样**传进来逐条画；因为用的就是原生聊天那套事件，所以思考块/工具卡/审批卡都一样有。卡的时候只丢中间字，不丢结果。

```text
serve 事件（透传给 webview content.type）   webview 通道   渲染动作（同 participant 的 UI 元素）
──────────────────────────────────────────────────────────────────────────────────
message_start / message_end                event         新建/收尾消息气泡
message_update kind:content_delta          event         正文气泡 append
message_update kind:thinking_delta         event         折叠思考块 append（vscode chat 同款）
tool_execution_start/update/end            event         工具卡状态机推进；display.file→diff 入口
control_request{ask_question}              event         弹审批卡（答复经意图通道回 control_response）
plan.* (Stage A)                           event         plan 徽标/计划面板刷新
context_metrics_update / sub_agent_*       event         用量徽标 / 子代理块
agent_end / agent_interrupted / llm_error  event         收尾 / 中断 / 错误气泡
（初始化·会话切换·重连）                    state         整份会话视图快照，一次性覆盖后再续 event
llm_notice{backpressure}                   event         仅提示"渲染跟不上"
```

## 3.5 IDE 抽象与 diff（复用 Phase 1）

- webview **不**自渲 diff；点"查看/应用"时发意图帧给宿主，宿主调用 Phase 1 `ide/VsCodeIde.ts` 的 `vscode.diff`(虚拟只读文档) + `WorkspaceEdit`，与 participant 完全同一条编辑落地路径（Phase 1 R5）。
- 可选增强：参考 continue `VerticalDiffManager` 的"垂直流式 diff + CodeLens accept/reject"，作为 webview 的可选视图；不替代默认 `vscode.diff` 路径。

## 3.6 富 UI：模型 / plan / 多会话

| 控件 | 行为 | 走的协议 | 相对 cline/continue | 说人话 |
|------|------|----------|----------------------|--------|
| 模型下拉 | 列模型 + 切换 + 标当前 | `list_models`→`set_model`→`get_state.model`（Stage A） | 同 continue 模型选择 | 顶部选模型。 |
| plan 开关 | enter/exit/build + 计划面板 | `set_plan_mode`（Stage A）+ `plan.*` 事件 + `get_state.planState` | 比 cline Plan/Act 更细（5 态） | 一键进/出/开跑计划。 |
| 多会话 tab（项目 scope 池） | 启动列本项目历史 + 默认 last-active；开/切/关并发会话 | `list_sessions{scope:"disk"}`(Stage A SA8)→`switch_session`(可切磁盘历史)/`new_session`/`close_session` | **优于 cline 单 task**；复刻 `tomcat code` 归组/恢复 | 一打开就看到本项目历史对话、默认接上次那条，还能多开。 |

> 说人话：多会话是 Tomcat 的独门优势——serve `registry` 本就并发持有多会话（Phase 1 已用），webview 做成多 tab，cline 单活跃 task 做不到。Stage B 在此之上**复刻 `tomcat code` 的"按 git 项目根归组 + 默认恢复 last-active"**：webview 启动调 `list_sessions{scope:"disk"}` 拉同一项目 scope 历史（与 `tomcat code` 共写一份 `sessions.json`），默认选中 `isCurrent` 那条。

### 3.6.1 共享会话池 + 单活跃归属（你已确认 shared_pool）

> 专业：两前端共享同一项目 scope 会话池（`session_key` 归组，见 [`02`](02-stage-a-slash-and-serve.md) §3.4.5），但激活态单归属，防双驱动。
> 说人话：历史大家一起看，但同一条对话同一时刻只许一个面板在跑。

| 维度 | 规则 | 落点 | 说人话 |
|------|------|------|--------|
| 历史枚举 | 两前端按同一 `session_key` 各自 `list_sessions{scope:"disk"}`，看同一份历史 | serve SA8 + 扩展两侧渲染 | 同一份项目会话清单。 |
| 默认指向 | 启动默认选 `current[session_key]`（last-active），非 `updated_at` 最大者 | serve `ensure_current_session`（已实现） | 默认接上次那条。 |
| 激活归属 | 某会话被前端激活成 live slot → 登记 `sessionId→owner`；owner 唯一 | 扩展侧归属表 | 谁激活谁负责驱动。 |
| 冲突处理 | 非 owner 想驱动同一会话 → 只读渲染 + "该会话正由另一入口使用"提示；硬保护读 serve `slot.is_busy` | 扩展侧 + serve `registry`(`is_busy`) | 另一面板想抢就拦下、给提示。 |
| 释放 | owner 关闭 tab/线程或 `close_session` → 释放归属，其他前端可接管 | 扩展侧 + `close_session` | 上一个放手了，别人才能接。 |

## 3.7 打包影响

- `gui/` 独立 Vite 构建产物（`gui/dist`）通过 `scripts/package-vsix.ts` 暂存清单纳入 VSIX；`.vscodeignore` 排除 `gui/` 源码与 `node_modules`，仅留 `gui/dist`。
- CSP 引用打包后资源（`asWebviewUri(gui/dist/...)`）+ nonce。
- `engines.vscode` 维持 Phase 1 稳定基线，不因 webview 抬高。
- 打包冒烟扩展现有 `tests/package_vsix_smoke.test.ts`：断言 `gui/dist` 入包、源码不入包。

---

## 验收锚点（汇总，详见 04 §8）

| 锚点编号 | 验收内容 | 层 |
|----------|----------|----|
| `T2B-PROTO-UNIT` | webview 协议编解码：`{messageId,done,content}` 流式 append、未知 id 丢弃 | 扩展单元 |
| `T2B-STREAM-UNIT` | 双通道：`event` 透传 Phase 1 事件（断言 `thinking_delta`/`tool`/`ask_question` 各命中对应 UI 元素）、重连 `state` 快照幂等覆盖 | 扩展单元 |
| `T2B-BRIDGE-REUSE` | 同一 `TomcatMessenger` 实例并发服务 participant + webview，核心代码无 diff | 扩展集成 |
| `T2B-SCOPE-POOL-INT` | webview 启动列当前项目 scope 历史会话、默认选中 last-active（`isCurrent`） | 扩展集成 |
| `T2B-OWNERSHIP-INT` | 同一 live 会话单前端归属：第二前端驱动同会话被拒/只读；owner 释放后可接管 | 扩展集成 |
| `T2B-MULTISESSION-E2E` | webview 开 2 个 tab → 2 个会话不串台 | 真实宿主 E2E |
| `T2B-DIFF-E2E` | webview 触发应用编辑 → 走 Phase 1 IDE 真改文件 | 真实宿主 E2E |
| `T2B-WEBVIEW-E2E` | 真实宿主：webview 流式渲染一问一答，CSP 无报错 | 真实宿主 E2E |
| `T2B-PKG-SMOKE` | VSIX 含 `gui/dist`、不含 `gui/` 源码 | 打包冒烟 |

> 说人话：Stage B 验收的两条硬门禁是「**桥接核心零改动还能同时喂两个前端**」和「**两前端共享项目会话池、同一条 live 会话单前端归属（不双驱动）**」——这俩过了，webview 就是干净的纯增量。
