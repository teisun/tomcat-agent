# Tomcat VSCode Chat 扩展 · Phase 2 技术方案：slash 命令补全（/plan·/model）与自建 Webview UI

> 适用范围：在 Phase 1（`@tomcat` 原生 Chat Participant + UI 无关桥接核心，详见 [`tomcat-vscode-extension.md`](tomcat-vscode-extension.md)）已上架可用的基础上，分两阶段把 Tomcat 更完整的能力接入 VSCode：**Stage A** 用稳定 slash command 把 `/plan`（计划模式）、`/model`（切换模型）接进原生聊天，并为此**扩展 `tomcat serve` 后端协议**；**Stage B** 在同一桥接核心上自建 **React + Vite Webview**，做富交互前端。**两个前端默认并存**，共享单个 `tomcat serve` 进程，并**共享同一项目 scope 的会话池**（复用 `tomcat code` 的"按 git 项目根归组 + 默认恢复 last-active"逻辑）；同一条 live 会话同时只允许一个前端驱动（单活跃归属）。全程只用 VSCode **稳定 API**，不依赖任何 proposed API。
> 上位规范：[`ARCHITECTURE_SPEC.md`](../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)。本方案按规范 §1–§10 拆为「总览（本文）+ 4 篇子文档」，文首「方案导图集」置于子文档之前、不占用 § 编号。
> 与 Phase 1 关系：Phase 1 = 「桥接核心（层 2）+ 原生 participant（层 1-A）」已交付；Phase 2 **不重写桥接核心**，只新增「serve 后端能力 + slash 命令 + webview 前端（层 1-B）」。Phase 1 文档为本文的事实基线。
> 单一事实源：协议与类型仍以 `tomcat/src/api/serve/types.rs` + `tomcat/src/infra/events/mod.rs` 为准；plan 模式行为以 `tomcat/src/core/plan_runtime/mod.rs` 为准；本组文档只描述「扩展侧如何消费 + serve 侧需补什么」。
> 外部参考仓库（与本仓同级，位于 `/Users/yankeben/workspace/`，仅作证据引用、不进本仓）：`vscode/`（VSCode 本体）、`cline/`、`continue/`。

**一句话定位**：Phase 1 让 Tomcat 当后端、原生聊天当前端跑通了「聊天/工具/审批/多会话」；Phase 2 补两件事——**(1) 把 Tomcat 独有的 `/plan`、`/model` 能力也接进来（瓶颈在 serve 后端缺命令，先补后端再接 UI）；(2) 加一个自建 Webview 富前端，与原生 participant 并存**，桥接核心两阶段 100% 复用。

---

## 子文档索引

本方案按 ARCHITECTURE_SPEC §1–§10 拆分；下表给出「子文档 ↔ 规范 §」对应关系。建议先读本文「文首导图集」建立心智模型，再按需下钻。

| 子文档 | 覆盖规范 § | 内容 | 何时读 |
|--------|------------|------|--------|
| [`01-scope-and-research.md`](tomcat-vscode-extension-phase2/01-scope-and-research.md) | §1 术语 · §2 竞品调研 | Phase 2 新术语（slash 命令 / 计划模式 / 模型目录 / 双前端 / webview 协议）；VS Code「能不能接、怎么接」能力裁决（含 vscode 源码证据）；cline/continue webview 实证横向表 | 想搞清"为什么用 slash 而非 Configure custom agents、webview 怎么学 cline/continue"先读它。 |
| [`02-stage-a-slash-and-serve.md`](tomcat-vscode-extension-phase2/02-stage-a-slash-and-serve.md) | §3 落地选型与实施（Stage A） | §3.1 七列决策表（SA1–SA7）+ §3.2 五列实施点 + Stage A 拆节：扩展侧 slash 路由 + **Tomcat serve 后端扩展**（`set_plan_mode`/`list_models`/`get_state.planState`/`plan.*` 事件） | 要把 `/plan` `/model` 落地时按它对协议、按它改后端。 |
| [`03-stage-b-webview.md`](tomcat-vscode-extension-phase2/03-stage-b-webview.md) | §3 落地选型与实施（Stage B） | §3.1 七列决策表（SB1–SB8）+ §3.2 五列实施点 + Stage B 拆节：双前端并存模型、webview 宿主/CSP、双通道 postMessage 协议、IDE 抽象与 diff、模型/plan/多会话 UI、打包影响 | 要自建 webview 时按它分层、对协议、建文件。 |
| [`04-protocol-runtime.md`](tomcat-vscode-extension-phase2/04-protocol-runtime.md) | §4 协议 · §5 One-Glance · §6 配置 · §7 错误 · §8 测试 · §9 风险 · §10 历史 | 新 serve 命令/字段表 + webview 帧表 + jsonc 样例；扩展侧文件职责框图；配置/错误/测试矩阵/风险/否决留痕 | 实现/验收/排错时查它。 |

