# Tomcat VSCode Chat 扩展技术方案：把 Tomcat 接入 VSCode 聊天体验的进程边界方案

> 适用范围：用一个**独立、可上架、可安装**的 VSCode 扩展，spawn 已就绪的 `tomcat serve --stdio` 子进程，用 Tomcat 既有 NDJSON wire 协议桥接，把 Tomcat 的 agent loop / 工具 / 权限 / 多会话能力接入 VSCode 聊天体验；**全程只用稳定 API**，不依赖任何 proposed API。
> 后续阶段：Phase 2（`/plan`·`/model` slash 命令补全 + serve 后端协议扩展 + 自建 Webview 富前端，与原生 participant 并存）见 [`tomcat-vscode-extension-phase2.md`](tomcat-vscode-extension-phase2.md)。本文为 Phase 2 的事实基线。
> 上位规范：[`ARCHITECTURE_SPEC.md`](../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)。本方案按规范 §1–§10 拆为「总览（本文）+ 4 篇子文档」，文首「方案导图集」置于子文档之前、不占用 § 编号。
> 标杆对照：Tomcat 服务端侧 [`agent-server-and-ui-gateway.md`](../../../tomcat/docs/architecture/agent-server-and-ui-gateway.md)（服务端能力）与本组文档（客户端接入）互补。
> 单一事实源：协议与类型以 `tomcat/src/api/serve/types.rs` + `tomcat/src/infra/events/mod.rs` 为准；本组文档不复制定义，只描述「扩展侧如何消费」。
> 外部参考仓库（与本仓同级，位于 `/Users/yankeben/workspace/`，仅作证据引用、不进本仓）：`vscode/`（VSCode 本体 + 内置 `extensions/copilot/`）、`cline/`、`continue/`。

**一句话定位**：让 Tomcat 当**后端**、VSCode 扩展当**前端**——扩展只做「serve 客户端 + UI 映射」，Tomcat 保留自己的 loop/工具/权限/多会话。

---

## 子文档索引

本方案按 ARCHITECTURE_SPEC §1–§10 拆分；下表给出「子文档 ↔ 规范 §」对应关系。建议先读本文「文首导图集」建立心智模型，再按需下钻。

| 子文档 | 覆盖规范 § | 内容 | 何时读 |
|--------|------------|------|--------|
| [`01-terminology-and-research.md`](tomcat-vscode-extension/01-terminology-and-research.md) | §1 术语 · §2 竞品调研 | 术语三件套；参考哪份仓库；四类形态横向对比；proposed API 必要性清单 + 稳定替代 | 想搞清"为什么这么选、能不能上架"先读它。 |
| [`02-implementation-details.md`](tomcat-vscode-extension/02-implementation-details.md) | §3 落地选型与实施 | §3.1 七列决策表（R1–R10）+ §3.2 五列实施点表 + Phase1/2 拆节 | 要看每个分叉的最终裁决与分步交付。 |
| [`03-protocol-and-file-map.md`](tomcat-vscode-extension/03-protocol-and-file-map.md) | §4 协议 · §5 One-Glance Map | ServeCommand/OutFrame/WireEvent 字段表 + NDJSON 样例 + 扩展侧文件职责框图 | 要实现桥接时按它对协议、按它建文件。 |
| [`04-runtime-reference.md`](tomcat-vscode-extension/04-runtime-reference.md) | §6 配置 · §7 错误 · §8 测试 · §9 风险 · §10 历史 | 配置项；错误归一化；测试矩阵；风险应对；否决方案留痕 | 落地/验收/排错时查它。 |

---

## 文首导读：方案导图集

### 阅读顺序建议（说人话）

1. **A.1 抽象总图**：先看「三层职责 + 单一事实源 + 两条 UI 分叉」——谁负责推理、谁负责桥接、谁负责画 UI，事实源在哪。
2. **A.2 具体总图**：再把同一条链路落到真实文件 / 进程 / wire 帧（`extension.ts` ↔ `TomcatMessenger` ↔ `tomcat serve` ↔ 生成的 `wire.d.ts`）。
3. **B 状态机**：最后看「一次 chat turn」的生命周期：`idle → running → awaiting_user(审批/提问) → done`，以及中断 / 子进程崩溃如何收尾。

> 说人话：这套方案的核心矛盾是「想要 Copilot 那种成熟聊天/编辑/审批 UI，但又要能上架」。先看 A.1 想清楚「我们不复刻 Copilot 的核心 UI，而是把 Tomcat 当后端、UI 用稳定 API 自己画」；A.2 落到具体文件后你会发现桥接层和 Tomcat serve 几乎同构（都是一行一个 JSON 帧），所以桥接核心可 100% 复用，UI 前端可换。

