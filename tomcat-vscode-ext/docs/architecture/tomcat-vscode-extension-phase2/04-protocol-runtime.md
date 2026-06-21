# Tomcat VSCode 扩展 · Phase 2 · 04 协议与运行时参考

> 总览见 [`../tomcat-vscode-extension-phase2.md`](../tomcat-vscode-extension-phase2.md)（含定位、阅读顺序与文首导图集）。
> 本文对应 [`ARCHITECTURE_SPEC.md`](../../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) 的 **§4 协议**、**§5 One-Glance Map**、**§6 配置**、**§7 错误模型**、**§8 测试矩阵**、**§9 风险**、**§10 历史决策**。
> 上游设计见 [`02-stage-a-slash-and-serve.md`](02-stage-a-slash-and-serve.md)（serve 命令语义）与 [`03-stage-b-webview.md`](03-stage-b-webview.md)（webview 通道）。
> 单一事实源：serve 协议类型 `tomcat/src/api/serve/types.rs`（`ServeCommand`/`ResponseFrame`/`OutFrame`）、capabilities `tomcat/src/api/serve/control.rs`、事件白名单 `tomcat/src/api/serve/event_pump.rs`。本文字段表与 Phase 1 [`../tomcat-vscode-extension/03-protocol-and-file-map.md`](../tomcat-vscode-extension/03-protocol-and-file-map.md) §4 互补（Phase 1 命令不在此重复）。

---

## 4. 协议（Phase 2 增量）

> 专业：Phase 2 只增量定义 3 处——新 serve 命令 `set_plan_mode`/`list_models`、`get_state` 响应新增 `planState`、webview↔host 双通道帧。命令帧沿用 `ServeCommand` 既有约定：枚举 `#[serde(tag="type", rename_all="snake_case")]`，字段 `rename_all="camelCase"`，可选字段 `skip_serializing_if`。所有线上字段名为 camelCase。
> 说人话：Phase 2 在协议上只多了两条命令、一个状态字段，加上 webview 自己那层消息格式；其它全沿用 Phase 1。

### 4.1 新 serve 命令字段表（UI → serve）

下表「三态」= 序列化形态：`必填` / `可选(省略则不下发)` / `定值`。命令落点 `tomcat/src/api/serve/types.rs::ServeCommand`（新增 `SetPlanMode`/`ListModels` 变体）。

#### `set_plan_mode`（驱动 PlanState；桥接 `PlanRuntime`）

| 字段 | 类型 | 必填 | 默认 | 三态 | 含义 | 说人话 |
|------|------|------|------|------|------|--------|
| `type` | string | 是 | — | 定值 `"set_plan_mode"` | 命令判别（snake_case tag） | 告诉后端这是计划模式命令。 |
| `id` | string | 否 | 无 | 可选 | 命令关联 id，回执原样带回 | 用来对上请求和回执。 |
| `sessionId` | string | 否 | active session | 可选 | 目标会话；省略走当前活跃会话 | 切哪个会话的计划态。 |
| `action` | enum | 是 | — | 必填 `"enter"\|"exit"\|"build"` | 动作；映射 `enter_planning`/`exit_to_chat`/`build_plan` | 进/退/开跑。 |
| `planId` | string | 否 | runtime 默认源 | 可选（仅 `build` 有意义） | 构建目标 plan_id 或 path；省略走 `default_build_target()` | 开跑哪份计划，不填用默认。 |

响应 `ResponseFrame.payload`（`success:true`）：

| 字段 | 类型 | 必填 | 含义 | 说人话 |
|------|------|------|------|--------|
| `planState` | string | 是 | 切换后状态 `chat\|planning\|executing\|pending\|completed` | 现在是什么计划态。 |
| `planId` | string | 否 | `executing/pending/completed` 时带 | 跟哪份计划绑着。 |
| `planPath` | string | 否 | `build` 成功时回写 `~/.tomcat/plans/<id>.plan.md` | 计划文件在哪。 |

#### `list_models`（枚举 `ModelCatalog`）

| 字段 | 类型 | 必填 | 默认 | 三态 | 含义 | 说人话 |
|------|------|------|------|------|------|--------|
| `type` | string | 是 | — | 定值 `"list_models"` | 命令判别 | 告诉后端"把模型列出来"。 |
| `id` | string | 否 | 无 | 可选 | 命令关联 id | 对上请求回执。 |

响应 `ResponseFrame.payload`（`success:true`）：

| 字段 | 类型 | 必填 | 含义 | 说人话 |
|------|------|------|------|--------|
| `models` | array | 是 | `ModelCatalog::entries()` 映射；元素至少含 `{id:string}` | 可选模型清单。 |
| `models[].id` | string | 是 | 模型标识（用于 `set_model`） | 每个模型的名字。 |

#### `get_state`（响应新增 `planState`，命令本身不变）