---

## 文首导读：方案导图集

### 阅读顺序建议（说人话）

1. **A.1 抽象总图**：先看「同一个 Tomcat 后端，两个前端共用一层桥接，而 `/plan` `/model` 的瓶颈在后端缺命令」——谁负责画 UI、谁负责桥接、缺口在哪。
2. **A.2 具体总图**：再把同一条链路落到真实文件 / 进程 / wire 帧（`package.json` 命令声明 ↔ `commands.ts` 路由 ↔ `TomcatMessenger` ↔ `tomcat serve` 新增命令 ↔ `plan_runtime`）。
3. **B 状态机**：最后看「plan 模式」的生命周期：`chat → planning → executing → completed`，以及它如何由新增的 `set_plan_mode` 命令驱动、由 `get_state.planState` 回读。

> 说人话：Phase 2 的核心认知是「**VSCode 侧几乎没限制（slash 命令全稳定），真正卡点在 Tomcat serve 后端没有驱动 plan 模式的命令、也没有枚举模型的命令**」。所以 Stage A 的重头戏是改 Rust 后端；扩展侧只是把 slash 命令路由过去。Stage B 的 webview 则是纯前端增量，桥接核心一行不改。**Webview 应用框架的详细裁决（VSCode 只给宿主 API、Tomcat 选 `React + Vite`、参考 `cline/continue`、不走 Electron）下沉到 [`03-stage-b-webview.md`](tomcat-vscode-extension-phase2/03-stage-b-webview.md) §3.1。**

### A.1 抽象 ASCII 总图（职责 / 事实源 / 缺口 / 分叉）

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ 层 1  UI 前端（两条并存，仅用稳定 API）                                          │
│   前端 A：原生 @tomcat Participant —— Phase 1 已交付，Phase 2 增 /plan、/model    │
│           slash 命令（package.json contributes.chatParticipants[].commands）。   │
│   前端 B：自建 React + Vite Webview —— Phase 2 新增，富交互（内联 diff/思考块/模型选择/│
│           plan 可视化/多会话 tab）。                                              │
│   并存约束：A、B 同时注册、共享层 2 桥接核心与单 serve，并共享同一项目 scope 会话池；│
│           同一条 live 会话单前端归属（避免双驱动抢 busy/turn）。                   │
└───────────────────────────────┬────────────────────────────────────────────┘
                                 │ UI 无关接口：onUserPrompt / renderEvent / askUser
                                 ▼  （+ Phase 2 新增：setPlanMode / setModel / listModels）
┌──────────────────────────────────────────────────────────────────────────────┐
│ 层 2  桥接核心（Phase 1 已交付，100% 复用；Phase 2 只加几个命令包装）             │
│   不变：子进程生命周期 + NDJSON 编解码 + 会话路由 + control 回环 + 背压消费。      │
│   新增（薄包装）：sendSetPlanMode / sendListModels / 读 get_state.planState。     │
└───────────────────────────────┬────────────────────────────────────────────┘
                                 │ stdin: ServeCommand(NDJSON) ──▶
                                 │ ◀── stdout: OutFrame = Response | Control | Event
                                 ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│ 层 3  Tomcat 运行时（Phase 2 的真正改动点；单一事实源）                          │
