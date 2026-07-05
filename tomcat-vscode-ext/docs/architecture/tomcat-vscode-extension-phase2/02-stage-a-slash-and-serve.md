# Tomcat VSCode 扩展 · Phase 2 · 02 Stage A：slash 命令 + serve 后端扩展

> 总览见 [`../tomcat-vscode-extension-phase2.md`](../tomcat-vscode-extension-phase2.md)（含定位、阅读顺序与文首导图集）。
> 本文对应 [`ARCHITECTURE_SPEC.md`](../../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) 的 **§3 落地选型与实施**（Stage A 部分）：§3.1 七列决策表（SA1–SA7）+ §3.2 五列实施点表。
> 能力裁决证据见 [`01-scope-and-research.md`](01-scope-and-research.md) §2.1/§2.2；协议字段表见 [`04-protocol-runtime.md`](04-protocol-runtime.md) §4。
> 单一事实源：新命令落点 `tomcat/src/api/serve/types.rs`（`ServeCommand`）、`commands.rs`（handler）、`control.rs`（capabilities）、`event_pump.rs`（事件白名单）；plan 行为 `tomcat/src/core/plan_runtime/mod.rs`；模型目录 `tomcat/src/core/llm/catalog.rs`。

---

## 3. Stage A：把 `/plan`、`/model` 接进原生聊天

> 专业：Stage A 是「扩展侧轻、后端侧重」的一阶段。扩展侧只新增两条 slash 命令路由（稳定 API）；后端侧给 `tomcat serve` 补两条命令 + 一个状态字段 + 一组事件，把早已存在的 `PlanRuntime` / `ModelCatalog` 暴露到 wire 协议。
> 说人话：`/plan` `/model` 在 VSCode 里加按钮很简单，难的是让 `tomcat serve` 听得懂这俩命令——所以这阶段大半工作量在 Rust 后端。

### 3.0 Stage A 一图概览

```text
用户在原生聊天输入 "@tomcat /plan"
        │ VSCode 解析 slash → request.command="plan"
        ▼
ui/participant/commands.ts            ┌─ /plan  ─► messenger.sendSetPlanMode({action:"enter"})
  switch(request.command){            ├─ /plan exit ─► sendSetPlanMode({action:"exit"})
    "plan":  …                ────────┤─ /plan build ─► sendSetPlanMode({action:"build",planId?})
    "model": …                        └─ /model ─► sendListModels() → showQuickPick → sendSetModel
  }
        │  NDJSON 命令帧
        ▼
tomcat serve (Rust)
  commands.rs handle_command:
    ServeCommand::SetPlanMode{action} → plan_runtime.{enter_planning|exit_to_chat|build_plan}()
    ServeCommand::ListModels          → ModelCatalog::entries() → ResponseFrame.payload.models[]
    ServeCommand::GetState            → payload 增 planState（读 plan_runtime.mode()）
  event_pump.rs: plan.* 事件经 event_bus → OutFrame::Event → 扩展刷新 plan 徽标
```

> 导读：左半边是扩展（稳定 slash），右半边是 serve（新增命令）。`/plan` 命令最终落到 `PlanRuntime` 的三个**已存在**方法上，serve 只是"翻译官"。`/model` 则是"先列后选再切"三步。

### 3.1 决策表（SA1–SA7）