| 响应字段 | 类型 | 必填 | 含义 | 说人话 |
|----------|------|------|------|--------|
| `mode` | string | 是（既有） | 会话 scope：`code\|claw`（不变，源 `commands.rs:233`） | 这会话是 code 还是 claw。 |
| `model` | string | 是（既有） | 当前模型（不变，源 `commands.rs:235`） | 现在用哪个模型。 |
| `planState` | string | 是（**新增**） | plan 生命周期：`chat\|planning\|executing\|pending\|completed`，读 `plan_runtime.mode()` | 现在的计划态。 |
| `planId` | string | 否（**新增**） | 非 chat 态时带 | 绑的哪份计划。 |
| `sessionKey` | string | 否（**新增**） | 当前 scope 的 `session_key`（`scope.rs:49`），webview 据此分组 | 这会话属于哪个项目 scope。 |

#### `list_sessions`（扩展：磁盘 scope 历史；SA8）

> 既有 `list_sessions` 只回进程内 registry live slot（`commands.rs:205`）。Stage A 新增可选 `scope` 参数，回当前项目 scope 的磁盘全量历史。

| 字段 | 类型 | 必填 | 默认 | 三态 | 含义 | 说人话 |
|------|------|------|------|------|------|--------|
| `type` | string | 是 | — | 定值 `"list_sessions"` | 命令判别 | 列会话。 |
| `id` | string | 否 | 无 | 可选 | 命令关联 id | 对回执。 |
| `scope` | string | 否 | `"live"` | 可选 `"live"`(registry,既有)\|`"disk"`(磁盘 scope 全量,新增) | 列范围 | 列内存里的还是项目历史全部。 |

响应 `payload`（`scope:"disk"` 时，源 `SessionManager::list_sessions()` `session_impl.rs:380`，按 `updatedAt` 倒序）：

| 字段 | 类型 | 必填 | 含义 | 说人话 |
|------|------|------|------|--------|
| `sessions` | array | 是 | scope 内会话，倒序 | 本项目历史会话。 |
| `sessions[].sessionId` | string | 是 | 会话 id | 每条的 id。 |
| `sessions[].updatedAt` | number | 是 | 最近更新时间戳 | 最近啥时候动过。 |
| `sessions[].isCurrent` | boolean | 是 | 是否 `current[session_key]`（last-active） | 是不是"上次那条"。 |
| `sessions[].busy` | boolean | 否 | 该会话当前是否 live 且忙（`slot.is_busy`） | 是不是正被某入口跑着。 |

#### `switch_session`（扩展：可切磁盘历史会话；SA8）

> 既有 `switch_session` 只切 registry 内 slot（不在 registry 报 `unknown_session`，`commands.rs:150`）。Stage A 扩展为：目标 id 不在 registry 但属于当前 scope 时，自动 `switch_current_to_session_id`（`session_impl.rs:321`）+ 重建 `SessionSlot` + 注册（`open_existing_session(id)` 组合）。字段沿用 Phase 1（`sessionId` 必填），无新增。

### 4.2 capabilities 与事件白名单增量

| 增量 | 落点 | 值 | 说人话 |
|------|------|----|--------|
| capabilities += | `control.rs:35`（initialize payload） | `"set_plan_mode"`、`"list_models"` | 握手时多声明俩能力。 |
| `EVENT_NAMES` += | `event_pump.rs:14` | `WIRE_PLAN_CREATE/BUILD/UPDATE/REVIEW/VERIFY/COMPLETE`（具体常量名以 `infra/wire` 为准） | 让 plan 事件能被转发。 |
| event_bus emit（前提） | `core/plan_runtime/mod.rs` 对应动作处 | 现仅 `write_transcript_custom`，需补 `event_bus.emit(WIRE_PLAN_*, {sessionId,...})` | 事件得真发出来，白名单才接得到。 |

### 4.3 jsonc 调用样例

```jsonc
// UI → serve：进入计划模式
{ "type": "set_plan_mode", "id": "c1", "sessionId": "sid_a1", "action": "enter" }
// serve → UI：回执
{ "type": "response", "id": "c1", "success": true, "sessionId": "sid_a1",
  "payload": { "planState": "planning" } }

// UI → serve：开跑（缺省 target 走 default_build_target）
{ "type": "set_plan_mode", "id": "c2", "sessionId": "sid_a1", "action": "build" }
{ "type": "response", "id": "c2", "success": true, "sessionId": "sid_a1",
  "payload": { "planState": "executing", "planId": "20260621-foo",
               "planPath": "~/.tomcat/plans/20260621-foo.plan.md" } }
// 随后 serve 自动注入 user turn "start building ~/.tomcat/plans/20260621-foo.plan.md"（对齐 CLI）

// UI → serve：列模型
{ "type": "list_models", "id": "c3" }
{ "type": "response", "id": "c3", "success": true,
  "payload": { "models": [ { "id": "claude-opus-4" }, { "id": "gpt-5.5" } ] } }

// UI → serve：列当前项目 scope 的磁盘历史会话（SA8）
{ "type": "list_sessions", "id": "c5", "scope": "disk" }
{ "type": "response", "id": "c5", "success": true,
  "payload": { "sessions": [
    { "sessionId": "s_20260620_2", "updatedAt": 1718900000, "isCurrent": true },
    { "sessionId": "s_20260618_1", "updatedAt": 1718700000, "isCurrent": false } ] } }
// UI → serve：打开一条磁盘历史会话（即便不在 registry）
{ "type": "switch_session", "id": "c6", "sessionId": "s_20260618_1" }
{ "type": "response", "id": "c6", "success": true, "sessionId": "s_20260618_1" }

// 失败样例：Executing 态下 exit 被拒
{ "type": "set_plan_mode", "id": "c4", "sessionId": "sid_a1", "action": "exit" }
{ "type": "response", "id": "c4", "success": false, "sessionId": "sid_a1",
  "error": "plan_state_conflict" }
```

