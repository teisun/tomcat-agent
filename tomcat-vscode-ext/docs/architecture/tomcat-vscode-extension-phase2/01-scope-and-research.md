# Tomcat VSCode 扩展 · Phase 2 · 01 术语与竞品调研

> 总览见 [`../tomcat-vscode-extension-phase2.md`](../tomcat-vscode-extension-phase2.md)（含定位、阅读顺序与文首导图集）。
> 本文对应 [`ARCHITECTURE_SPEC.md`](../../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) 的 **§1 术语统一** 与 **§2 竞品 / 选型对比（调研）**。
> 单一事实源：协议与类型以 `tomcat/src/api/serve/types.rs` 为准；plan 行为以 `tomcat/src/core/plan_runtime/mod.rs` 为准。
> 外部参考仓库（与本仓同级，位于 `/Users/yankeben/workspace/`，仅作证据引用）：`vscode/`、`cline/`、`continue/`。

---

## 1. 术语统一

> 专业：本节钉死 Phase 2 新引入的交界处命名，复用 Phase 1 术语（参与者 / 桥接核心 / sessionId / control 帧 / ask_question 等，见 Phase 1 [`01-terminology-and-research.md`](../tomcat-vscode-extension/01-terminology-and-research.md)），只补 Phase 2 增量。
> 说人话：先把"slash 命令、计划模式、模型目录、双前端、webview 协议"这几个 Phase 2 新词说清楚。

| 术语 | 语义（大白话） | 数据载体 | 行为约束 | 说人话 |
|------|----------------|----------|----------|--------|
| slash command（斜杠命令） | 原生聊天里 `@tomcat /plan` 形式的子命令 | `package.json` `contributes.chatParticipants[].commands[]`（`{name,description}`，稳定）；handler 读 `request.command` | 由 VSCode 解析为 agent 子命令；命令名是稳定字符串，handler 内分流 | 聊天框里 `@tomcat` 后面打 `/` 弹出来的那些命令。 |
| plan 模式 / PlanState | Tomcat 的"先规划再执行"运行态 | `tomcat/src/core/plan_runtime/state.rs::PlanState`（Chat/Planning/Executing/Pending/Completed） | per-session、跨 turn 持久；只由 `PlanRuntime` 三方法切换；**当前仅 CLI 可驱动，serve 缺命令** | Tomcat 那个 `/plan` 进去先列计划、`build` 再开干的模式。 |
| `set_plan_mode`（Stage A 新增 serve 命令） | 扩展驱动 plan 模式切换的 serve 命令 | 拟新增 `ServeCommand::SetPlanMode{action,planId?}` | `action∈{enter,exit,build}`；桥接 `PlanRuntime::enter_planning/exit_to_chat/build_plan` | 让扩展也能像 CLI 那样切计划模式的新命令。 |
| 模型目录 / ModelCatalog | 可用模型清单 | `tomcat/src/core/llm/catalog.rs::ModelCatalog`（内置表 + `~/.tomcat/models.toml`） | `entries()` 返回排序后的 `ModelEntry`；当前 serve **无枚举命令** | "有哪些模型可选"的那张表。 |
| `list_models`（Stage A 新增 serve 命令） | 扩展枚举可用模型的 serve 命令 | 拟新增 `ServeCommand::ListModels{id}` | 回 `ResponseFrame.payload.models[]`；过渡期扩展可直接读 `models.toml` | 让扩展拿到模型清单去填 QuickPick/下拉。 |
| 双前端并存 | participant 与 webview 同时注册可用 | 扩展激活时同注册两者；`tomcat.ui` 可关其一 | 共享层 2 桥接核心 + 单 serve；**共享同一项目 scope 会话池**，单条 live 会话单前端归属 | 原生聊天和自建面板两个入口同时在，看的是同一份项目会话。 |
| 项目 scope 会话池 | 当前 git 项目下的全部历史会话集合 | `sessions.json` 按 `session_key` 归组（Code 模式 = git 项目根 hash，`scope.rs:49`）；`SessionManager::list_sessions()` 列举（`session_impl.rs:380`，按 `updated_at` 倒序） | 两前端按同一 `session_key` 枚举同一份历史；与 `tomcat code` 共写同一 `sessions.json` | "这个项目下我开过哪些对话"的那张表，code 和插件共用。 |
| last-active 恢复 | 启动默认指向上次停留的会话 | `sessions.json` 的 `current[session_key]` 指针（每次 switch/new 更新，`session_impl.rs:188`）；`ensure_current_session()` 恢复（`:313`） | 与 `tomcat code` 同语义：默认恢复指针指向的会话，非自动选 `updated_at` 最大者 | 一打开就接着上次那条聊。 |
| 单活跃归属（single live-owner） | 一条 live 会话同时只归一个前端驱动 | 扩展侧 `sessionId → owner(frontend)` 映射 + serve `slot.is_busy` 硬保护 | 历史**枚举共享**；但**激活成 live 会话单归属**，另一前端想用同一会话 → 只读/冲突提示 | 同一个对话不能俩面板一起发消息抢着跑。 |
| webview 宿主协议 | webview ↔ 扩展宿主的消息通道 | typed `postMessage` **传输信封** `{messageId,channel,done,content}`（学 continue） | 仅换信封不换语义：因 webview 是浏览器沙箱，唯一通道是 postMessage（Phase 1 stdio wire 无法直达）；`content` 仍原样承载 Phase 1 事件 | 自建面板和插件主进程之间收发消息的格式。 |
| state/event 双通道 | webview 渲染的两条数据流 | 全量 `state` 快照（重连用）+ `event` **透传 Phase 1 `WireEvent`**（含 `thinking_delta`/`tool`/`ask_question`，学 cline 双通道形态） | 初始化/切换/重连走全量 state；平时逐条透传 Phase 1 事件 → 与 participant 同源，富 UI（思考/工具/审批/diff）与 vscode chat 等价 | 一条管"整体长啥样"（重连用）、一条把后端事件原样传进来逐条画。 |
| 自建 Webview（前端 B） | React 渲染的富交互聊天 UI | `contributes.views` + `WebviewViewProvider`（稳定） | CSP nonce + `asWebviewUri` + `localResourceRoots`；复用桥接核心 | 我们自己画的那套漂亮聊天面板。 |