│   现状缺口①：serve 无驱动 plan 模式的命令（plan 只在 CLI REPL 经 dispatch_chat   │
│             _command 触发；serve 把 "/plan" 当普通 prompt 文本喂给 LLM）。        │
│   现状缺口②：serve 无 list_models 枚举命令（set_model 已有；客户端只能读           │
│             ~/.tomcat/models.toml 或硬编码）。                                    │
│   Stage A 补：新增 set_plan_mode（桥接 PlanRuntime::enter_planning/exit_to_chat/  │
│             build_plan）、list_models（枚举 ModelCatalog）、get_state.planState、 │
│             plan.* 事件进 event_pump 白名单。                                     │
└──────────────────────────────────────────────────────────────────────────────┘

关键分叉（方案成立关键）：
  ① 接入入口：slash command（采纳，稳定可上架） / Configure custom agents（否决，
     .agent.md 模式，运行时注册 API 为 proposed 且绑不到 @tomcat）
  ② plan 模式驱动：serve 新增 set_plan_mode（采纳） / 把 "/plan" 当 prompt 发（否决，
     serve 不解析 slash，只会被当文字喂给 LLM）
  ③ 前端形态：participant + webview 并存（采纳） / 二选一（否决，浪费已建能力）
```

> 导读：这张图先回答"接什么、缺什么、改哪层"。最该记住的是 **缺口在层 3、不在层 1**——VSCode 的 slash 命令完全够用，真正要动的是 Tomcat serve 协议。层 2 桥接核心只加几个薄薄的命令包装，Phase 1/2 两个前端都吃同一份。

### A.2 具体 ASCII 总图（落到真实文件 / 进程 / wire 帧）

```text
 VSCode 扩展进程 (Node/TS)                              子进程 (Rust)
