## 附录 A：Q&A（常见追问）

本附录不新增决策，只把正文里最常被反复追问的结论，用问答方式钉死，方便后续做 VSCode 插件、桌面 GUI 或继续讨论 Phase 2 时直接引用。

说人话：前面 `§2/§3` 是给 reviewer 审方案用的，这里是把“那最终到底先做什么、为什么、什么时候升级”翻译成几句能直接拿来拍板的话。

### Q1. 如果目标是先做 VSCode 插件，应该先做 `stdio` 还是 `WebSocket daemon`？

**专业结论**：先做 `**stdio` 子进程**，不要先做 `WebSocket daemon`。  
理由已经在 **§2.2 / §3.1 R1、R2** 定稿：本地 IDE 集成的主流路线是 `spawn(agent-cli)` + 行分隔 JSON 双工，而不是先起一个常驻网络服务。

证据：

- `codex-rs/cli/src/main.rs`：VSCode 场景走 `app-server` 并标记 `SessionSource::VSCode`；
- `codex-rs/app-server/README.md`：VSCode / SDK 路径以 `codex app-server --listen stdio://` 为主；
- `codex-rs/app-server-transport/src/transport/stdio.rs`：`codex app-server` 默认就是 stdio；
- `cc-fork-01/src/cli/structuredIO.ts`：`stream-json` 以 stdout 单写者为主链路；
- `pi_agent_rust/src/rpc.rs`：`pi --mode rpc` 用 JSONL stdio；
- `pi-mono/packages/coding-agent/src/modes/rpc/rpc-mode.ts`：RPC 模式同样走 stdio。

说人话：VSCode 扩展宿主本来就擅长 `child_process.spawn()`，所以第一步直接开 `tomcat serve --stdio` 最顺；先上 daemon 只会把事情做重。

### Q2. 为什么不建议一开始就做 `WebSocket daemon`？

**专业结论**：因为它会提前引入一整组只有“多客户端 / Web / 真远程”才需要的复杂度，而这些复杂度在单窗口 VSCode MVP 里没有收益。

对比表如下：


| 维度                            | `stdio` 子进程          | `WebSocket daemon`         | VSCode 单窗口实际需要吗 | 说人话                   |
| ----------------------------- | -------------------- | -------------------------- | --------------- | --------------------- |
| 端口分配 / 发现                     | 不需要                  | 需要（lockfile / 握手 / URL 发现） | ❌               | 管道天然直连，daemon 先得找到它。  |
| 鉴权（token / loopback / origin） | 不需要（进程边界即信任）         | 必须                         | ❌               | 本机父子进程不需要再验一次口令。      |
| 生命周期                          | 跟随扩展，扩展关进程退          | 谁启停 daemon、孤儿进程回收          | ❌               | 子进程跟着扩展死最省心。          |
| 多客户端 / 广播                     | 一对一，天然               | 要做 `sessionId` 路由/广播       | ❌               | 单窗口并不需要广播。            |
| 重连 / 背压                       | 管道断 = 收口退出           | 要做重连队列 / 慢客户端处理            | ❌               | daemon 需要想更多断线后的状态恢复。 |
| 实现量                           | 最小（见 **§3.2 P0–P5**） | 额外一整套网关（见 **§3.2 P7**）     | ——              | 不是更优雅，只是更重。           |


具体多出来的成本：

1. **端口发现与生命周期**：谁启动 daemon、扩展重载后怎么接回、孤儿进程如何回收。
2. **鉴权与本地暴露面**：token、loopback 绑定、origin/CORS 校验（见 `openclaw/src/gateway/auth.ts`、`openclaw/src/gateway/origin-check.ts`）。
3. **多客户端语义**：是否允许多个窗口共连、广播还是单播、`sessionId` 怎么路由。
4. **重连与背压**：断线重连、积压队列、慢客户端掉帧策略。

补充证据：6 个调研对象里，5 个的本地 / IDE 主链路都是 stdio 行分隔 JSON；`openclaw` 才是纯 WS 网关，而它也因此要维护 `src/tui/embedded-backend.ts` 这一大块本地 embed 补偿逻辑。

说人话：daemon 不是“更高级的 stdio”，而是另一档复杂度。你现在只是想让 VSCode 先调起来，不需要先背这些包袱。

### Q3. 如果用户在 VSCode Remote-SSH / devcontainer / Codespaces 里用 Tomcat，结论会变吗？