---

## 2. 竞品 / 选型对比（调研）

> 专业：本节是 [`02-stage-a-slash-and-serve.md`](02-stage-a-slash-and-serve.md) §3 与 [`03-stage-b-webview.md`](03-stage-b-webview.md) §3 已定稿结论的证据链。先回答"VSCode 到底能不能接、用什么接"（§2.1 + §2.2），再回答"webview 怎么照着 Cline/Continue 做"（§2.3）。
> 说人话：先用 vscode 源码证明 `/plan` `/model` 用 slash 命令能接、"Configure custom agents"接不了；再看 Cline/Continue 的 webview 是怎么搭的，照着学。

### 2.1 VS Code 能力裁决：`/plan` `/model` 怎么接（结论先行）

| 关切 | 结论 | 取自（vscode 源码） | 说人话 |
|------|------|----------------------|--------|
| `@tomcat /plan` 能否声明 | **能，稳定**。`contributes.chatParticipants[].commands[]` 是一等稳定贡献点；handler 收 `request.command` | `vscode/src/vs/workbench/contrib/chat/browser/chatParticipant.contribution.ts:149`（commands schema，仅 `name`/`description`）、`vscode/src/vscode-dts/vscode.d.ts:19866`（`ChatRequest.command`）、解析路由 `vs/workbench/contrib/chat/common/requestParser/chatRequestParser.ts:224`（`usedAgent.slashCommands.find`） | 在清单里声明 `/plan`，handler 里读到 `command==="plan"` 就分流，全程稳定。 |
| 用户选的模型能否读到 | **能，只读**。`request.model: LanguageModelChat` 反映 UI 选择 | `vscode/src/vscode-dts/vscode.d.ts:19899`（`ChatRequest.model`）、`extHostChatAgents2.ts:916`（由 `request.userSelectedModelId` 解析） | 能看到用户在 UI 里选了哪个模型，但不能反向替用户改。 |
| 能否把 Tomcat 模型塞进原生模型选择器 | **能，稳定但偏重**。`languageModelChatProviders` 贡献点 + `lm.registerLanguageModelChatProvider` | `vscode/src/vscode-dts/vscode.d.ts:20847`（稳定注册 API）、`vs/workbench/contrib/chat/common/languageModels.ts`（`languageModelChatProviders` 贡献点 schema） | 可以让 Tomcat 模型出现在原生模型菜单，但接近已否决的"形态 B"，Phase 2 优先用自带 `/model` QuickPick。 |
| "Configure custom agents" 能否绑 `@tomcat` | **不能（对我们没用）**。它是 `.agent.md` 自定义模式系统，运行时注册 API 为 proposed，且不绑 participant | `vscode/src/vs/workbench/contrib/chat/browser/chatSlashCommands.ts:137`（label「Configure custom agents」→ `:147` `OpenModePickerAction`）、`vscode.proposed.chatPromptFiles.d.ts:472`（`registerCustomAgentProvider` 属 proposed） | 那个菜单管的是"换系统提示词的 markdown 模式"，动态注册要 proposed，第三方拿不到，也挂不到 `@tomcat`，所以不走它。 |