### 4.4 webview ↔ host 帧（Stage B）

> 关键认知（回答"为何不直接=Phase 1 wire"）：webview 是浏览器沙箱进程，与宿主唯一通道是 `postMessage`；Phase 1 wire 是 `tomcat serve`(Rust)↔宿主(Node) 的 **stdio NDJSON**，**传输层不同**，必须有一层 host↔webview 信封。但**事件语义 100% 沿用 Phase 1**——webview 承载与 participant **同一套事件词汇**（`thinking_delta`/`tool_execution_*`/`ask_question`/`plan.*`…），以保证与 vscode chat **同等 UI 体验**（思考块/工具卡/审批卡/diff/子代理/用量）。本层只新增 Phase 1 没有、participant 也不需要的三样：**快照通道**（webview 重建时一次性恢复）、**意图通道**（用户动作回传宿主调 VSCode API）、**会话池/owner 信息**（双前端共享池仲裁）。

> 说人话：不是另起一套 UI 语义，而是"换个信封"。Phase 1 那套 thinking/工具/审批事件原样传进 React，所以体验和原生聊天一致；只额外加"重连给快照、点击回传意图、谁占用会话"这三件 webview 特有的事。

#### host → webview（渲染）

| 字段 | 类型 | 必填 | 含义 | 说人话 |
|------|------|------|------|--------|
| `messageId` | string | 是 | 帧 id；流式同 id 续传 | 这条消息的编号。 |
| `channel` | string | 是 | `"state"`（全量快照）\| `"event"`（透传 Phase 1 事件） | 整体快照还是逐条事件。 |
| `done` | boolean | 否 | `event` 为流式时标识该消息是否收尾 | 字打完没。 |
| `content` | object | 是 | `state`=会话视图快照；`event`=**原样携带 Phase 1 `WireEvent`**（见下表富类型） | 具体内容。 |

`channel:"event"` 的 `content` 直接是 Phase 1 `WireEvent`（`type` 仍 snake_case），webview 渲染映射与 participant `render.ts` 同源——**保证 vscode chat 同款富 UI**：

| Phase 1 事件 `type`（透传给 webview 的 `content.type`） | webview 渲染 | UI 元素 | 说人话 |
|------|------|------|--------|
| `message_update` `kind:"content_delta"` | 正文气泡 append | 流式正文 | 正文一点点蹦。 |
| `message_update` `kind:"thinking_delta"`（含 `signature?`） | **折叠思考块** append | thinking | 思考过程（和 vscode chat 一样可折叠）。 |
| `message_start`/`message_end` | 新建/收尾消息气泡 | 消息块 | 一条消息头尾。 |
| `tool_execution_start` | 起工具卡（running） | 工具卡 | 开始用工具。 |
| `tool_execution_update`/`tool_call_streaming` | 工具卡进度 | 工具卡 | 参数/中间结果在路上。 |
| `tool_execution_end`（`display{kind:file\|plan\|text}`,`isError`） | 工具卡收尾；`file`→diff 入口 | 工具卡 + diff | 结果回来了，文件类给"看 diff"。 |
| `control_request{subtype:"ask_question"}` | **审批卡**（推荐项高亮，按钮三态） | 审批卡 | 弹审批/选择（同 vscode chat 卡片）。 |
| `plan.*`（Stage A） | 计划徽标/计划面板刷新 | plan 视图 | 计划进度。 |
| `context_metrics_update` | 用量/上下文徽标 | 状态条 | token/上下文占用。 |
| `sub_agent_start`/`sub_agent_end` | 子代理关联展示 | 子代理块 | 派生子 Agent。 |
| `agent_end`/`agent_interrupted`/`llm_error`/`llm_notice` | 收尾/错误/提示 | 状态 | 一轮结束或报错/提示。 |

> `state` 快照 `content` 结构（用于初始化/重连/会话切换，幂等覆盖）：`{sessionId, messages[], plan:{state,planId?}, model, tools[], ownedByThisFrontend:boolean}`，由宿主基于已收 Phase 1 事件聚合，webview 一次性渲染后再续接 `event`。

#### webview → host（意图）