**专业结论**：对 **Remote-SSH / devcontainer / Codespaces**，**不会变**。  
只要扩展宿主和工作区在同一台机器上运行，扩展依然可以在那台机器上直接 `spawn("tomcat", ["serve", "--stdio"])`，stdio 结论不变。

这类场景变化的是**扩展宿主所在的位置**，不是 UI 与 agent 的关系模型：它仍然是“一个进程拉起另一个本机子进程并独占它的 stdin/stdout”。

`vscode.dev` / 纯浏览器 web 版要单独看：**只有当存在可用的 Node/remote extension host 时，stdio 结论才继续成立**；如果是纯浏览器 extension host、不能 `spawn` 本地进程，就不该在本文里把它和 Remote-SSH 直接等同。

说人话：Remote-SSH 只是“VSCode 在远端帮你开子进程”，不是“必须上 WebSocket”。至于纯浏览器 web 版，关键不是“远不远程”，而是“还能不能开子进程”。

### Q4. 什么时候才值得做 `WebSocket daemon` / 网络 Gateway？

**专业结论**：命中下列任一场景时，再进入 **§3.2 P7（Phase 2）**：

1. **需要跨扩展重载 / 窗口重载存活**：agent 进程不想跟着 VSCode child process 一起死。
2. **需要多客户端共享同一个 agent**：例如桌面 GUI、Web 控制台、CLI viewer 同时连。
3. **需要真远程接入**：agent 跑在另一台机器，UI 不是本机 spawn 子进程。
4. **需要浏览器直接接入**：浏览器不能直接持有本地子进程 stdio，这时才需要 WS/HTTP。

在此之前，持久化交给 Tomcat 现有的 session 存储即可，不需要为了“会话下次还能接着来”就先引入常驻 daemon。

说人话：只有当 UI 不再是“一个本机扩展拉一个本机 agent 子进程”时，WebSocket daemon 才开始真正值回票价。

### Q5. 先做 `stdio` 会不会把架构锁死，后面不好升级到网关？

**专业结论**：不会。  
本文从一开始就把 **dispatcher** 和 **transport** 分开：`commands.rs`、`event_pump.rs`、`control.rs` 是**协议/编排层**，`stdio` / `ws` 只是**传输适配层**。这也是 **§3.2.4** 明确钉死的升级路径。

升级时复用关系如下：

- 保留：`commands.rs`（命令分发）
- 保留：`event_pump.rs`（订阅 EventBus 下行）
- 保留：`control.rs`（审批/初始化/中断回环）
- 替换/新增：`stdin.rs`、`writer.rs` → `gateway/ws.rs`

可参考：

- `codex-rs/app-server/src/in_process.rs`：同一 dispatcher 同时支持 embed 与远端 client；
- `hermes-agent/tui_gateway/server.py` + `tui_gateway/ws.py`：同一 handler 表同时支持 stdio 和 WebSocket。
- `codex-rs/stdio-to-uds/src/lib.rs`：甚至可以让扩展始终只说 stdio，再由一个轻代理桥到后台 daemon / UDS，扩展侧无需改协议。

说人话：先做 stdio 不是走岔路，而是在把“协议内核”先做实。以后只是把“这份 JSON 怎么运出去”从管道换成 WebSocket。

### Q6. 那 `SSE` 呢？要不要顺手一起做？

**专业结论**：**不要把 SSE 当成首发主链路。**  
从调研看，SSE 大多只适合：

- 历史回放 / 事件追放（`openclaw/src/gateway/sessions-history-http.ts`）；
- OpenAI 兼容 API 的流式输出（`hermes-agent/gateway/platforms/api_server.py`）；
- 云端 worker / bridge 的只读流（`cc-fork-01/src/cli/transports/SSETransport.ts`）。

但 Tomcat 的 VSCode MVP 需要的是**双向**能力：`prompt`、`interrupt`、`control_request/response`、审批/提问回包。SSE 天生单向，下行之外还得再补一条上行通道，复杂度不比 WebSocket 低。

说人话：SSE 更像“只读直播”，Tomcat 插件要的是“边直播边对话边审批”，所以它不是第一步。

### Q7. 如果以后要兼容 Zed / 其他 IDE 标准协议，是不是应该现在就直接上 ACP？

**专业结论**：也不建议。  
ACP 适合作为 **Phase 2+ 兼容层**，不是首发唯一协议。原因见 **§3.1 R4**：ACP 自带一套 IDE 语义（`session/`*、permission/file API），与 Tomcat 当前能力面不是一一同构，先上会拖慢 `tomcat serve --stdio` 的最小闭环。