选型理由（为什么 `/plan` `/model` 用 slash command，不用 Configure custom agents）：

1. **slash command 全稳定可上架**：`contributes.chatParticipants[].commands` 与 `request.command` 均在稳定 `vscode.d.ts`，无 proposed 门禁；
2. **Configure custom agents 是另一套系统**：`.agent.md` 模式由 workbench 自己渲染，动态注册 `registerCustomAgentProvider` 在 `vscode.proposed.chatPromptFiles.d.ts:472`，第三方扩展不可用；
3. **它绑不到我们的 participant**：自定义 agent 的 `target` 枚举是 `vscode`/`github-copilot`/`claude` 执行后端，无"绑定某个 `@participant`"的属性，接进去也驱动不了 Tomcat 的 loop；
4. **真正瓶颈在 Tomcat serve 后端，不在 VSCode**：见 §2.2。

### 2.2 Tomcat serve 现状缺口（为什么 Stage A 重头戏在 Rust 后端）

| 能力 | serve 现状 | 缺口与补法 | 取自（本仓代码） | 说人话 |
|------|------------|------------|------------------|--------|
| 切换模型 `/model` | **已支持**。`set_model` 命令在 capabilities 内；`get_state` 回当前 `model` | 仅缺"枚举可用模型"——新增 `list_models` 或过渡期读 `models.toml` | `tomcat/src/api/serve/types.rs:128`（`SetModel`）、`control.rs:40`（capabilities 含 `set_model`）、`commands.rs:239`（handler）、`commands.rs:235`（`get_state.model`） | 切模型后端早能切，只差"告诉前端有哪些模型可选"。 |
| 枚举模型 | **不支持**。无 `list_models` 命令 | 新增 `ServeCommand::ListModels` → `ModelCatalog::entries()` | `tomcat/src/api/serve/control.rs:35`（capabilities 无 list_models）、`tomcat/src/core/llm/catalog.rs:137`（`entries()` 现成） | 后端有模型目录，只是没开"列出来"的命令口。 |
| 进入/退出/构建 plan | **完全不支持**。plan 只在 CLI REPL 经 `dispatch_chat_command` 驱动；serve 把 `"/plan"` 当 prompt 文本 | 新增 `ServeCommand::SetPlanMode{action}` → 桥接 `PlanRuntime` 三方法 | `tomcat/src/api/chat/commands/cmd_plan.rs:53`（CLI `/plan` 分发）、`tomcat/src/core/plan_runtime/mod.rs:340`（`enter_planning`）、`:361`（`exit_to_chat`）、`:939`（`build_plan`）；serve 命令枚举 `types.rs:90` 无任何 plan 变体 | plan 引擎早写好了，但 serve 没开命令口，扩展现在根本驱动不了它。 |
| 读 plan 当前态 | **部分**。`get_state` 回的 `mode` 是会话 scope（code/claw），不是 PlanState | `get_state` payload 增 `planState` 字段 | `tomcat/src/api/serve/commands.rs:233`（`mode` 来自 `slot.mode`，非 PlanState） | 现在问"什么模式"答的是 code/claw，不是 plan 态，得补。 |
| plan 生命周期事件 | **不在 serve 事件流**。`plan.*` 由 `write_transcript_custom` 落 transcript，不经 event_pump 白名单 | `event_pump.rs::EVENT_NAMES` 加 `WIRE_PLAN_*`，并让 `PlanRuntime` 同时经 event_bus emit | `tomcat/src/api/serve/event_pump.rs:14`（`EVENT_NAMES` 无 plan.*）、`plan_runtime/mod.rs:1088`（`write_transcript_custom` 写 `WIRE_PLAN_BUILD`） | plan 进度现在只写日志、不推给前端，要把它接到事件流上前端才看得见。 |

> 说人话：这张表是 Phase 2 的命门——**VSCode 侧几乎零成本，所有真正的工作量都在给 `tomcat serve` 补命令**。`/model` 后端基本现成（只差枚举），`/plan` 后端要新增命令把早已存在的 `PlanRuntime` 暴露出来。

### 2.3 Webview 形态横向对比（Stage B 借鉴 Cline / Continue）