| 字段 | 类型 | 必填 | 含义 | 说人话 |
|------|------|------|------|--------|
| `messageId` | string | 是 | 请求 id，回执对齐 | 请求编号。 |
| `type` | string | 是 | `"prompt"\|"steer"\|"interrupt"\|"setModel"\|"setPlanMode"\|"newSession"\|"switchSession"\|"closeSession"\|"listSessions"\|"applyEdit"\|"openDiff"\|"answerQuestion"` | 想干啥（映射 Phase 1 命令 + IDE 动作）。 |
| `data` | object | 否 | 该意图的参数（如 `{action,planId}`、`{questionId,optionIds}`、`{path}`） | 参数。 |

```jsonc
// host → webview：透传一条 thinking 事件（与 participant 折叠思考块同源）
{ "messageId": "m7", "channel": "event", "done": false,
  "content": { "type": "message_update", "sessionId": "sid_b1",
               "assistantMessageEvent": { "kind": "thinking_delta", "delta": "先定位失败用例…" } } }
// host → webview：透传一张工具卡收尾（带 diff 入口）
{ "messageId": "m8", "channel": "event", "done": true,
  "content": { "type": "tool_execution_end", "sessionId": "sid_b1", "toolCallId": "call_1",
               "toolName": "edit", "display": { "kind": "file", "file": "src/foo.rs" }, "isError": false } }
// host → webview：会话切换/重连时给完整快照
{ "messageId": "m0", "channel": "state",
  "content": { "sessionId": "sid_b1", "messages": [/* … */],
               "plan": { "state": "executing", "planId": "20260621-foo" },
               "model": "claude-opus-4", "ownedByThisFrontend": true } }
// webview → host：用户点审批卡"应用"（宿主回 control_response 给 serve）
{ "messageId": "r9", "type": "answerQuestion",
  "data": { "questionId": "apply", "optionIds": ["yes"] } }
// webview → host：用户点"看 diff"（宿主调 Phase 1 VsCodeIde，webview 不直接碰 VSCode API）
{ "messageId": "r10", "type": "openDiff", "data": { "path": "src/foo.rs" } }
```

### 4.5 单一事实源声明

- 命令/响应/事件帧类型：`tomcat/src/api/serve/types.rs`（新增变体后由 `#[derive(JsonSchema)]` 自动进 schema）。
- capabilities：`tomcat/src/api/serve/control.rs`。事件白名单：`tomcat/src/api/serve/event_pump.rs`。wire 常量：`tomcat/src/infra/wire`。
- 扩展侧类型 `src/serveClient/wire.d.ts` 由 `tomcat serve --print-schema` 生成；CI `npm run check:wire` 校验一致（防漂移）。
- webview 帧 `{messageId,channel,done,content}` 为扩展侧私有**传输信封**（不进 serve schema），事实源 `src/ui/webview/protocol.ts`。注意：`channel:"event"` 的 `content` **不是新语义**，而是原样携带 Phase 1 `WireEvent`（事实源仍是 `tomcat/src/api/serve/types.rs` + `tomcat/src/infra/events/mod.rs`）——webview 借此获得与 participant 同源的富 UI（thinking/tool/审批/diff）。

---

## 5. One-Glance Map（Phase 2 文件职责框图）

> 专业：只画 Phase 2 新增/改动节点；Phase 1 既有文件（`extension.ts`/`TomcatMessenger.ts`/`ide/*`/`sessionRouter.ts` 等）见 Phase 1 [`03-protocol-and-file-map.md`](../tomcat-vscode-extension/03-protocol-and-file-map.md) §5，本图以「(P1,复用)」标注、不展开。
> 说人话：一眼看清"这次新增了哪些文件、各干嘛、谁锁它"。