> 列含义：维度｜关切（要解决什么）｜决策（结论）｜取自（本仓 + 外部证据）｜入选理由｜未入选 + 拒因｜说人话。

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
|------|------|------|------|----------|----------------|--------|
| **SA1** 接入入口 | `/plan` `/model` 在原生聊天的入口形态 | participant slash command（`contributes.chatParticipants[].commands` + `request.command` 分流） | 本仓 Phase 1 `package.json` 已声明 participant；vscode `vscode.d.ts:19866`、`chatParticipant.contribution.ts:149`；cline/continue 均用 participant/命令 | 全稳定、可上架、与 `@tomcat` 同进同出 | ① "Configure custom agents"：`.agent.md` 模式 + `registerCustomAgentProvider` proposed、绑不到 participant（`vscode.proposed.chatPromptFiles.d.ts:472`）→ 拒；② 顶层 command palette：脱离聊天上下文、拿不到 `request` → 拒 | 就用 `@tomcat /plan` 这种聊天内命令，别碰那个需要特权的自定义 agent 菜单。 |
| **SA2** plan 驱动通道 | serve 当前无法被驱动进 plan 模式 | 新增 serve 命令 `set_plan_mode`，桥接 `PlanRuntime` | 本仓 `plan_runtime/mod.rs:340/361/939`（三方法现成）、`cmd_plan.rs:53`（CLI 同款分发）；现状 `types.rs:90`（命令枚举无 plan） | plan 引擎已实现，仅缺 wire 入口；与 CLI 行为同源 | ① 把 `"/plan"` 当 `prompt` 文本发：serve 不解析 slash，会被原样喂给 LLM → 拒；② 复用 `new_session.mode`：`ServeSessionMode` 只有 `Code`/`Claw`，无 plan 语义（`types.rs:53`）→ 拒 | 不能把 "/plan" 当聊天内容发，得给后端开一条真命令。 |
| **SA3** `set_plan_mode` 形态 | 单命令 vs 三命令 | 单命令 `set_plan_mode{action:"enter"\|"exit"\|"build", planId?}` | 本仓 `plan_runtime/mod.rs`（enter/exit/build 三方法天然对应一个 action 集）；wire 风格对齐现有 `set_model`（`types.rs:128`） | 少一堆 capability 噪声；扩展只记一个命令名；action 与 PlanRuntime 方法一一映射 | 三个独立命令（`enter_plan`/`exit_plan`/`build_plan`）：capabilities/handler/类型三处都要 ×3，收益为零 → 拒 | 一个命令带个"动作"参数，比开三个命令清爽。 |
| **SA4** 模型枚举 | 扩展拿不到"有哪些模型" | 新增 serve 命令 `list_models`，回 `ModelCatalog::entries()` | 本仓 `catalog.rs:137`（`entries()` 现成）、`control.rs:35`（capabilities 无 list_models） | 单一事实源在后端；与 `set_model` 成对；过渡期可降级 | 扩展直接读 `~/.tomcat/models.toml`：绕过后端目录合并逻辑、内置模型读不到、易漂移 → 仅作过渡降级，不作主路径 | 让后端把模型清单吐出来，别让前端自己猜文件。 |
| **SA5** plan 态暴露 | 扩展需读当前 PlanState 刷 UI | 扩展现有 `get_state`，payload 增 `planState` 字段 | 本仓 `commands.rs:220`（get_state 现成）、`:233`（现 `mode` 是 scope，不是 PlanState）、`plan_runtime/mod.rs:332`（`mode()`） | 复用现成命令，不加新 capability；一次 `get_state` 拿全 | 新增 `get_plan_state` 命令：与 `get_state` 职责重叠、扩展要多发一发 → 拒 | 现成的"问状态"命令里多塞一个字段就行，不另开命令。 |
| **SA6** plan 进度可见性 | plan 生命周期当前不在事件流 | `plan.*` 进 `event_pump` 白名单 + `PlanRuntime` 经 `event_bus` emit；扩展实时刷新；`get_state` 轮询兜底 | 本仓 `event_pump.rs:14`（白名单无 plan.*）、`plan_runtime/mod.rs:1088`（现仅 `write_transcript_custom` 落 transcript） | 实时性好；与现有事件管道同构（按 sessionId 过滤） | 仅靠 `get_state` 轮询：延迟高、徽标抖动；纯 transcript：扩展看不到 → 事件为主、轮询兜底 | 让 plan 进度也走"推送"，而不是前端一直问。 |
| **SA7** `/model` UI 形态 | 选模型用什么控件 | `window.showQuickPick`（扩展自带列表，源自 SA4 的 `list_models`） | 本仓 Phase 1 已用 QuickPick 做 ask_question fallback；vscode `vscode.d.ts:20847`（`registerLanguageModelChatProvider` 稳定但偏重） | 轻量、不绕 Tomcat agent loop、与 ask_question UI 同款 | 把 Tomcat 模型注册进原生模型选择器（`languageModelChatProviders`）：接近已否决"形态 B"，且模型选择会落进 Copilot 路由，与 `@tomcat` 语义割裂 → Phase 2 不走 | 选模型就弹个列表让你挑，别去改 VSCode 全局模型菜单。 |
| **SA8** 项目 scope 会话复用 | serve 能否复刻 `tomcat code` 的"按项目归组历史 + 默认恢复 last-active" | **复用**：serve `list_sessions` 增"磁盘 scope 全量历史"维度；`switch_session` 支持打开磁盘历史会话；启动恢复 `current` 指针（已实现） | 本仓 `scope.rs:49`（`session_key_for_agent`=git 项目根 hash）、`session_impl.rs:380`（`list_sessions` 按 updated_at 倒序）、`:313`（`ensure_current_session`）、`:321`（`switch_current_to_session_id`）；现状 serve `commands.rs:205`（list 仅 registry）/`:150`（switch 仅 registry） | store 层能力现成、与 `tomcat code` 共享同一 `sessions.json`；零造轮子；正好喂 Stage B 多会话 tab | ① serve 各前端各自 `new_session` 完全隔离：webview 看不到 code/participant 的项目历史，丢掉用户要的"项目归组" → 拒；② 扩展侧自己扫 `sessions.json`：绕开 session_key 推导与并发写保护 → 拒 | 让插件也能像 code 那样"列出本项目历史对话、默认接着上次那条"。 |