### A.1 抽象 ASCII 总图（职责 / 事实源 / 分叉）

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ 层 1  UI 前端（可插拔，仅用 VSCode 稳定 API）                                   │
│   专业：把"用户输入 / 流式回答 / 工具卡 / 编辑 diff / 审批"映射到 VSCode 控件。 │
│   说人话：负责"长什么样、点哪里"，不碰推理，也不直接读写子进程。               │
│   分叉 ──┬─ Phase 1：原生 Chat Participant（ChatResponseStream）               │
│          └─ Phase 2：自建 Webview（React，typed postMessage 流式帧）            │
└───────────────────────────────┬────────────────────────────────────────────┘
                                 │ UI 无关接口：onUserPrompt / renderEvent / askUser
                                 ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│ 层 2  桥接核心（UI 无关，100% 复用；本扩展唯一需要自研的"大脑外壳"）            │
│   专业：子进程生命周期 + NDJSON 编解码 + 会话路由 + 控制帧(审批)回环 + 背压消费。│
│   说人话：负责"把 Tomcat 这个命令行后端，翻译成 UI 能用的事件流和回调"。        │
│   事实源(类型)：tomcat serve --print-schema 生成的 wire.d.ts（不手写协议类型）。│
└───────────────────────────────┬────────────────────────────────────────────┘
                                 │ stdin: ServeCommand(NDJSON) ──▶
                                 │ ◀── stdout: OutFrame = Response | Control | Event
                                 ▼