```text
扩展侧（tomcat-vscode-ext/，TS）
├─ package.json
│    contributes.chatParticipants[0].commands += {plan,model}   ◀NEW(Stage A)  ←test: T2A-MANIFEST
│    contributes.views += tomcat webview                        ◀NEW(Stage B)  ←test: T2B-WEBVIEW-E2E
├─ src/serveClient/
│    TomcatMessenger.ts        (P1,复用) + sendSetPlanMode()/sendListModels()  ◀NEW  ←test: T2A-BRIDGE-UNIT,T2B-BRIDGE-REUSE
│    wire.d.ts                 由 --print-schema 重生成（自动含新命令）        ◀GEN  ←test: T2A-SCHEMA-CHECK
├─ src/ui/participant/         (前端 A)
│    commands.ts               switch(request.command){plan,model} 路由        ◀NEW(Stage A) ←test: T2A-SLASH-UNIT,T2A-MODEL-INT
│    render*.ts                plan 徽标/状态渲染（订阅 plan.*）               ◀NEW(Stage A) ←test: T2A-PLAN-E2E
├─ src/ui/webview/             (前端 B)                                        ◀NEW(Stage B)
│    provider.ts               WebviewViewProvider：CSP nonce/asWebviewUri/localResourceRoots  ←test: T2B-WEBVIEW-E2E
│    protocol.ts               typed postMessage {messageId,channel,done,content} 编解码        ←test: T2B-PROTO-UNIT,T2B-STREAM-UNIT
│    sessionPool.ts            list_sessions{scope:disk} 拉项目历史 + 默认 last-active        ←test: T2B-SCOPE-POOL-INT
│    ownership.ts              sessionId→owner(frontend) 归属表（单活跃，防双驱动）            ←test: T2B-OWNERSHIP-INT
├─ gui/                        React + Vite 独立工程                            ◀NEW(Stage B) ←test: T2B-GUI-UNIT
│    dist/                     构建产物（纳入 VSIX；源码不入包）                              ←test: T2B-PKG-SMOKE
├─ src/ide/VsCodeIde.ts        (P1,复用) diff/编辑落地（webview 仅触发）                       ←test: T2B-DIFF-E2E
└─ scripts/package-vsix.ts     (P1) 暂存清单 += gui/dist                        ◀MOD(Stage B) ←test: T2B-PKG-SMOKE

serve 侧（tomcat/，Rust，单一事实源；本方案定义、不在本任务实现）
├─ src/api/serve/types.rs      ServeCommand::SetPlanMode/ListModels 变体 + 三处 match  ◀NEW ←test: T2A-SERVE-TYPES
├─ src/api/serve/commands.rs   handle：SetPlanMode→plan_runtime.*；ListModels→catalog；get_state+planState；list_sessions{disk}/switch 磁盘会话(SA8) ◀NEW ←test: T2A-SERVE-PLAN-INT,T2A-SERVE-MODEL-INT,T2A-STATE-INT,T2A-SCOPE-LIST-INT,T2A-SCOPE-SWITCH-INT
├─ src/core/session/{manager,scope}  list_sessions/ensure_current_session/switch_current_to_session_id/session_key_for_agent (★已存在,复用)
├─ src/api/serve/control.rs    capabilities += set_plan_mode/list_models             ◀NEW ←test: T2A-CAP-UNIT
├─ src/api/serve/event_pump.rs EVENT_NAMES += plan.*                                 ◀NEW ←test: T2A-PLAN-EVENT-INT
├─ src/core/plan_runtime/mod.rs enter_planning/exit_to_chat/build_plan (★已存在,复用) + event_bus.emit(plan.*) ◀MOD
└─ src/core/llm/catalog.rs     ModelCatalog::entries() (★已存在,复用)
```

> 导读：带 `◀NEW` 的是新建文件/字段，`◀MOD` 是改动，`◀GEN` 是生成物，`(P1,复用)`/`★已存在` 是不动的资产。右侧 `←test:` 把每个节点锚到 §8 测试编号。**最该记住**：serve 侧带 ★ 的几处都是已存在能力（`PlanRuntime` / `ModelCatalog` / `SessionManager`+`scope`），Phase 2 只加"接口层"——尤其会话池复用是把 CLI 早有的"按项目归组 + last-active"暴露给 serve，不是新造。

---

## 6. 配置与会话池/归属

> 专业：Phase 2 新增 1 项扩展配置 `tomcat.ui`；其余沿用 Phase 1（`tomcat.path` 等，见 Phase 1 §6）。总则：env > 扩展设置 > 默认。
> 说人话：只多一个开关——"显示哪个前端"；会话怎么共享/归属见下。

| 变量 | 取值 | 默认 | 含义 | 说人话 |
|------|------|------|------|--------|
| `tomcat.ui`（扩展设置） | `"both"`\|`"participant"`\|`"webview"` | `"both"` | 启用哪些前端；`both`=两者并存 | 想留原生、自建、还是都留。 |
| `tomcat.path`（P1） | string | 自动探测 | tomcat 可执行路径 | 见 Phase 1 §6。 |
| `tomcat.session.defaultCwd`（P1） | string | 工作区根 | 新会话默认 cwd | 见 Phase 1 §6。 |

共享项目 scope 会话池 + 单活跃归属约定（MUST，你已确认 shared_pool）：

- **枚举共享**：participant 与 webview 按**同一** `session_key`（由 cwd+mode 推导，`scope.rs:49`）各自 `list_sessions{scope:"disk"}` 看**同一份**项目历史；与 `tomcat code` 共写同一 `sessions.json`。
- **默认 last-active**：启动默认指向 `current[session_key]`（`ensure_current_session`，serve 已实现），即"上次停留的会话"，**非**自动选 `updatedAt` 最大者。
- **单活跃归属**：同一 `TomcatMessenger` 单实例 + 单 serve 承载所有会话；扩展侧维护 `sessionId → owner(frontend)` 归属表——某会话被一个前端激活成 live 后归属该前端，另一前端对它**只读/提示冲突**；硬保护读 serve `slot.is_busy`。事件按 `sessionId` 路由到 owner 前端渲染（沿用 Phase 1）。
- **释放/接管**：owner 关闭其 tab/线程或 `close_session` 后释放归属，其他前端可接管同一会话。
- `tomcat.ui` 变更经 `onDidChangeConfiguration` 热生效：关闭某前端时注销其视图/参与者并释放其名下归属（不强制 `close_session`，会话仍在池中可被另一前端接管）。

---

## 7. 错误模型 / 截断 / 降级（Phase 2 增量）