┌───────────────────────────────────┐        ┌──────────────────────────────────┐
│ package.json                       │        │ $ tomcat serve --stdio            │
│  contributes.chatParticipants[]    │        │                                   │
│    .commands:[{name:"plan"},        │       │  src/api/serve/types.rs           │
│              {name:"model"}]  ◀NEW  │       │   ServeCommand enum  ◀── NEW 变体: │
│  contributes.views (webview)  ◀NEW  │       │     SetPlanMode / ListModels      │
│            │ activate              │        │                                   │
│            ▼                       │ stdin  │  src/api/serve/commands.rs        │
│ extension.ts                       │ NDJSON │   handle_command  ◀── NEW 分支:   │
│  注册 participant(A) + webview(B)  ┼───────▶│     SetPlanMode→plan_runtime.*    │
│            │                       │ 一行一帧│    ListModels→ModelCatalog.entries│
│            ▼                       │        │     GetState 增 planState 字段    │
│ serveClient/TomcatMessenger.ts     │        │                                   │
│  (Phase1) + sendSetPlanMode  ◀NEW  │◀───────┤  src/api/serve/control.rs         │
│           + sendListModels   ◀NEW  │ stdout │   initialize capabilities  ◀NEW:  │
│            │                       │ 一行一帧│    +"set_plan_mode" +"list_models"│
│            ├──► ui/participant/*   │        │                                   │
│            │     commands.ts: /plan│        │  src/api/serve/event_pump.rs      │
│            │     /model 路由(A)    │        │   EVENT_NAMES  ◀── NEW: plan.*    │
│            └──► ui/webview/*  ◀NEW │        │                                   │
│                  provider.ts (CSP) │        │  src/core/plan_runtime/mod.rs     │
│                  protocol.ts       │        │   PlanRuntime::enter_planning /   │
│                  gui/ (React+Vite) │        │     exit_to_chat / build_plan ★   │
│            │                       │        │   (★ 已存在，serve 复用，不重写)   │
│            ▼                       │        │  src/core/llm/catalog.rs          │
│ ide/VsCodeIde.ts (Phase1 复用)     │        │   ModelCatalog::entries() ★       │
└───────────────────────────────────┘        └──────────────────────────────────┘
    构建期：tomcat serve --print-schema → 刷新 serveClient/wire.d.ts（新增命令自动入类型）
```

> 导读：这张图把抽象缺口落到真实对象。**最该看清两件事**：(1) Rust 侧的 `PlanRuntime` 和 `ModelCatalog` **已经存在**（带 ★），Stage A 只是给 `serve` 加几个命令变体把它们暴露出来，不是从零写 plan 引擎；(2) 扩展侧两个前端（`ui/participant/*` 与 `ui/webview/*`）都挂在同一个 `TomcatMessenger` 上，webview 是纯增量。协议一改，构建期 `--print-schema` 自动把新命令灌进 `wire.d.ts`，TS 编译期即可发现漂移。

### B. 状态机：plan 模式的生命周期（serve 驱动版）

```text
        set_plan_mode{action:"enter"}        set_plan_mode{action:"build",planId?}
┌────────┐ ──────────────────────────▶ ┌──────────┐ ──────────────────────────▶ ┌──────────────┐
│  Chat  │                              │ Planning │                              │  Executing   │
└────────┘ ◀────────────────────────── └────┬─────┘        prompt 推进            │ {plan_id}    │
     ▲       set_plan_mode{action:"exit"}    │ set_plan_mode{action:"exit"}        └──────┬───────┘
     │                                       ▼                                            │ 全部 todo 完成
     │                                 ┌──────────────┐  set_plan_mode{action:"build"}     ▼
     │     set_plan_mode{exit}         │   Pending    │ ──────────────────────────▶ ┌──────────────┐
     ├──────────────────────────────  │  {plan_id}   │                              │  Completed   │
     │                                 └──────────────┘                              │ {plan_id}    │
     │                                                                               └──────┬───────┘
     │             finalize（下一轮装配前回到 Chat）                                         │
     └───────────────────────────────────────────────────────────────────────────────────┘
```

| 当前状态 | 命令 / 事件 | 目标状态 | serve 侧动作（桥接 PlanRuntime） | 说人话 |
|----------|-------------|----------|----------------------------------|--------|
| Chat | `set_plan_mode{action:"enter"}` | Planning | `PlanRuntime::enter_planning()`；回 `ResponseFrame.ok{planState:"planning"}` | 用户点"进入计划模式"，后端切到 Planning。 |
| Planning | `set_plan_mode{action:"exit"}` | Chat | `PlanRuntime::exit_to_chat()` | 不想计划了，退回普通聊天。 |
| Planning | `set_plan_mode{action:"build",planId?}` | Executing | `PlanRuntime::build_plan(target, sessionId)` + serve 自动注入 `"start building <path>"` prompt | 计划定好了，开跑执行。 |
| Planning/Pending | `set_plan_mode{action:"exit"}` | Chat | `exit_to_chat()`（`Executing` 拒绝） | 计划挂起后也能退回聊天。 |
| Executing | 全部 todo 完成（`plan.complete` 事件） | Completed→Chat | `PlanRuntime` 内部 `set_mode_completed` → 下轮 `finalize_completed_to_chat` | 执行完了自然收口回聊天。 |
| 任意 | `get_state` | 不变 | 回 `payload.planState`（扩展据此刷新徽标/按钮） | 前端随时能问"现在是什么计划态"。 |

> 导读：状态机的关键是 **plan 模式现在由一条新的 serve 命令 `set_plan_mode` 驱动**，内部直接调用早已存在的 `PlanRuntime` 三个方法（`enter_planning` / `exit_to_chat` / `build_plan`，见 `tomcat/src/core/plan_runtime/mod.rs`）。扩展不需要懂 plan 引擎细节，只发命令、读 `get_state.planState`。`Executing→Completed→Chat` 的收口由 Tomcat 内部自然完成，扩展只需监听 `plan.*` 事件刷新 UI。

---

## 一句话总结

Phase 2 = 在 Phase 1 的桥接核心之上做两件并存的增量：**Stage A** 先给 `tomcat serve` 补 `set_plan_mode` / `list_models` 命令与 `get_state.planState` 字段（把早已存在的 `PlanRuntime` / `ModelCatalog` 暴露出来），再用稳定的 participant slash command 把 `/plan`、`/model` 接进原生聊天；**Stage B** 在同一桥接核心上自建 **React + Vite Webview** 做富交互前端，与原生 participant 默认并存、共享同一项目 scope 会话池（复用 `tomcat code` 的归组与 last-active 恢复），单条 live 会话单前端归属。VSCode 只提供 Webview 宿主 API，不提供 UI 框架；具体 UI 选型与开发打点参考 `cline` / `continue`，但不引入 Electron 桌面壳与重型 proto 总线。详细论证见上表四篇子文档。