┌──────────────────────────────────────────────────────────────────────────────┐
│ 层 3  Tomcat 运行时（已就绪，不改；单一事实源）                                 │
│   专业：tomcat serve 暴露 agent loop / 多模型 / 内置工具 / 权限 / 多会话。       │
│   说人话：真正"干活和思考"的人；扩展只是它的一个前端客户端。                    │
│   事实源(行为)：src/api/serve/*（types/control/event_pump/writer/ask_question）。│
└──────────────────────────────────────────────────────────────────────────────┘

关键分叉（方案成立关键）：
  ① 集成形态：A=独立扩展桥接 serve（采纳） / B=降级为 LM 模型 / C=fork Copilot（均否决）
  ② UI 路线：participant（先）→ webview（后），中间夹"桥接核心"，故换 UI 不动桥接
  ③ API 档位：稳定（采纳，可上架） / proposed（否决，身份门禁→第三方扩展被置空）
```

> 导读：这张图先回答"谁干什么"。最该记住的是**层 2 桥接核心与 UI 解耦**——因为它是本扩展唯一必须自研且最稳定的部分（协议固定、类型自动生成），Phase 1/2 换 UI 时它原样复用；而层 3 Tomcat 完全不改。proposed API 的去留只影响"层 1 能画多花"，不影响 A/B/C 选型。

### A.2 具体 ASCII 总图（落到真实文件 / 进程 / wire 帧）

```text
 VSCode 扩展进程 (Node/TS)                              子进程 (Rust)
┌───────────────────────────────────┐        ┌──────────────────────────────────┐
│ package.json                       │        │ $ tomcat serve --stdio            │
│   contributes.chatParticipants     │        │   (cfg.serve.transport=stdio)     │
│   或 viewsContainers+views(webview)│        │                                   │
│            │ activate              │        │  src/api/serve/stdin.rs           │
│            ▼                       │        │   run_stdio_loop: 逐行 NDJSON     │
│ extension.ts                       │        │     → parse_command_line          │
│   - 解析 tomcat 路径/参数          │        │     → handle_command (commands.rs)│
│   - new TomcatMessenger()          │        │                                   │
│            │                       │        │  control.rs: initialize 握手      │
│            ▼                       │ stdin  │    {protocolVersion:1,            │
│ serveClient/TomcatMessenger.ts ────┼───────▶│     capabilities:[prompt,steer,…, │
│   - child_process.spawn            │ NDJSON │     ask_question], sessionId}     │
│   - 一行一帧 send(ServeCommand)    │ 一行一帧│                                   │
│   - 行缓冲 split('\n') 解析 OutFrame│◀───────┤  event_pump.rs: AgentEvent →      │
│   - request id ↔ Promise 表        │ stdout │    OutFrame::Event (按 sessionId) │
│   - control_request(ask_question)  │        │  writer.rs: 单写者 + 会话轮转 +   │
│     ↔ control_response 回环         │        │    delta 合并(25ms)+背压(64帧)    │
│            │ 事件流 / 回调          │        │  ask_question.rs: 审批/提问桥      │
│            ▼                       │        │  registry.rs: 多会话 (max_sessions)│
│ ide/VsCodeIde.ts (稳定 vscode API) │        │                                   │
│   applyEdit / vscode.diff / 打开文件│        │  schema.rs: --print-schema →      │
│            │                       │        │    serve.schema.json + serve.d.ts │
│            ▼                       │        └──────────────────────────────────┘
│ ui/participant/*  (Phase 1)        │
│   ChatResponseStream.markdown/      │   构建期一次性：
│   button/filetree/progress/anchor   │   tomcat serve --print-schema
│ ui/webview/*      (Phase 2)         │     → 拷贝 serve.d.ts 为 wire.d.ts
│   React + Vite + typed webview 协议 │     → 桥接核心 import 该类型
└───────────────────────────────────┘
```

> 导读：这张图把抽象三层落到真实对象。**最该看清两件事**：(1) 扩展与 Tomcat 之间只有一根 stdio 管道，传 NDJSON，左边 `TomcatMessenger` 与右边 `stdin.rs/writer.rs` 严格对称；(2) 类型不是手写的——构建期跑一次 `tomcat serve --print-schema` 把 `serve.d.ts` 拷成扩展侧 `wire.d.ts`，协议漂移在编译期就能发现。Phase 1 时 `ui/webview/*` 不存在，桥接核心直接喂 `ChatResponseStream`。

### B. 状态机：一次 chat turn 的生命周期

```text
            user 输入                prompt(ServeCommand)
┌────────┐ ───────────▶ ┌──────────┐ ──────────────────▶ ┌──────────┐
│  idle  │              │ running  │  ◀── message_update  │ streaming│
└────────┘              └────┬─────┘      (delta 渲染)     └────┬─────┘
     ▲                       │  control_request(ask_question)   │
     │                       ▼                                  │
     │                 ┌──────────────┐  control_response       │
     │   agent_end     │ awaiting_user │ ──────(用户答复)──────▶ │
     │  (回到 idle)    │ (审批/提问)   │                         │
     │                 └──────┬───────┘                         │
     │                        │ 取消/超时 → control_cancel       │
     │                        ▼                                  │
     │                  (cancelled 兜底，turn 继续)              │
     │                                                          ▼
     │            interrupt(ServeCommand)              ┌──────────────────┐
     ├─────────────────────────────────────────────── │ agent_end / error│
     │            (用户点停止 → cancel_token)           └──────────────────┘
     │                                                          │
     │   子进程崩溃/EOF → exit 事件                              │
     └────────────────── failed (标记会话不可用，提示重启) ◀────┘
```

| 当前状态 | 事件 | 目标状态 | 副作用（扩展侧） | 说人话 |
|----------|------|----------|------------------|--------|
| idle | 用户输入 | running | send `prompt`/`follow_up`（带 sessionId）；登记 turn token | 用户敲回车就把 prompt 发给子进程，开始一轮。 |
| running/streaming | `message_update` | streaming | 累积 delta → `stream.markdown` 增量渲染 | 子进程吐字，UI 实时拼接显示。 |
| running | `control_request{ask_question}` | awaiting_user | 渲染问题为按钮/QuickPick，挂起等待 | Agent 要你拍板（选项/审批），UI 弹出来等你点。 |
| awaiting_user | 用户答复 | running | send `control_response{requestId,result}` | 你点了，扩展把结果回传，Agent 继续。 |
| awaiting_user | 取消/超时 | running | send `control_cancel{requestId}`（serve 兜底 cancelled=true） | 你不答或关了，子进程按"取消"兜底，不卡死。 |
| running/awaiting_user | 用户点停止 | done(interrupted) | send `interrupt{sessionId}`；等 `agent_interrupted`/`agent_end` | 点停止就发中断，子进程软取消当前轮。 |
| running | `agent_end{error:None}` | idle | 结束本 turn，落地最终消息 | 一轮正常收敛，回到空闲等下一句。 |
| 任意 | 子进程 exit/EOF | failed | 标记会话不可用，UI 提示并提供「重启 serve」按钮 | 后端挂了就告诉用户并给重启入口，不静默吞掉。 |

> 导读：状态机的关键是 **awaiting_user** 这一态——它对应 Tomcat 的控制帧回环（`control_request{ask_question}` ↔ `control_response`）。稳定 API 下我们用按钮/QuickPick 实现它，无需 proposed 的 `confirmation()` 卡片。中断走独立的 `interrupt` 命令（不是关管道），子进程崩溃才进 `failed`。

---

## 一句话总结

把 Tomcat 当**后端**、VSCode 扩展当**前端**：spawn `tomcat serve --stdio`，复用其 NDJSON wire 与自动生成的类型，中间夹一层 **UI 无关的桥接核心**；UI 先用**稳定** Chat Participant（Phase 1）、后可换自画 Webview（Phase 2），桥接核心两阶段 100% 复用。全程不依赖任何 proposed API，因此**可上架、可安装**——招牌体验里需要 proposed 的部分（内联 diff / 审批卡 / 思考块 / 工具卡 / 用量）全部用 Cline/Continue 已验证的稳定替代凑齐。详细论证见上表四篇子文档。