> 专业：在 Phase 1 归一化结局（见 Phase 1 §7）之上，补 Stage A/B 新增结局。原则不变：协议错误用 `error` 码归一化；降级只丢中间字。
> 说人话：把 Phase 2 新增的"会出什么岔子、怎么收场"补齐。

```text
plan 状态冲突（如 Executing 下 exit）   → ResponseFrame{success:false, error:"plan_state_conflict"}（扩展：提示当前态，按钮置灰）
/plan build 闸门未过 / 计划不存在        → error:"plan_build_blocked" / "plan_not_found"（扩展：提示先 create_plan）
serve 未支持新命令（旧后端 + 新扩展）    → control_request 子类型走 "unknown_command"；普通命令 serve 回 error，扩展据 capabilities 预先禁用 /plan /model 入口（优雅降级，不报红）
list_models 为空 / 失败                  → 扩展回退读 ~/.tomcat/models.toml（过渡降级）；仍空则提示"未配置模型"
plan.* 事件未上线（仅 transcript）        → 扩展回退轮询 get_state.planState 刷新徽标（功能降级、不阻断）
webview 渲染背压（event 流积压）          → 只丢可重建的 delta 帧(message_update 中间字)；state/审批/生命周期帧必达（对齐 Phase 1 R9）；UI 提示"渲染跟不上"
webview ↔ host 帧 id 失配                 → 丢弃未知 messageId（不崩，记日志）
webview 资源/CSP 加载失败                 → provider 回退错误页 + "重载 webview" 按钮；participant 不受影响
非 owner 前端驱动同一 live 会话           → 扩展侧归属表拦下 → 只读渲染 + "该会话正由另一入口使用"提示；硬保护读 serve slot.is_busy
switch_session 目标不属当前 scope/不存在  → ResponseFrame{success:false, error:"unknown_session"}（扩展：刷新会话池列表）
```

> 说人话：两条新原则——(1) **能力位先行**：扩展握手读 capabilities，旧后端没有 `set_plan_mode`/`list_models` 就直接灰掉入口，不让用户点了报错；(2) **plan 可见性可降级**：事件没上线就退回轮询，功能不至于不可用。

---

## 8. 测试矩阵（验收）

> 专业：每条 §3 交付映射到锁死它的测试；分四层：扩展单元 / spawn 真实 serve 集成 / 真实宿主 E2E / 打包冒烟。Phase 2 尚未实现，状态列统一 `PENDING`（实现后改 `✅日期`）。serve 侧行为以 Tomcat 自带测试为事实源回指。
> 说人话：这些是 Phase 2 的验收清单，现在全是 PENDING，落地一个勾一个。