### 3.2 实施点（五列）

> 列含义：实施点｜交付范围（可观察行为）｜主要代码落点｜验收锚点（→ [`04-protocol-runtime.md`](04-protocol-runtime.md) §8 测试编号）｜说人话。
> 落点分「扩展侧（TS，本仓 `tomcat-vscode-ext/`）」与「serve 侧（Rust，`tomcat/`）」。

| 实施点 | 交付范围 | 主要代码落点 | 验收锚点 | 说人话 |
|--------|----------|--------------|----------|--------|
| **E-A1** slash 声明 | `@tomcat /plan`、`@tomcat /model` 出现在聊天命令列表 | 扩展 `package.json` `contributes.chatParticipants[0].commands += [{name:"plan",description},{name:"model",description}]` | `T2A-MANIFEST` | 在清单里把两条命令登记上。 |
| **E-A2** slash 路由 | handler 按 `request.command` 分流 plan/model，未知命令兜底 | 扩展 `src/ui/participant/commands.ts`：`switch(request.command)` | `T2A-SLASH-UNIT` | 收到 plan/model 就走对应分支。 |
| **E-A3** `/model` 选择器 | `list_models` → QuickPick → `set_model` → 气泡确认；`get_state` 标记当前模型 | 扩展 `commands.ts` + `serveClient/TomcatMessenger.ts::sendListModels/sendSetModel` | `T2A-MODEL-INT` | 弹列表选模型、切完给个确认。 |
| **E-A4** plan 状态徽标 | `/plan` 后聊天显示「计划模式」徽标；`plan.*` 事件/`get_state.planState` 驱动刷新；`build` 后转「执行中」 | 扩展 `commands.ts` + `src/ui/participant/render*.ts`（监听 `plan.*`） | `T2A-PLAN-E2E` | 进了计划模式聊天上能看出来，开跑后变"执行中"。 |
| **E-A5** 桥接薄包装 | `sendSetPlanMode({action,planId?})`、`sendListModels()`；复用 Phase 1 request/response 回环 | 扩展 `src/serveClient/TomcatMessenger.ts`（**仅加方法，不改核心**） | `T2A-BRIDGE-UNIT` | 给桥接层加两个发命令的小函数。 |
| **S-A1** 命令类型 | `ServeCommand::SetPlanMode{id?,sessionId?,action,planId?}`、`ListModels{id?}` | serve `src/api/serve/types.rs`（enum + `wire_type`/`command_id`/`session_id` 三处 match 补齐） | `T2A-SERVE-TYPES` | 在 Rust 协议枚举里加两个新命令。 |
| **S-A2** 命令分发 | handler：`SetPlanMode`→`plan_runtime.{enter_planning\|exit_to_chat\|build_plan}`；`ListModels`→`ModelCatalog::entries()`→`ResponseFrame.payload.models[]` | serve `src/api/serve/commands.rs`（新增 match 分支；resolve session 后取 `slot.ctx.session_runtime.plan_runtime`） | `T2A-SERVE-PLAN-INT`、`T2A-SERVE-MODEL-INT` | 收到命令就调早写好的 plan/模型逻辑。 |
| **S-A3** 能力位 | `initialize` capabilities += `"set_plan_mode"`、`"list_models"` | serve `src/api/serve/control.rs:35`（capabilities 数组） | `T2A-CAP-UNIT` | 握手时告诉扩展"我支持这俩新命令"。 |
| **S-A4** 状态字段 | `get_state` payload 增 `planState`（`chat\|planning\|executing\|pending\|completed` + 可选 `planId`） | serve `src/api/serve/commands.rs:220`（get_state 分支读 `plan_runtime.mode()`） | `T2A-STATE-INT` | 问状态时把 plan 态一起带回来。 |
| **S-A5** 事件白名单 | `plan.*`（create/build/update/review/verify/complete）进 `EVENT_NAMES`；`PlanRuntime` 经 `event_bus` emit（当前仅写 transcript） | serve `src/api/serve/event_pump.rs:14` + `src/core/plan_runtime/mod.rs`（emit 点）+ `src/infra/events`（wire 常量复用） | `T2A-PLAN-EVENT-INT` | 把 plan 进度也推到事件流上。 |
| **S-A6** schema 重生成 | `tomcat serve --print-schema` 含新命令；扩展 `wire.d.ts` 刷新；CI 校验一致 | serve `src/api/serve/schema.rs`（自动随 `JsonSchema` 派生）+ 扩展 `scripts/gen-wire.ts` | `T2A-SCHEMA-CHECK` | 协议一加命令，生成的 TS 类型自动跟上。 |
| **S-A7** 项目 scope 会话枚举/切换 | `list_sessions` 增 `scope:"disk"` 维度回磁盘 scope 全量历史；`switch_session` 支持打开磁盘历史会话（不在 registry 也能切） | serve `src/api/serve/commands.rs:205`（list 接 `SessionManager::list_sessions()`）、`:150`（switch 组合 `switch_current_to_session_id`+重建 `SessionSlot`+`registry.insert`，即新增 `open_existing_session(id)` 组合） | `T2A-SCOPE-LIST-INT`、`T2A-SCOPE-SWITCH-INT` | 让 serve 能列项目历史、能打开历史会话。 |