更合理的顺序是：

1. 先把 `ServeCommand` / `AgentEvent` / `control_*` 自有 wire 跑通；
2. 再做 `src/api/serve/acp/*`，把 ACP method 映射到内部命令/事件。

说人话：先把自己的普通话说顺，再学行业普通话；不要一开始为了兼容别人，把自己最小闭环拖慢。

### Q8. VSCode 扩展侧最小接线大概长什么样？

**专业结论**：最小实现就是 Node 扩展宿主 `spawn("tomcat", ["serve", "--stdio"])`，stdin 发命令帧，stdout 按行读 `AgentEvent`，控制回环也还是同一条管道上的 NDJSON。

示意：

```ts
import { spawn } from "node:child_process";
import * as readline from "node:readline";

const child = spawn("tomcat", ["serve", "--stdio"], {
  cwd: workspaceRoot,
  env: { ...process.env, TOMCAT_AGENT_ACTIVE: "1" },
});

readline.createInterface({ input: child.stdout }).on("line", (line) => {
  const ev = JSON.parse(line);
  dispatchToWebview(ev);
});

function send(msg: object) {
  child.stdin.write(JSON.stringify(msg) + "\n");
}

send({
  type: "control_request",
  subtype: "initialize",
  request_id: "init-0",
  payload: { clientInfo: { name: "tomcat-vscode" } },
});

send({ type: "prompt", id: "c1", text: "..." });
```

再配合 **§3.2 P6** 的 `tomcat serve --print-schema` 导出 `.d.ts`，扩展侧就不用手写 wire 类型；这一点可直接对标 `codex-rs/app-server-protocol/src/export.rs` 的做法。

说人话：第一版插件根本不需要什么神秘通信层，就是 Node 子进程 + 一行一条 JSON。把这条链路打通，后面再谈 daemon 才有意义。

### Q9. 我本期就要做到和 Cursor / Copilot / Codex 一样「一个进程同时跑多个会话」，到底能不能、缺什么？

**专业结论**：**能，而且本期就做**（这是 §3.1 R8 本次修订后的定稿）。

为什么能：你的核心**早就为多会话留好了缝**（见 §2.5.3 与 `multi-agent.md` §14.3.1）：

- `AgentLoop` 无全局单例，可按 `session_id` 多实例并发；
- 事件已按会话打信封——`ScopedEventEmitter::emit → WireEnvelope.sessionId`（`src/infra/event_bus/mod.rs`、`src/infra/events/mod.rs`），命名与 pi 家族一致；
- `AgentRegistry`（`src/core/agent_registry/mod.rs`）**已实现**进程级登记、`cascade_abort` 级联中止、`MAX_CONCURRENT_AGENTS=16` 并发上限；
- 重资源（`LlmProvider`/`PrimitiveExecutor`/`EventBus`）已是进程级 `Arc<dyn …>` 共享，会话级状态隔离（`ScopeServices`/`SessionRuntime`）。

缺的只有 **serve 传输层那一层壳**（全部新增在 `src/api/serve/`，`AgentLoop` 一行不改）：

1. `ChatContextRegistry`（`registry.rs`，`DashMap<sessionId, SessionSlot>`）——落地 `multi-agent.md` 维度A/MA2 那张规划表；
2. 命令按 `sessionId` 选会话槽（`commands.rs`）；
3. 同会话串行守卫（per-session `busy`），跨会话真并发；
4. 单写者按 `sessionId` demux + 跨会话公平轮转（`writer.rs`）。

这正是 **codex `app-server` 的 `ThreadManager` 模型**（`codex-rs/core/src/thread_manager.rs` 的 `HashMap<ThreadId, Arc<CodexThread>>` + per-thread session loop + `SerializationScope::Thread`），与 Tomcat 同语言、可直接对照。落地细节见 **§3.3**。

说人话：你没记错——核心确实是「每个会话能各跑各的」。但现在真实形态是 `chat_loop` 每次输入新建一个 `AgentLoop`，而整个 `ChatContext` 是绑死单个会话装配的（单 `SessionManager::new_scoped` + 单 root guard + 单 `cancel_token`），一个进程同时只驱动一个活跃 `ChatContext`。补一张 `ChatContextRegistry` 把多个 `ChatContext` 并排放进来、命令按 sessionId 选、事件按 sessionId 分，就到 Cursor/Codex 那个级别了。