| 编号 | 维度 | 用例（建议测试函数/文件） | 状态 | 说人话 |
|------|------|---------------------------|------|--------|
| `T2A-MANIFEST` | 清单契约 | `tests/manifest_contract.test.ts`：`chatParticipants[0].commands` 含 `plan`/`model` | PENDING | 命令登记没漏。 |
| `T2A-SLASH-UNIT` | 扩展单元 | `src/ui/participant/tests/slash_routing.test.ts`：`request.command` 分流 + 未知兜底 | PENDING | 命令走对分支。 |
| `T2A-BRIDGE-UNIT` | 扩展单元 | `src/serveClient/tests/plan_model_commands.test.ts`：`sendSetPlanMode`/`sendListModels` 帧组装 + 回执 | PENDING | 发命令格式对。 |
| `T2A-MODEL-INT` | 集成（spawn serve） | `tests/serve_list_models.test.ts`：`list_models` 非空 → `set_model` → `get_state.model` 反映 | PENDING | 真切模型生效。 |
| `T2A-SERVE-TYPES` | serve 单元 | `tomcat` `src/api/serve/types.rs::tests`：新变体 序列化/`wire_type`/`session_id` | PENDING | 协议类型对。 |
| `T2A-SERVE-PLAN-INT` | 集成（spawn serve） | `tests/serve_set_plan_mode.test.ts`：`enter`→planning、`build`→executing、`exit`→chat | PENDING | plan 态真能切。 |
| `T2A-SERVE-MODEL-INT` | serve 集成 | `tomcat` `tests/serve_stdio_e2e.rs`：`list_models` 回 `ModelCatalog::entries()` | PENDING | 后端真吐模型表。 |
| `T2A-STATE-INT` | 集成（spawn serve） | `tests/serve_get_state_planstate.test.ts`：`get_state.planState` 随 plan 切换变 | PENDING | 状态查得到。 |
| `T2A-CAP-UNIT` | serve 单元 | `tomcat` `src/api/serve/control.rs::tests`：capabilities 含新命令 | PENDING | 握手声明对。 |
| `T2A-PLAN-EVENT-INT` | 集成（spawn serve） | `tests/serve_plan_events.test.ts`：`build` 后收 `plan.build` 事件（按 sessionId） | PENDING | plan 进度推得出。 |
| `T2A-PLAN-E2E` | 真实宿主 E2E | `E2E-VSCEXT-2A1`：`@tomcat /plan` 徽标、`/plan build` 转执行中 | PENDING | 真宿主里看得见。 |
| `T2A-SCHEMA-CHECK` | 防漂移门禁 | `npm run check:wire`：`--print-schema` 含新命令、生成物一致 | PENDING | 协议变了能拦下。 |
| `T2A-SCOPE-LIST-INT` | 集成（spawn serve） | `tests/serve_list_scope_sessions.test.ts`：`list_sessions{scope:"disk"}` 回项目 scope 全量历史 + `isCurrent` 标记 | PENDING | 项目历史列得出。 |
| `T2A-SCOPE-SWITCH-INT` | 集成（spawn serve） | `tests/serve_switch_disk_session.test.ts`：`switch_session` 切到未在 registry 的磁盘会话并 hydrate 续聊 | PENDING | 历史会话打得开。 |
| `T2B-PROTO-UNIT` | 扩展单元 | `src/ui/webview/tests/protocol.test.ts`：`{messageId,done,content}` 流式 + 未知 id 丢弃 | PENDING | 消息层稳。 |
| `T2B-STREAM-UNIT` | 扩展单元 | `src/ui/webview/tests/dual_channel.test.ts`：`event` 透传断言 `thinking_delta`/`tool_execution_*`/`ask_question` 各命中对应 UI 元素；重连 `state` 快照幂等覆盖 | PENDING | 富 UI 与 vscode chat 等价 + 双通道对。 |
| `T2B-GUI-UNIT` | 扩展单元 | `gui/src/**/*.test.tsx`：消息/工具/审批组件渲染 | PENDING | 界面组件对。 |
| `T2B-BRIDGE-REUSE` | 扩展集成 | `tests/dual_frontend_share.test.ts`：单 messenger 服务两前端，核心无 diff | PENDING | 底层真没改。 |
| `T2B-SCOPE-POOL-INT` | 扩展集成 | `tests/session_scope_pool.test.ts`：webview 列项目 scope 历史、默认选 `isCurrent`；与 participant 看同一份 | PENDING | 两入口看同一份项目历史。 |
| `T2B-OWNERSHIP-INT` | 扩展集成 | `tests/session_ownership.test.ts`：同一会话第二前端驱动被拒/只读；owner 释放后可接管 | PENDING | 同一会话不双驱动。 |
| `T2B-MULTISESSION-E2E` | 真实宿主 E2E | `E2E-VSCEXT-2B1`：webview 2 tab → 2 会话不串台 | PENDING | 多 tab 真隔离。 |
| `T2B-DIFF-E2E` | 真实宿主 E2E | `E2E-VSCEXT-2B2`：webview 触发应用编辑走 Phase 1 IDE | PENDING | 改文件还是稳那套。 |
| `T2B-WEBVIEW-E2E` | 真实宿主 E2E | `E2E-VSCEXT-2B3`：webview 流式渲染、CSP 无报错 | PENDING | 面板真能用。 |
| `T2B-PKG-SMOKE` | 打包冒烟 | `tests/package_vsix_smoke.test.ts`：VSIX 含 `gui/dist`、不含 `gui/` 源码 | PENDING | 打包不漏不肥。 |
| 协议事实源（回指） | serve | `tomcat` `src/api/serve/*::tests`、`tests/serve_stdio_e2e.rs` | ✅（既有）+ PENDING（新增分支） | 协议行为 Tomcat 自测。 |

补充口径：

1. **Stage A 强门禁** = `T2A-SERVE-PLAN-INT` + `T2A-MODEL-INT`：真起 serve 能切 plan、能切模型，是 Stage A 成立的硬条件。
2. **Stage B 强门禁** = `T2B-BRIDGE-REUSE` + `T2B-OWNERSHIP-INT`：桥接核心零改动 + 共享会话池下同一会话单前端归属（不双驱动），是 Stage B 成立的硬条件。

---

## 9. 风险与应对

> 专业：覆盖协议演进 / proposed 合规 / webview 安全与性能 / plan 持久化 / 双前端并发。应对落到具体动作。
> 说人话：Phase 2 最可能翻车的点 + 具体怎么防。