| 竞品 | 形态 | 关键设计 | 我们借鉴的点 | 说人话 |
|------|------|----------|---------------|--------|
| Cline | 纯 Webview（侧栏）+ ProtoBus | `WebviewViewProvider` 注入 React + CSP nonce（`WebviewProvider.ts:75`、CSP `:113`、`getNonce.ts`）；ProtoBus over postMessage（`grpc-handler.ts:63` 按 `is_streaming` 分流）；**双通道**：全量 state（`state.proto` `subscribeToState`）+ 增量 partial（`ui.proto::ClineMessage.partial`，`task/index.ts:1046` `sendPartialMessageEvent`）；diff 用虚拟只读文档(`cline-diff` scheme)+`vscode.diff`+`WorkspaceEdit`+装饰（`VscodeDiffViewProvider.ts:67`）；Plan/Act 模式 toggle（`controller/index.ts:388` `togglePlanActMode`，`ToolExecutor.ts:342` 工具门禁）；**单活跃 task**（`Controller.task?`，`index.ts:246` 新 task 前 `clearTask`） | webview 注入 + CSP nonce 写法、**双通道流式**、稳定 diff 写法、Plan 模式 UI toggle、审批按钮三态 | 自己画 UI 的范本：连 diff、审批、plan 切换都用稳定 API 做出来了。 |
| Continue | core/gui/ide 三层 + `binary/` NDJSON 子进程 | `core/`(引擎)+`gui/`(React)+`extensions/vscode/`(宿主) 三层（`core/core.ts` 注入 `IDE`）；NDJSON 子进程 `binary/src/IpcMessenger.ts`（信封 `{messageType,messageId,data}` 校验 `:23`、`\r\n` 分帧 `:100`、id↔Promise 关联 `:122`、AsyncGenerator 流式 `{done,content}` `:37`）；webview 协议 `extensions/vscode/src/webviewProtocol.ts`（同形 postMessage）+ gui `IdeMessenger.tsx`；`IDE` 抽象 `core/index.d.ts`（readFile/writeFile/openFile/getWorkspaceDirs…）；模型 roles `selectedModelByRole`（`core/llm/streamChat.ts:33`）；`MessageModes`（`core/index.d.ts`）+ `gui/src/redux/util/getBaseSystemMessage.ts` 按 mode 切系统提示 + `gui/src/components/ModeSelect/ModeSelect.tsx` 循环 UI；diff 用 `extensions/vscode/` `VerticalDiffManager` 流式 `DiffLine` + CodeLens accept/reject | **三层分离 + NDJSON 子进程（与 TomcatMessenger 几乎同构）**、webview 协议同形复用、`IDE` 能力抽象、mode→系统提示映射、流式 `{done,content}` 帧 | 跟我们最像：它的"子进程 NDJSON + 三层 + webview 同形协议"几乎就是 Phase 2 的蓝本。 |
| Copilot Chat（对照） | 原生 chat + 内置 UI + proposed API | agent 模式/编辑 diff/思考块/审批卡由 VSCode core 渲染、靠 proposed（`chatParticipantAdditions`）+ 身份门禁 | 仅借交互范式，不借代码、不依赖 proposed | 体验天花板，但靠"自己人特权"，只学它长什么样。 |

选型理由（Stage B 为什么这么搭）：

1. **桥接核心已与 UI 解耦**：Phase 1 的 `TomcatMessenger`（学 Continue `IpcMessenger`）天然能再喂一个 webview 前端，零返工；
2. **webview 协议采 Continue 的轻量 `{messageId,done,content}`，不抄 Cline 的 gRPC/proto**：Tomcat serve 本身就是 NDJSON 一问一流答，Continue 式 typed postMessage 同构、依赖最小；
3. **双通道（全量 `state` 快照 + 增量 `event`）采 Cline 实证的形态**：但增量通道**原样透传 Phase 1 `WireEvent`**（含 thinking/tool/审批），不另造 UI 语义——既保证与 participant 同源的 vscode chat 富 UI，又省掉每次全量重画；
4. **diff 采两者共有的稳定写法**（虚拟文档 + `vscode.diff` + `WorkspaceEdit`），与 Phase 1 R5 一致，复用 `ide/VsCodeIde.ts`；
5. **多会话 tab 是 Tomcat 相对 Cline 的优势**：Tomcat serve 原生并发多会话（`registry.rs`），webview 可做多 tab，而 Cline 单活跃 task 做不到。

> 说人话：结论——**Stage B 不发明新东西**：分层和子进程协议学 Continue，双通道流式和 diff/plan 切换 UI 学 Cline，桥接核心直接复用 Phase 1。唯一比它们强的点是多会话并发（Tomcat 后端原生支持）。