---

## 3.3 扩展侧设计（前端 A：原生 participant slash）

> 专业：扩展侧不持有 plan/model 业务状态，纯路由 + 渲染。命令分流读稳定的 `request.command`；UI 状态来自 `get_state` 与 `plan.*` 事件。
> 说人话：扩展只管"把命令转给后端、把后端的状态画出来"，不自己记 plan 进度。

`/plan` 子命令映射（与 CLI `cmd_plan.rs` 同源，但走 serve 命令而非 REPL 分发）：

```text
@tomcat /plan              → sendSetPlanMode({action:"enter"})
@tomcat /plan exit         → sendSetPlanMode({action:"exit"})
@tomcat /plan build [id]   → sendSetPlanMode({action:"build", planId:id?})
（/plan list 可选：扩展侧暂不实现，留待 webview 的 plan 列表面板，见 03 §3）
```

`/model` 三步流（伪代码，表达顺序：专业 → 说人话 → 伪代码）：

```text
专业：list_models → showQuickPick(当前模型置顶/打勾) → set_model → get_state 复核 → 渲染确认气泡。
说人话：列出来、让你挑、切过去、再确认一下。
```

```ts
// src/ui/participant/commands.ts（示意，非最终实现）
async function handleModelCommand(req, stream, messenger) {
  const { models } = await messenger.sendListModels();        // SA4
  const cur = (await messenger.sendGetState(req.sessionId)).model;
  const picked = await vscode.window.showQuickPick(
    models.map(m => ({ label: m.id, description: m.id === cur ? "当前" : "" })),
    { title: "选择 Tomcat 模型" });
  if (!picked) return;                                         // 用户取消，无副作用
  await messenger.sendSetModel(req.sessionId, picked.label);   // 复用 Phase 1 set_model
  stream.markdown(`已切换模型：\`${picked.label}\``);
}
```

plan 徽标刷新：handler 发完 `set_plan_mode` 后读 `ResponseFrame.payload.planState` 立即更新；同时订阅 `plan.*` 事件做后续推进刷新（执行中→完成），轮询 `get_state.planState` 作兜底。

## 3.4 serve 后端扩展（层 3，本方案定义协议，不在本任务实现）

> 专业：后端改动集中在 `api/serve/*`，把 `core/plan_runtime` 与 `core/llm/catalog` 暴露成 wire 命令。`PlanRuntime` 的方法签名与状态机**不变**（见单一事实源），serve 仅做"命令 → 方法"的桥接与错误归一化。
> 说人话：不动 plan 引擎本体，只给它装个"对外接口"。

### 3.4.1 `set_plan_mode` → PlanRuntime

| action | 调用（`slot.ctx.session_runtime.plan_runtime`） | 成功 payload | 失败（归一化为 `ResponseFrame.error`） | 说人话 |
|--------|------------------------------------------------|--------------|----------------------------------------|--------|
| `enter` | `enter_planning()`（`mod.rs:340`） | `{planState:"planning"}` | `AlreadyInMode`→`plan_already_in_mode`；非法态→`plan_state_conflict` | 进计划模式。 |
| `exit` | `exit_to_chat()`（`mod.rs:361`，`Executing` 拒） | `{planState:"chat"}` | `NotInPlanning`/`AlreadyInMode`→`plan_state_conflict` | 退回聊天。 |
| `build` | `build_plan(target, sessionId)`（`mod.rs:939`）；`target` 缺省走 `default_build_target()`（`mod.rs:901`） | `{planState:"executing", planId, planPath}` + 自动注入 `"start building <path>"` prompt（对齐 CLI `cmd_plan.rs:105` 的 `Continue` 语义） | `BuildBlocked`/`BuildPlanNotFound`→`plan_build_blocked`/`plan_not_found` | 开跑执行计划。 |

> 注：`build` 的"自动开跑"在 CLI 由 `ChatCommandOutcome::Continue{line:"start building …"}` 实现（`cmd_plan.rs:105`）。serve 侧应在 `build_plan` 成功后，按相同语义把该行作为 user turn 注入会话（复用现有 `prompt` 链路），保证 plan 行为与 CLI 一致。`session_id` 取 resolve 后的会话 id（`build_plan` 第二参数）。

### 3.4.2 `list_models` → ModelCatalog

- handler 调 `ModelCatalog::entries()`（`catalog.rs:137`），映射为 `[{id, ...}]` 写入 `ResponseFrame.payload.models`。
- 当前模型仍由 `get_state.model` 反映（`commands.rs:235`），扩展据此在 QuickPick 标"当前"。
- 字段细节见 [`04-protocol-runtime.md`](04-protocol-runtime.md) §4。

### 3.4.3 `get_state.planState`

- 现 `get_state` 回的 `mode` 是会话 scope（`code`/`claw`，`commands.rs:233`），**保持不变**；新增 `planState` 字段读 `plan_runtime.mode()`（`PlanState::as_str()`），`Executing/Pending/Completed` 同时回 `planId`。
- 与 scope `mode` 正交：一个答"代码/claw 会话"，一个答"plan 生命周期态"。

### 3.4.4 `plan.*` 事件上线

- `event_pump.rs::EVENT_NAMES`（`:14`）追加 `WIRE_PLAN_*`（create/build/update/review/verify/complete）。
- **关键前提**：当前 `PlanRuntime` 只经 `write_transcript_custom` 落 transcript（如 `mod.rs:1088` 的 `WIRE_PLAN_BUILD`），**未经 `event_bus` emit**；Stage A 须在 `PlanRuntime` 对应动作处补一次 `event_bus.emit`（带 `sessionId`），事件才会被 `event_pump` 按会话过滤转发。否则 `EVENT_NAMES` 加了也收不到事件。
- 事件帧形态对齐现有 `OutFrame::Event`（`types.rs:423`），扩展侧渲染同 Phase 1 事件管道。

### 3.4.5 项目 scope 会话池复用（SA8 落地）

> 专业：复用 `tomcat code` 已有的"按 git 项目根归组历史 + 默认恢复 last-active"。底层 `SessionManager` API 现成，serve 只需把 registry-only 的 `list_sessions`/`switch_session` 桥到磁盘 scope 层；启动恢复 `current` 指针 serve **已实现**（`mod.rs` 走 `ensure_current_session`）。
> 说人话：让插件也能像 `tomcat code` 那样"列出本项目历史会话、默认接着上次那条"，而且看的是同一份 `sessions.json`。

现状与缺口（证据见 [会话作用域调研](0da17338-d61a-4957-8d1e-471e2e62d2f3)）：

| 能力 | 底层 API（现成） | serve 现状 | Stage A 需补 |
|------|------------------|------------|--------------|
| cwd → 项目 scope key | `session_key_for_agent`（`scope.rs:49`，Code 模式=git 根 hash） | 已用（`mod.rs` 建 slot 时算） | 无 |
| 列项目 scope 全量历史 | `SessionManager::list_sessions()`（`session_impl.rs:380`，updated_at 倒序） | `list_sessions` 只列 registry live slot（`commands.rs:205`） | `list_sessions` 增磁盘 scope 维度 |
| 默认恢复 last-active | `current[session_key]` 指针 + `ensure_current_session`（`session_impl.rs:188/313`） | **已实现**（启动恢复一条 current） | 无（仅在协议/状态里暴露 `isCurrent`） |
| 打开磁盘历史会话 | `switch_current_to_session_id`（`session_impl.rs:321`） | `switch_session` 只切 registry 内 slot（`commands.rs:150`） | 新增"磁盘会话→建 slot→注册"组合（`open_existing_session(id)`） |

约束：

- **枚举共享、激活单归属**：两前端按同一 `session_key` 看同一份历史列表；但同一条会话被某前端激活成 live（建 SessionSlot）后，另一前端对它只读或提示冲突——硬保护落在 serve `slot.is_busy`，软归属落在扩展侧 `sessionId→owner` 映射（详见 [`04`](04-protocol-runtime.md) §6/§9）。
- **`session_key` 才是归组键**，不是裸 cwd（同 repo 子目录共享同一 key）；Claw 模式固定 `agent:main:main`，与 cwd 无关。
- 字段（`list_sessions` 回 `{sessionId,updatedAt,isCurrent,busy}` 等）见 [`04`](04-protocol-runtime.md) §4。

---

## 验收锚点（汇总，详见 04 §8）

| 锚点编号 | 验收内容 | 层 |
|----------|----------|----|
| `T2A-SLASH-UNIT` | `request.command==="plan"/"model"` 正确分流；未知命令兜底 | 扩展单元 |
| `T2A-MODEL-INT` | spawn 真实 serve：`list_models` 回非空 + `set_model` 切换 + `get_state.model` 反映 | 扩展集成 |
| `T2A-SERVE-PLAN-INT` | spawn 真实 serve：`set_plan_mode{enter}`→`get_state.planState=="planning"`；`{build}`→`"executing"`；`{exit}`→`"chat"` | serve 集成 |
| `T2A-PLAN-EVENT-INT` | `build` 后收到 `plan.build` 事件（经 event_pump，按 sessionId 过滤） | serve 集成 |
| `T2A-PLAN-E2E` | 真实宿主：`@tomcat /plan` 进计划态徽标、`/plan build` 转执行中 | 真实宿主 E2E |
| `T2A-CAP-UNIT` | `initialize` capabilities 含 `set_plan_mode`/`list_models` | serve 单元 |
| `T2A-SCHEMA-CHECK` | `--print-schema` 含新命令；`npm run check:wire` 一致 | 防漂移门禁 |
| `T2A-SCOPE-LIST-INT` | spawn 真实 serve：`list_sessions{scope:"disk"}` 回当前项目 scope 全量历史（标 `isCurrent`） | serve 集成 |
| `T2A-SCOPE-SWITCH-INT` | spawn 真实 serve：`switch_session` 切到磁盘历史会话（未在 registry）并能 hydrate 续聊 | serve 集成 |

> 说人话：Stage A 验收的硬门禁是「**真起一个 serve，发 `set_plan_mode` 真能把 PlanState 切过去并能在 `get_state` 读回**」——这条过了，扩展侧的 slash 路由就是水到渠成。