| 风险 | 影响 | 应对（具体动作） | 说人话 |
|------|------|--------------------|--------|
| 误用 proposed API | 高 | 全程稳定 API：slash=`contributes.chatParticipants[].commands`、webview=`WebviewViewProvider`/`contributes.views`；CI lint 扫 `vscode.proposed.*`/`enabledApiProposals` 即失败 | 一碰 proposed 就 CI 拦。 |
| serve 协议漂移（新命令） | 高 | 类型由 `--print-schema` 生成；`check:wire` 校验；TS 编译期暴露字段变更 | 协议变编译就红。 |
| plan 事件未经 event_bus（现状） | 高 | Stage A 必须在 `PlanRuntime` 动作处补 `event_bus.emit`，否则白名单加了也收不到；扩展侧 `get_state` 轮询兜底 | 别只加白名单忘了真发事件。 |
| 旧后端 + 新扩展不兼容 | 中 | 扩展读 capabilities 决定是否启用 `/plan` `/model` 入口；缺则灰掉、不报错 | 后端没升级就别显示新功能。 |
| webview CSP / 安全审查不过 | 中 | nonce + `default-src 'none'` + `asWebviewUri` + `localResourceRoots`（学 cline）；禁 `unsafe-inline` | 按 cline 安全基线加载。 |
| webview 流式背压卡顿 | 中 | 双通道只丢可重建 delta(message_update 中间字)；state/审批/生命周期帧必达；`retainContextWhenHidden` 防重建 | 卡了只丢中间字。 |
| 双前端驱动同一会话竞争 | 中 | 共享池但单活跃归属：扩展侧 `sessionId→owner` 映射 + serve `slot.is_busy` 硬保护；非 owner 只读；`T2B-OWNERSHIP-INT` 锁死 | 历史共享，但同一对话同一时刻只一个面板能跑。 |
| 会话池与 `tomcat code` 并发写 sessions.json | 低 | 复用既有 `SessionManager` 原子写 + advisory（Phase 1/CLI 已用）；扩展不直接写文件，只发命令 | 写会话表的活全交后端，别两头乱写。 |
| plan 持久化与会话 | 中 | plan 状态以 serve/`PlanRuntime` + `~/.tomcat/plans/` 为准（盘是真相）；扩展不缓存 plan 业务态，只读 `get_state`/事件 | plan 真相在后端盘上，前端不自作主张。 |
| webview 产物撑大 VSIX | 低 | gui 独立构建仅 `dist` 入包；`.vscodeignore` 排源码/`node_modules`；`package_vsix_smoke` 守门 | 只打产物不打源码。 |
| VSCode 版本兼容 | 低 | `engines.vscode` 维持 Phase 1 稳定基线；仅用稳定 d.ts 符号 | 基线不抬高。 |

---

## 10. 历史决策 / 跨文档修订

被取代 / 否决的方案（留痕）：

- ~~用 "Configure custom agents" 接 `/plan` `/model`~~ → 否：`.agent.md` 模式系统，动态注册 `registerCustomAgentProvider` 为 proposed（`vscode.proposed.chatPromptFiles.d.ts:472`），第三方不可用，且 `target` 不含"绑定 `@participant`"，接进去也驱动不了 Tomcat loop。证据见 [`01-scope-and-research.md`](01-scope-and-research.md) §2.1。
- ~~把 `"/plan"` 当普通 `prompt` 文本发给 serve~~ → 否：serve 不解析 slash，`"/plan"` 会被原样喂给 LLM；plan 模式只能由 `PlanRuntime` 方法切换，故必须新增 `set_plan_mode` 命令。
- ~~复用 `new_session.params.mode` 表达 plan~~ → 否：`ServeSessionMode` 只有 `Code`/`Claw`（`types.rs:53`），是会话 scope，与 PlanState 正交，语义不可混用。
- ~~把 Tomcat 模型注册进原生模型选择器（`languageModelChatProviders`）作为 `/model` 主路径~~ → 否（暂）：接近已否决"形态 B"，模型选择会落进 Copilot 路由、与 `@tomcat` 语义割裂；Phase 2 用自带 `list_models`+QuickPick。该贡献点仍是稳定 API，留作未来可选增强。
- ~~webview 用 gRPC-over-postMessage（cline ProtoBus）~~ → 否：引入 proto 编译/代码生成工具链，对单扩展偏重；采 continue 式轻量 `{messageId,done,content}` typed postMessage。
- ~~participant 与 webview 二选一（webview 取代原生入口）~~ → 否：浪费 Phase 1 已交付的原生参与者；改为默认并存，`tomcat.ui` 可选关其一。
- ~~两前端各持独立 sessionId 命名空间、互不可见（完全隔离）~~ → 否（已修订）：与用户要的"按项目 scope 归组历史 + 默认恢复 last-active"（仿 `tomcat code`）相悖；改为**共享同一项目 scope 会话池**（枚举共享、默认 last-active），仅"激活态"单前端归属，复用 `SessionManager::list_sessions`/`ensure_current_session`/`switch_current_to_session_id`。证据见 [会话作用域调研](0da17338-d61a-4957-8d1e-471e2e62d2f3)。
- ~~两前端同时驱动同一条 live 会话~~ → 否：会互抢 busy/turn；改为单活跃归属（owner 唯一，非 owner 只读），serve `slot.is_busy` 兜底。

跨文档修订：

- Phase 1 主文档 [`../tomcat-vscode-extension.md`](../tomcat-vscode-extension.md) 已加指向本 Phase 2 方案的链接；Phase 1 子文档内容不改。
- 本方案定义的 serve 协议扩展（`set_plan_mode`/`list_models`/`get_state.planState`/`plan.*`）落地后，Tomcat 仓 [`agent-server-and-ui-gateway.md`](../../../../tomcat/docs/architecture/agent-server-and-ui-gateway.md) 的 wire 变更应回链本文 §4 更新生成物与字段表。
- 实现阶段另起任务卡 + 研发计划（仿 Phase 1 `T2-P1-019`），本组文档为其唯一事实来源。