再压成一句记忆：`ChatContextRegistry` 管“我现在开了哪些房间”；`AgentRegistry` 管“这些房间里现在有哪些工人和子工人在干活、怎么一键停掉”。两个都叫 registry，但一个是前台总台，一个是后台调度，不该合并成一张表。

### Q10. 「我架构上已经分了多线程（每个 session 一个线程）」这个理解对吗？为什么还说支持不了多会话？

**专业结论**：方向对，但表述要修正。当前不是「每个 session 常驻一个线程」，而是「**每次用户输入 `chat_loop` 起一个 `AgentLoop::new().run()`**」；并发底座（多实例、信封、`AgentRegistry`）是齐的，但**外层只装配了一个 `ChatContext`**，所以一个 serve 进程同一时刻只伺候一个活跃会话。

差距精确定位（`src/api/chat/context.rs`）：

- `ChatContext::from_config_with_mode_and_overrides` 内 `SessionManager::new_scoped(session_key)`、`register_root(current_session_id)`、单个 `cancel_token`——**整壳绑定单会话**；
- 没有 `ChatContextRegistry`（`multi-agent.md` MA2 规划但**尚未实现**），所以无处安放「第二个、第三个活跃会话」。

把这两点补上（serve 层），「每个会话各自一个 run task + 各自 interrupt / cancel」就名副其实了，而且不是「一会话一 OS 线程」，是 `tokio::spawn` 的异步任务，更省。

说人话：你已经把「发动机」（能并发的 `AgentLoop` + 按会话分流的事件总线）造好了，差的是「多车位的停车场」（`ChatContextRegistry`）和「按车牌分流的闸机」（sessionId 路由）。这俩在 serve 层，本期补得上。

### Q11. 单进程多会话，为什么不学 cc-fork「一个会话开一个子进程」？哪个更稳？

**专业结论**：**走单进程多路复用（codex 模型），不学 cc-fork 多进程**。两者是不同取舍（见 §2.5.4）：

- cc-fork（`bridge/sessionRunner.ts` 每会话 `spawn(claude --print)`）是「为强隔离 / 水平扩展付内存税」——重资源 ×N、要造子进程管理 + stdout fan-in + token 注入，适合云端 worker 农场；
- Tomcat 的诉求是「VSCode 里同时开几个会话 tab」，而现状 `GlobalServices` 已进程级共享、`EventBus` 已带 `sessionId` 信封，单进程多路复用既省资源又**贴合现有对象**，是最短路径；
- 稳定性上，Tomcat 已有 `catch_unwind` + `AgentRegistry` 的 `tokio::spawn + JoinHandle` panic 隔离（`src/core/agent_registry/mod.rs`），单会话崩不连累其它会话，不需要靠进程边界换隔离。

反例警示：pi-mono / pi-rpc 的「switch 式单活跃」（`AgentSessionRuntime` teardown→`dispose`→rebind）会在切换时**丢掉 in-flight turn**，开不了真正的多 tab——这正是我们**不**采用的路线。

说人话：多进程是「拿内存换物理隔离」，适合跑成百上千个独立任务；你要的是本机几个会话 tab 并行，单进程 + 一张会话表最划算，崩溃隔离已经有现成的 panic 兜底。

---

**一句话总结**：要给 Tomcat 加 VSCode 插件 / 桌面 GUI，**不必先做网络网关**——其他 agent 暴露能力的主流是「**行分隔 JSON over stdio 子进程**」（pi/cc-fork/codex-stdio/hermes-stdio），SSE 基本只用于历史回放/兼容 API，WebSocket/HTTP daemon（openclaw）是面向 Web/多端的「最终形态」。Tomcat 的最短路径是新增一个 `**tomcat serve --stdio` Agent Server**：上行用自有 `{type}` NDJSON 命令、下行**直接复用既有 `AgentEvent` wire**、审批/提问**复用既有 `EventBusAskQuestionPanel` 回环**、所有输出过**单写者 drain**；**并且本期即做「单进程多会话并发」**——新增 `ChatContextRegistry`（落地 `multi-agent.md` 维度A/MA2）+ 命令按 `sessionId` 路由 + 同会话串行/跨会话真并发 + 单写者按 `sessionId` 公平 demux，对标 codex `app-server` 的 `ThreadManager`；核心（`AgentLoop` 多实例、`WireEnvelope.sessionId` 信封、`AgentRegistry` 登记/级联中止/并发上限）**已就绪、一行不动**，缺的只是 serve 这层壳。这层 dispatcher 与传输解耦，等真要 Web/远程时，Phase 2 再把它多挂一个 WebSocket 传输即可。