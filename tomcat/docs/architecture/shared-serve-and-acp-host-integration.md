# 共享 serve 与 ACP 宿主接入：Paperclip、Multica 和 VS Code

> **状态：设计稿，未实施。** 依据修订后的运行时计划 `plan_tomcat_paperclip_multica_agent_3e12d03d`，不是旧版“断线后后台续跑”方案。
> **本期结论：**一个长期运行的 serve；两个宿主只新增标准 Agent 适配器；控制连接断开即取消其任务，旁观连接断开不影响任务。
> **本轮同步：**按计划第 3 节的慢连接修订，每条连接复用现有 writer：合并增量，堆满时丢消息增量并提示，不因慢读断开或取消任务。此前的 access/origin、两层只读、普通打开流程与当前消息补发保持不变。
> **范围：**同机、同用户、同 Tomcat 数据目录和 agent，允许不同项目/worktree。WebSocket、断线续跑并接回原轮均为后续规划。
> **规范：**遵循 [ARCHITECTURE_SPEC](../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)，正文 §1–§10 与其对应。返回[架构文档地图](README.md)。
>
> 本文把计划落到职责、接口、状态和验收，不宣称已有这些接口。标记 **【新增】** 的文件、字段和测试名均为拟建；现有实现与限制另列证据。计划负责实施进度，本文负责本期技术契约。

## 文首导读：方案导图集

### 阅读顺序建议

1. 看 A.1：谁执行、谁控制、谁只看。
2. 看 A.2 和 B：消息怎么进来，连接断开后谁负责停工具。
3. 看 §3：关键取舍和实施顺序；看 §4：协议与字段。
4. 实施时按 §5 找文件，按 §8 验收；已知限制与待验证接缝集中在 §9。

### A.1 抽象总图：服务长驻，不代表任务脱离控制者

```text
控制者发任务                         旁观者打开会话
     |                                     |
     v                                     v
连接准入 + 唯一控制者                 原有打开流程 + 只读
     |                                     ^
     v                                     |
共享服务中的会话运行时 ------> 实时消息 / 工具状态
     |                                     |
     +------> 原有会话历史 <----------------+
     |
     +-- 正常完成 ------------> 保存结果、回报宿主
     +-- 控制者取消/断开 -----> 停任务和工具、回报取消或停止失败
     +-- 旁观者切走/断开 -----> 只停止向它转发，不改变任务
     +-- 服务停止/崩溃 -------> 全部中断，不自行重跑
```

服务是执行状态的唯一拥有者，控制权属于连接，历史仍以原 transcript 为准。服务只在事件转发处记住当前一条助手消息的正文、思考和结束状态，给迟到的连接补前缀；已保存的用户消息、助手消息和工具内容走原有历史接口，不维护整轮画面。

**说人话：**大家共用一间厨房；点菜的人走了，他那锅停火。围观的人走了，不影响做菜。

### A.2 具体总图：两个入口，共用一个运行时

```text
Paperclip tomcat-local        Multica tomcat backend         VS Code
          |                            |                       |
          | ACP JSON-RPC / stdio       |                       |
     tomcat acp                   tomcat acp              Messenger
     [薄转发器]                   [薄转发器]              [原生 NDJSON]
          |                            |                       |
          +----------------------------+-----------------------+
                                       |
                      Unix socket / Windows 命名管道
                      OS 用户权限 + 本机握手 + 环境快照
                                       |
                    +---------- 一个 tomcat serve ----------+
                    | connection：各自握手/环境/输出/当前显示 |
                    | ACP SDK → 内部会话操作 ← 原生命令      |
                    | runtime + ChatContextRegistry          |
                    |   session A          session B         |
                    |   SessionSlot        SessionSlot       |
                    |   AgentLoop/工具     AgentLoop/工具    |
                    |   当前消息补发 → 每连接独立消息队列    |
                    +-------------------+--------------------+
                                        |
                              原有 SessionManager / JSONL
```

`tomcat acp` 不调用模型、运行工具或保存会话；它负责连接、报环境、转发和退出协调。ACP 业务映射在 serve 内，VS Code 不经过 ACP 转发器。转发器由宿主按连接需要启动，不是为两个平台各开发一套程序。

**说人话：**转发器是接线头，不是第二个 Agent；真正干活的始终是中间那个服务。

### B. 状态机：连接、任务、服务分别收尾

```text
连接：握手中 -> 就绪 -> draining -> 已关闭
                  |        ^
                  +-- EOF / 写出失败 / 明确离开

会话：空闲 -> 运行中 <-> 等批准
        ^       |          |
        |       +-- 取消 / 控制连接断开 --> 停止中
        |                                    |
        +-------- 正常完成 / 确认已停止 -------+
                                             |
                                        停止超时：保持不可接手

服务：不存在 -> 启动锁内复查并启动 -> 就绪 -> 明确停服 -> 不存在
                                       |
                                   客户端全部离开：仍就绪
```

| 当前状态 | 事件 | 目标状态 | 副作用 | 说人话 |
| --- | --- | --- | --- | --- |
| 就绪连接 | new/load 成功 | 控制该会话 | 原子登记唯一控制者 | 先领到会话，才能发任务。 |
| 就绪连接 | 读得慢、输出积压 | 仍就绪 | 本连接 writer 合并增量；满时丢消息增量并提示，不触发取消 | 界面卡一下，不等于人走了。 |
| 运行/等批准 | 控制连接断开 | 停止中 | 封住新准入、清空断线控制绑定，取消确认和排队项、停止工具与子 agent | 控制者已离开，不代表工具已停手。 |
| 停止中 | 所有相关工作退出且必要落盘完成 | 空闲、可接手 | 解除停止中占用，通知旁观者；不自动把旁观者升级为控制者 | 确认停手后，重新打开才能接手。 |
| 停止中 | 到达宽限期 | 停止失败、不可接手 | 报 `stop_timeout`，保留运行句柄/占用事实 | 超时不是“已经停了”。 |
| 旁观中 | 切走/关闭视图/断开 | 结束旁观 | 停止接收该会话事件，不修改任务或控制者 | 关直播不等于关机器。 |
| 服务就绪 | stop/restart | 关闭所有会话 | 执行现有清理，不设客户端否决 | 明确停服会影响大家。 |

## 1. 术语统一

本节区分进程寿命、连接寿命和一次执行的寿命。后文“整轮结束”专指最外层任务结束、工具收尾及必要持久化完成，不是某条 `MessageEnd` 到达。

| 术语 | 语义 | 数据载体 / 单一事实源 | 行为约束 | 说人话 |
| --- | --- | --- | --- | --- |
| 服务实例 | 同一数据目录与 agent 的共享执行进程 | 【新增】`ServeRuntime`、本机监听、实例 ID | 身份不含 cwd、客户端或程序版本；版本不同不能另起第二份 | 一家共用的厨房。 |
| 连接 | 一个客户端与服务的一条通信链路 | 【新增】`ServeConnection`、连接 ID、输出队列 | ID 只在当前服务实例有效；握手环境只存内存 | 一根连接线。 |
| 背压 | 客户端读取速度赶不上服务输出 | 每连接独立 writer；内部仍按 sessionId 分缓冲 | 沿现有合并/丢增量/提示策略；不因慢读断开，区别于真实 EOF 或写失败 | 接收不及就少播片段，不把任务停掉。 |
| 控制者 | 有权给某会话派活的连接 | slot 的控制绑定，注册表统一维护，只在服务内存中 | 每会话最多一个；断线清空绑定并取消，原任务停完才能交接 | 当前点菜的人。 |
| 旁观者 | 只能读消息和状态的连接 | 连接的当前显示 sessionId + slot 控制绑定 | 打开已加载会话不重建执行上下文、不接手控制；只改本连接显示，不参与写命令 | 只看不动。 |
| 会话 | 可跨多次 prompt 保留的对话 | `SessionManager`、`SessionSlot`、原 transcript | load 是读历史继续，不是接回旧 RPC | 一份对话记录。 |
| 整轮任务 | 一个 prompt 驱动的最外层执行 | `run_task`、cancel token、完成结果 | 可包含多条 assistant 消息和多次工具调用；不能以首个 agent_end 猜成功 | 一次完整交办。 |
| 当前消息补发记录 | 当前一条助手消息的累计正文、思考及是否结束 | `event_pump.rs` 转发处的会话内记录【新增】 | 下一条 message_start 才替换，slot 卸载时释放；不以 message_end 当落盘确认 | 只补正在说的这句话，不重做整场会议纪要。 |
| 会话来源 `origin` | 这个会话是否经 ACP 创建及创建方名称 | 【新增】transcript 第一行的可选对象 `{kind:"acp", clientName:…}` | 只在 ACP new 时写一次；原生/老会话缺省；名称不参与权限判断 | “谁开的会话”，不是“谁现在在线”。 |
| 本窗口访问方式 `access` | 本窗口此刻是 control 还是 observe | 【新增】switch_session 回复及扩展会话视图内存 | 服务按来源和控制者判断；不落盘，与消息 role 无关；重新打开才重新判定 | 这扇窗口现在能不能下指令。 |
| 连接环境快照 | 客户端交给会话子进程的环境 | 内存 `map<string,string>` | 不全局 `set_var`，不写聊天、日志或会话文件 | 每个任务带自己的工具箱。 |

“唯一执行者”只保证**这个共享 serve 内**同一会话不被重复创建或控制。本期保留现有 usage 共享锁，不声称它能排斥另一个独立 `chat`/旧 stdio 进程执行同一历史；该兼容边界见 §9。

## 2. 竞品与选型调研

本期选择共享服务，主要为了同一执行实例上的实时旁观和用户要求的统一入口；不再以“断线续跑”为当前理由。外部实现是取舍证据，不是 Tomcat 已经通过测试的证明。

| 实现 | 关键设计及源码证据 | 借鉴 / 不照搬 | 说人话 |
| --- | --- | --- | --- |
| Codex | `codex/codex-rs/app-server-transport/src/transport/unix_socket.rs::prepare_control_socket_path`、`run_control_socket_acceptor`：本机监听、残留检查；`app-server-daemon/src/backend/pid_start.rs`：真实脱离；`app-server/src/thread_state.rs`、`request_processors/thread_lifecycle.rs`：快照与订阅 | 借鉴本机连接、启动协调和补发/开始转发的同一同步边界；Tomcat 只保留当前一条消息，不照搬整轮快照或控制断线续跑 | 可以学它怎么接线，不必照抄任务寿命。 |
| VS Code | `vscode/src/vs/base/parts/ipc/node/ipc.net.ts::createStaticIPCHandle`；`src/vs/code/electron-main/main.ts::claimInstance` | 借鉴固定 IPC 路径及争用处理；Tomcat 采用当前用户权限，不额外存令牌 | 用系统现成的门锁。 |
| OpenCode | `opencode/packages/cli/src/services/daemon.ts`：TCP、`server.json`、password；`packages/opencode/src/cli/cmd/acp.ts`：ACP 入口内起服务 | 借鉴 serve/attach；本期不用端口发现文件和密码，不为每个 ACP 子进程养执行器 | 都在本机，先不搭网络网关。 |
| Continue | `continue/extensions/cli/src/commands/serve.helpers.ts`、`serve.ts`；`extensions/vscode/src/extension/VsCodeExtension.ts` | CLI/扩展各自运行的 Core 与历史快照不等于同一条 token 直播 | 能读聊天记录，不代表正在看直播。 |
| Codex 项目规则 | `codex/codex-rs/core/src/agents_md.rs::agents_md_paths` | 取根→cwd、最近仓库边界；不取文件名回退、32KiB 总量截断 | 只读当前目录这条祖先链上的说明。 |
| Paperclip / Multica | `paperclip/packages/adapters/gemini-local/src/server/acp.ts::createGeminiAcpExecutor`；`multica/server/pkg/agent/traecli.go`、`hermes.go::hermesClient` | 复用标准适配器与 ACP 客户端，不改调度状态机 | 像加其他 Agent 一样加 Tomcat。 |

现成测试证据：Codex `app_server_startup_lock_serializes_waiters`（`app-server-transport/src/transport/unix_socket_tests.rs`）、`thread_resume_rejoins_running_paginated_thread_with_initial_page`（`app-server/tests/suite/v2/thread_resume.rs`）、`thread_unsubscribe_during_turn_keeps_turn_running`（`thread_unsubscribe.rs`）。它们分别证明启动互斥、中途加入、旁观退订，不证明本方案的控制连接取消语义。

最终选择共享运行时而非私有执行器，理由只有四点：会话列表在一处；旁观直接读取真实执行状态；复用现有完整调度/权限/清理；符合“一个服务大家连”的入口要求。代价是服务启停、连接拆分和环境隔离。若 M1 证明这些代价无法合理控制，应修订计划，不同时保留另一套自动 fallback。

### 2.1 复核计划时发现的事实边界

这些是实现证据带来的澄清，不改变“标准适配器、本期断线取消”的方向。

- **Kimi 临时技能不是现成 ACP 能力。** `paperclip/packages/adapters/kimi-local/src/server/execute.ts::buildKimiSkillsDir` 在 CLI lane；ACP 分支提前返回。Tomcat 适配器必须自己在调用共享引擎前准备/清理目录，不能仅复制 ACP 配置后宣称技能已注入。
- **DSH 是取消生命周期先例，不是 ACP 协议先例。** `multica/server/pkg/agent/dsh.go` 自管 Cancel 并发 DSH cancel 帧；Tomcat 只借鉴“backend 自管优雅关闭、超时杀进程组”，通知方式改为关闭 relay stdin。
- **`cancel_and_wait` 不等于所有工作已停止。** 当前 `tomcat/src/api/serve/mod.rs:587-627` 等前台及 checkpoint，但没有直接 stop/wait 全部 Bash registry；断连清理必须补齐这个已有生命周期接缝。
- **自动批准与人工批准不同。** Tomcat 的 false 要求发权限请求；宿主仍可能按自身 approve-all/allow_once 策略回答。不承诺 false 一定弹人工界面。
- **没有无条件的“重启后原会话”。** Paperclip 完成路径才保存主要 session 参数，恢复失败可以新开；Multica 提前上报 ID 也不是同步持久化事务。保留它们原有行为，不保证崩溃续聊 exactly-once。
- **writer 已有背压处理，但不是严格内存上限。** `tomcat/src/api/serve/writer.rs:187-216` 先合并、满时丢消息增量并发 notice；非增量帧继续排队。Bash 输出预览另有既有合并/截断规则（同文件 `enqueue_shell_preview`），不能把计划里的“只丢增量”扩大成“所有工具预览绝不丢”，也不能宣称整个连接队列严格有界。见 §3.2.2、§9。

## 3. 落地选型与实施

这里的“已定稿”指依据当前计划确定的设计方向，不表示代码已交付。涉及新本机接口的字段是本文的拟定契约，M1 须用真实 SDK/客户端验证后固化到 Rust 类型及生成产物。

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| 执行归属 | 三个入口能否看到同一执行 | 采用共享 serve，复用完整 SessionSlot 生命周期。 | Tomcat `tomcat/src/api/serve/mod.rs`；Codex `codex/codex-rs/app-server/src/thread_state.rs` | 设计：连接层之下共用注册表；理由：不重拼 AgentLoop 周边流程 | OpenCode `opencode/packages/opencode/src/cli/cmd/acp.ts` 的每入口自带 server：旁观需额外发现 | 大家看同一锅，不各煮一锅。 |
| 寿命 | 不改宿主重试和收尾 | 控制连接断开取消，旁观断开只退订。 | Tomcat `tomcat/src/api/serve/mod.rs::cancel_and_wait`；Multica `multica/server/pkg/agent/launch.go` | 设计：由服务停任务；理由：保持宿主对 Agent 进程的生命周期假设 | Codex `codex/codex-rs/app-server/tests/suite/v2/thread_unsubscribe.rs` 的持续运行只用于旁观，不用于控制者 | 宿主以为已经停手时，不能还在偷偷改文件。 |
| 本机接入 | 并发启动、鉴权与发现 | 采用固定 IPC 路径、OS 权限及启动锁。 | Tomcat `tomcat/src/infra/platform/fs.rs`；VS Code `vscode/src/vs/base/parts/ipc/node/ipc.net.ts` | 设计：Unix socket/命名管道；理由：本机不需要端口密码登记 | OpenCode `opencode/packages/cli/src/services/daemon.ts` 的 TCP/password：增加当前不需要的状态 | 不为本机接线搭额外的网络服务。 |
| 协议 | 标准 Agent 与私有 UI 共存 | ACP v1 与原生 NDJSON 在 serve 内汇入同一运行时。 | Tomcat `tomcat/src/api/serve/types.rs`；Paperclip `paperclip/packages/adapters/gemini-local/src/server/acp.ts` | 设计：SDK 映射、relay 转发；理由：宿主不认识 Tomcat 内部协议 | 旧计划私有 ACP attach/requestId 扩展：本期不续跑，无消费者 | 平台说 ACP，界面说原生话，都交给同一个执行器。 |
| 环境 | 首个启动者污染其它会话 | 会话子进程用连接快照，服务凭据/代理用保存配置。 | Tomcat `tomcat/src/core/tools/primitive/bash_task.rs`、`tomcat/src/core/llm/auth.rs`；Paperclip `paperclip/packages/adapter-utils/src/acpx-engine/execute.ts` | 设计：逐会话注入；理由：保住宿主原有任务环境；项目 MCP 共享例外见 §6 | VS Code 私有 serve 继承整份父环境的旧路径（`tomcat-vscode-ext/src/serveClient/TomcatMessenger.ts`）：不能直接扩成共享默认 | 谁先开服务，不应决定别人的模型 key。 |
| 实时旁观 | 中途加入不能漏前缀 | 复用普通打开流程，在开始转发时只补发当前消息。 | Tomcat `tomcat/src/api/serve/{commands,event_pump}.rs`、`tomcat-vscode-ext/src/ui/webview/{provider,state}.ts`；Codex `codex/codex-rs/app-server/src/request_processors/thread_lifecycle.rs` | 设计：同一转发边界补正文/思考，历史按稳定 ID 合并；理由：已完成消息已有持久历史 | Codex 的整轮快照：本期不照搬；Continue `continue/extensions/cli/src/commands/serve.helpers.ts` 的历史轮询：拿不到半截正文 | 打开还是老流程，只补没听到的半句话。 |
| 控制/旁观 | 谁决定窗口能否写会话 | 服务在打开时返回 access；界面置灰、服务按白名单准入。 | Tomcat `tomcat/src/api/serve/{commands,types}.rs`、`tomcat-vscode-ext/src/serveClient/sessionRouter.ts`；计划第 5 节 | 设计：来源入会话头，控制者在服务内存，access 只作窗口显示结果；理由：三者寿命不同 | 扩展自己猜身份或另加 Host 意图拦截：重复授权逻辑，且可能阻断打开动作 | 界面负责提示，服务负责守门。 |
| 慢连接 | 界面卡顿不能取消任务或拖慢别人 | 每连接复用 writer；合并增量、满时丢消息增量并提示，不按慢读断开。 | Tomcat `tomcat/src/api/serve/writer.rs::spawn_writer`、`enqueue_frame`；`tomcat/src/api/serve/tests/writer_test.rs` | 设计：stdout 出口改为各连接 socket，背压策略复用；理由：慢读不是控制者离开 | 队列溢出即断开：会连带取消该连接控制的任务；另造重放/重连协议：本期不需要 | 卡的是画面，不该把活停了。 |
| 项目说明 | 宿主说明不应反复进入用户消息 | AGENTS.md 并入现有规则管线，每轮重读。 | Tomcat `tomcat/src/core/project_instructions.rs`；Codex `codex/codex-rs/core/src/agents_md.rs` | 设计：根→cwd，单文件上限、不占原规则预算；理由：不重复污染对话 | Codex 同文件的回退文件名/总量截断：本期不需要；Multica inline prompt 路线：重复写历史 | 项目手册放在环境里，不每轮冒充用户再说一遍。 |
| 宿主接入 | 是否修改团队系统核心 | 只新增各自标准适配器和登记项。 | Paperclip `paperclip/packages/adapters/gemini-local/src/server/acp.ts`；Multica `multica/server/pkg/agent/agent.go` | 设计：复用执行/停止/恢复流程；理由：控制断线取消已满足当前边界 | Paperclip `paperclip/packages/paperclip-runner/src/drivers/acpx/codex-acpx-driver.ts` 的 native 路线：为接回活动轮承担额外资格与恢复改造 | 不为了加一个 Agent 重写平台。 |

### 3.2 实施点（设计闭环，交付待执行）

实施点与计划 todo 一对多映射，测试编号指向 §8。每项细节见后续小节或指定图，不重复维护第二份进度看板。

| 实施点 | 交付范围（含交付物） | 主要代码落点（含落地点） | 验收锚点（示例） | 说人话 |
| --- | --- | --- | --- | --- |
| M0 项目说明与基线 | 本地分支；AGENTS.md 层级、预算、刷新 | `project_instructions.rs`、`chat/context.rs`、`chat/run_loop/mod.rs` | T01–T03；图 §3.2.1 | 先让 Tomcat 读懂宿主放在仓库里的说明。 |
| M1 共享服务与 ACP | 服务启停、连接状态拆分、relay、标准 ACP、真实停止与慢读隔离 | `serve/{runtime,connection,service}.rs`【新增】、`registry.rs`、`writer.rs`、`api/acp/`【新增】、`platform/process.rs` | T04–T10、T28；图 A.2、§3.2.2 | 先证明接得上、跑得动、停得干净；慢读不停任务。 |
| M2 上下文与旁观 | 环境/MCP/技能、唯一控制权、槽位回收、按连接筛事件与当前消息补发 | `chat/context.rs`、MCP 工厂链、Bash 工厂、`serve/{connection,commands,event_pump}.rs` | T11–T17；§3.2.3–4 | 不串环境，也不漏正在生成的消息前半段。 |
| M2 VS Code | 共享 Messenger、switch 回复传 access/origin、两层只读、停服管理 | `TomcatMessenger.ts`、`ServeConnectionSupervisor.ts`、`sessionRouter.ts`、`ui/webview/`、GUI | T17–T20；图 §3.2.5 | 还是原来的会话列表和聊天界面。 |
| M3 宿主适配器 | tomcat-local / tomcat backend、模型和技能登记、配置 UI | Paperclip adapters/server/CLI/UI；Multica agent/execenv/迁移 | T21–T24；§3.2.6 | 加菜单选项，不改平台调度。 |
| 最终验收 | 两宿主真实旅程、兼容回归、三处界面证据 | §8 的既有 runner、隔离夹具和文档 | T25–T27 | 一条完整用户路径跑完，再交付。 |

#### 3.2.1 AGENTS.md：按环境读取，不追加对话

发现起点是会话绝对 cwd，终点是最近的 `.git` 文件/目录；找不到则只看 cwd。已有 `.cursor/rules` 和资源目录 rules 的发现根不变。

```text
/repo/.git
/repo/AGENTS.md                  先读
/repo/packages/app/AGENTS.md     后读（cwd 或其祖先）
/repo/packages/app/src           会话 cwd
/repo/packages/app/src/ui/AGENTS.md   本期不向下扫描

AGENTS.md 原文 -> 现有 project_rules -> User Custom Instructions
                 每个外层任务构建 prompt snapshot 时刷新，不进 transcript
```

正文只去 BOM，不解析 YAML，不要求 alwaysApply。每文件最多 64KiB；超限、非 UTF-8、空文件、越界或硬拒绝均跳过并记现有 instruction summary。AGENTS.md 排在普通规则之前，不消耗其 `min(32000, context_budget/10)` 字符预算。

最终文件可为指向仓库内普通文件的链接；目录链不得借 symlink 越界。复用权限门与有界读取，并在打开后核对目标身份，不把路径预检查当作无竞态保证。无 `.git` 时 containment 根就是 cwd。多级说明总量未另设总上限，是明确取舍；小上下文影响见 §9。

**说人话：**读当前项目这一条线上的手册，改了下一轮生效；别把用户的历史聊天越塞越胖。

#### 3.2.2 服务启动、退出与控制交接

连接状态从 `ServeState`/注册表拆出；会话表按 sessionId 原子准入。`active_session_id` 和最后选择的 scope 属于连接，不能继续保留为全局选择。控制绑定以注册表中的 slot 为唯一事实源；连接的“控制会话列表”只能由它反查或派生，不能形成另一份可独立修改的权威状态。

```text
连接失败 -> 拿 serve.lock -> 再连接
                              +-- 已就绪：复用
                              +-- 确认残留：清理自己的 socket，启动服务
                                  -> 固定 cwd 到 agent 数据目录/serve
                                  -> Unix setsid / Windows 脱离 Job
                                  -> 监听就绪后才允许客户端继续

relay stdin EOF -> 请求本连接 drain -> 拒绝新任务/清空待发
                                    -> 停前台、子 agent、后台 bash、待确认
                                    -> 等工具退出和必要落盘
                                    -> stopped 确认 -> relay 正常退出
                                    -> 超时/通信失败 -> 非零退出，不能声称 stopped
```

- 锁文件保持稳定，不靠删除锁文件解锁；只在持锁且确认连接拒绝/目标确属残留 socket 时清理。权限拒绝、协议不兼容不是“服务不存在”。
- Unix 目录 0700/socket 0600；Windows 管道 DACL 限当前用户、拒绝远端。路径过长报错，不静默更换服务身份。
- `cancel_and_wait` 是复用起点，须组合 Bash registry stop/wait、级联子 agent、排队项和后台回调收口。已取消的连接不得被后台任务完成事件再唤起新轮。
- relay 优雅等待上限 10 秒；Multica backend 宽限应更长，例如 15 秒，并协调 WaitDelay。10 秒是总预算，不是每个子任务串行各等 10 秒。
- 连接断开即清空它的控制绑定并取消任务；**无控制者不等于已经停下**。新控制者仍须等原任务清理完成，停止失败返回 busy/stop_timeout，不 abort 掉句柄后假装空闲。原生旁观窗口不会自动接手，只能重新打开后由服务重新判断；ACP 来源在 VS Code 始终只读。
- 同一个连接可控制多个会话，另外当前显示一个会话（可为旁观）；关闭连接取消其全部控制会话，不按“目前显示哪一个”缩小取消范围。

**强杀例外：**宿主强杀 relay 或服务通信故障时，relay 无法在退出前取得确认；服务发现断开后再取消。宿主清理 worktree 与取消之间仍可能有窗口。本期不改宿主收尾状态机，不能承诺强杀/超时后目录绝对零写入；M1 必须测量并暴露失败，见 §9。

满槽位时，注册表串行决定淘汰最久未用的合格 slot：无运行任务、command job、排队输入、待确认、后台工作；不由 ACP 连接持有，也不是任何 VS Code 连接当前显示的会话，**当前显示为旁观也受保护**。扫描各连接的当前显示 sessionId 即可，不加旁观引用计数。选择与卸载期间禁止并发新准入，锁内不执行耗时 I/O；卸载复用现有清理并释放控制绑定，不加空闲定时器。

被卸载的外部会话从服务 Live 列表消失，不额外扫描其他目录的磁盘历史把它找回来；需要查看时回到宿主。自己目录内的会话仍可通过原有打开路径重新载入。两个槽位都被运行中或当前显示会话占住时返回 `too_many_sessions`，不能把正在看的旁观页踢回历史来腾位。

**连接输出与慢读（背压）。** 一个服务有多条连接后，不能共用同一个输出队列；也不能因某个窗口暂时读不动就把它断开，否则会误取消它控制的任务。

```text
会话事件 -> 按连接筛选 -> writer A -> socket A（读得慢）
                      |    合并增量 -> 堆满时丢消息增量 + 提示
                      |    连接保持，任务继续
                      +-> writer B -> socket B（照常接收）

真正 EOF / 写失败 / 明确离开 -> 原有断连收尾；不是慢读策略
```

- **复用，不重写背压。** 原 `spawn_writer` 已接收任意 AsyncWrite（`tomcat/src/api/serve/writer.rs:56-65`）；共享模式在各连接建立时各建 writer，原生出口从唯一 stdout 改为各自 socket。每个 writer 仍按会话排队，写 socket 不阻塞其它连接的事件入队；ACP 出口仍经 SDK 输出标准帧，复用背压分类时不能把原生帧直接混进 ACP 流，T28 覆盖两种入口。
- **慢读不算断连。** 沿现有 `enqueue_frame` 中先 `try_coalesce_tail`、再判阈值的顺序：能合并则合并；仍达到阈值时丢该连接的消息增量，并排入 `llm_notice(finishReason="backpressure")`，同段积压不逐帧刷屏。控制/响应/生命周期帧不按消息增量丢弃；Bash 预览沿原合并/截断规则。恢复读取后在原连接收到提示、保留的帧和后续事件，不强制重连、不取消任务。
- **不把丢帧说成无损。** `serve.max_buffered_frames` 是现有丢增量阈值，不是全连接帧数或字节数的硬上限；关键帧仍可积压。本文不承诺客户端无限停读时内存有界，也不新增慢读超时、踢连接或重放机制。实际写出失败仍须由连接层按断连清理，不能只记录日志后留下失效连接。

**测试锚点：**T28 在真实共享服务上暂停一条控制连接的读取，确认任务继续、另一条连接照常收流，再恢复读取。已有 `serve_writer_coalesces_deltas_under_pressure`、`serve_writer_never_drops_lifecycle`、`serve_writer_backpressure_notice_emitted_once`（`tomcat/src/api/serve/tests/writer_test.rs:101-183`）只证明 writer 的局部行为，不能替代这个多连接旅程。

#### 3.2.3 环境、MCP 和技能的归属

共享进程不能把启动者的环境当作所有会话的默认值。连接传环境快照，会话控制权交接完成后才绑定新快照；子 agent 和 reviewer/verifier 必须继承所属任务的执行上下文。

```text
连接环境快照 -> 会话执行上下文 -> Bash registry -> bash 子进程
                           +--> 会话级 MCP 管理器 -> MCP 子进程
                           +--> 连接技能 overlay -> prompt / load_skill / 子 agent

Tomcat 保存配置 -> 模型凭据 / 服务 HTTP 代理（不读首个宿主的 key）
项目级 MCP 缓存 -> 同 cwd 共享，首个创建者的环境是已知例外
```

MCP 仍沿现有白名单从快照继承 PATH/HOME 等启动变量，再叠加 server 显式 env；**不是把整份快照里的所有 token 都交给每个 MCP**。stdio MCP 缺省 cwd 必须落到会话/项目根，不能继承服务自己的 `serve/` 目录。ACP 传入的 MCP 独立于 cwd 缓存，结束后回收，项目信任门不变。

项目 MCP 保持既有按目录共享，使用首个创建它的会话快照；同 cwd 不同控制者无法保证不同项目 MCP 环境。连接私有 MCP 与 Bash 必须隔离，不能把这个例外扩散到它们。

技能在 new/load 时经现有 inventory 同步锁重扫，即使复用已加载 slot 也要检查。`--skills-dir` 只叠加给该连接的控制会话，不写共享 home、不放进共享 ScopeContainer 的技能集合；目录、提示目录与 load_skill 必须使用同一个合成视图，子 agent 同样适用。同名优先级拟定为连接目录→项目→agent→managed，覆盖仍保留诊断，不借技能扩大读写授权。

**发现的额外接缝：**scope 内插件 dispatcher 会捕获首个 primitive/Bash registry。插件 executeBash 也必须取当前调用会话的执行上下文，不能只改主 Bash 工厂就宣称隔离完成。只调整这条调用依赖，不另造插件系统。相关真实调用点见 §5、测试 T14。

#### 3.2.4 普通打开流程 + 当前消息补发

**需求与证据。** 目标架构中宿主正在执行的会话已在共享服务里：现有 `tomcat/src/api/serve/commands.rs:570-608` 有已加载分支，`tomcat-vscode-ext/src/ui/webview/provider.ts:4302-4319` 已按 switch→state→history 打开会话；助手正文和工具调用使用稳定 ID 落盘（`tomcat/src/core/agent_loop/tool_dispatcher.rs:350`、`tomcat/src/core/agent_loop/turn_finalize.rs:144`）。缺的是迟到连接从没收到过的当前消息前缀，不是另一份整轮历史。

```text
VS Code 点开会话
  -> switch_session  服务判断 control/observe，回复 access + origin，更新当前显示
       -> 若刚开始接收该会话：在事件之间补发当前消息正文/思考
          已收到 message_end：补发全文后也补 message_end
  -> get_state       只读既有运行状态，不另发一份访问方式
  -> get_messages    读已保存历史，按 messageId / toolCallId 合并
  -> 后续实时事件    继续走原来的 message_update / 工具事件
```

**转发范围：**每条连接只接收“它控制的会话”与“它当前显示的会话”的并集。切到后台但仍由本窗控制的会话继续收；切走仅旁观的会话就停止收；不在连接建立时广播全部宿主会话。只有从“不接收”变成“开始接收”时补发，已经在收的会话不能重复补前缀。

**补发的最小状态：**`event_pump.rs` 随转发累计当前 assistantMessageId 的正文与思考，并记住对应 message_end。开始转发和补发在同一串行化边界内完成，先把补发事件排进该连接队列，再排后续增量；磁盘和 socket I/O 不在锁内等待。每种非空内容用原有 `message_update` 分别补发，正文/thinking 类型和展示策略不变，不回放全会话事件日志。

下一条 `message_start` 才替换这条记录；`message_end` 只标记结束，不清空。它先于最终持久化完成也可能到达（`tomcat/src/core/agent_loop/stream_handler.rs:542`），因此“刚说完、下一条还没开始”的连接仍先收到全文，再收到结束事件。slot 卸载时释放记录，不建立已完成消息、工具状态和等待项的整轮副本。

**扩展侧复用：**现有 `tomcat-vscode-ext/src/ui/webview/state.ts:3320-3323` 可接住没见过 message_start 的更新，工具事件走 `upsertTool`，历史走 `rebuildHistoryTimeline`。本期不另造 renderer、快照解析器、偏移量/全局 seq 或独立合流协议。源代码里有这些分支不等于新场景已验证：T16/T19 必须覆盖补发与历史重叠、message_end 未落盘、切走再切回和带缓存的重连；若有缺口，只在原流程修正，不能恢复整轮快照方案。

**慢读与补发的边界：**本节“不重不漏”针对正常接收时的中途加入、历史合并与重连，不是背压下的无损承诺。触发 §3.2.2 的丢增量后，该连接的实时正文/思考（包括以 message_update 补发的前缀）可能不完整；恢复读取会收到提示和后续事件，不因慢读重连或取消任务。已完成并落盘的内容仍可通过原 `get_messages` 读取；不新增逐帧补传，也不承诺提示一到就自动补齐尚未落盘的消息。

#### 3.2.5 VS Code：原界面加只读来源

扩展用同一个 Messenger 连接共享服务；supervisor 负责连接重试，不把 dispose 等同于 kill/shutdown。明确 stop 的客户端停用自动重启；其它连接看到停服不应无条件把服务拉起，用户主动连接才进入启动流程。

**身份判断只在服务端。** 原生客户端打开会话时按下面的顺序判断；ACP 客户端自己的 `session/load` 仍走单控制者准入，不套用 VS Code 的旁观规则。

```text
switch_session(X)
  |
  +-- 会话头 origin.kind == "acp"？ -- 是 --> observe
  |           否
  +-- 被其它连接控制着？ ------------ 是 --> observe
  |           否
  +-- 本连接已有控制权，或无主且清理已完成 --> control

回复 { activeSessionId, access, origin }
  -> sessionRouter -> sessionPool -> provider -> 会话视图 -> GUI
     保留服务结果，不按 busy、客户端名称或消息 role 自己推断
```

停止中的任务尚未清理完成时，仍受 §3.2.2 的接手门限制，不能因控制绑定已清空就提前执行新任务。原生旁观窗口在原控制者离开后仍是 observe，**重新打开**才可能得到 control；ACP 会话即使宿主离线、服务重启后仍有来源记录，VS Code 再打开也只能 observe。

扩展必须把回复结果传完整：现有 `tomcat-vscode-ext/src/serveClient/sessionRouter.ts:246-253` 只返回 ID，`tomcat-vscode-ext/src/ui/webview/sessionPool.ts:30-32` 继续透传 ID，需一并带回 access/origin。`tomcat-vscode-ext/src/ui/webview/state.ts:2822` 的 `applySessionState` 不能在 get_state 刷新时把旁观改成 owned；已有 owned 标记应由 access 派生，不作为第二份可独立写入的控制权。

现有 `switch_session` 的已加载分支仍会持久化 current 选择（`tomcat/src/api/serve/commands.rs:572-578`），共享模式必须区分本连接的显示与会话控制状态；“复用旧接口”不表示将这个单窗口副作用原样照搬给旁观者。

外部列表只合并 `list_sessions(scope=live)` 中正在运行或仍加载的会话，并补 cwd/来源；已卸载的外部会话不在本期跨目录打开范围。切走、关闭旁观视图只结束该连接对它的接收，不触发全局 slot 清理；关闭当前控制会话仍走原有停止/清理路径。

```text
会话 [ 修复登录 · multica-agent-sdk · 运行中 v ]
工作目录：.../task-worktree
--------------------------------------------------
宿主：修复登录失败，并补测试。
Tomcat：正在检查……（持续增长）
工具：读取文件 ✓  运行测试 …
--------------------------------------------------
只读观察 · 来自 multica-agent-sdk
输入/发送/停止置灰；不向本窗口路由可作答的批准请求

控制方断开：正在停止 -> 已取消 / 停止失败
服务断开：状态待确认 [重新连接 / 重启服务]
```

只读只加两层：**GUI 按 access 置灰，服务在命令分发入口按 §4.3 白名单拒绝旁观写命令。** GUI 沿既有 `canPromptWhenConfigured`、`canInterrupt` 加非 observe 条件，不加新组件（`tomcat-vscode-ext/gui/src/App.tsx:1555-1557`）。批准请求只路由控制连接，不为旁观另造批准卡片或拦截层。

**不新增 Host 意图入口拦截。** 现有 `sessionBlocked` 处理删除/草稿退役，还会挡住打开会话（`tomcat-vscode-ext/src/ui/webview/provider.ts:1672-1674`、`tomcat-vscode-ext/src/ui/webview/provider.ts:4303`），不能把 observe 塞进这个条件；其原有保护保持不变。绕过置灰的写请求由服务拒绝，扩展沿原失败提示显示错误。

来源名只取创建时 ACP `initialize.clientInfo.name`，原样用于展示；缺省显示本地化的“ACP 客户端”，不凭 acpx 进程名猜平台。Multica 的实际值是 `multica-agent-sdk`（`../multica/server/pkg/agent/hermes.go:681-688`）。来源名、消息 role 和 busy 都不是权限依据；外部 cwd 也不自动授予执行权限。

#### 3.2.6 两个宿主只加标准适配器

Paperclip 新增 `packages/adapters/tomcat-local/`，复用 acpx 引擎的环境、MCP、权限、取消和 session codec。包内补临时技能目录生命周期和 CLI 参数引用；不改共享执行/恢复算法，不走 native runner profile。命令覆盖、模型探测和菜单表单沿 Gemini/Kimi 结构。

Multica 新增 `server/pkg/agent/tomcat.go` 实现既有 `Backend`，复用 hermesClient 的 JSON-RPC，不复制 Hermes 的 `session/resume` 方言；Tomcat 使用标准 `session/load`。setup 成功后、prompt 前上报 session ID；沿既有异步 pin 流程，不新增持久化事务。Cancel 自管：关 stdin→等待 relay→超时杀进程组，不直接复用默认立即 SIGKILL，也不另发适配器私有 cancel 协议。

任务说明登记为 AGENTS.md，技能目录登记为 `.agents/skills`；不加入 inline system prompt 提供者。Paperclip 的 CLI 临时技能目录只能作为实现参考：若每轮随机目录进入 `agentCommand`，acpx fingerprint 会改变，可能触发新会话。该组合列为 M1/M3 兼容验证项，不把“保留平台默认恢复行为”夸成“每次都原 ID 续聊”，也不擅改公共指纹规则。

## 4. 协议、数据与调用样例

标准 ACP 与 Tomcat 本机管理协议分开：前者给宿主，后者只在 relay/扩展与 serve 之间。所有新增命令名与字段均是拟定设计；实现的单一事实源为 `tomcat/src/api/serve/types.rs` 及新增 `tomcat/src/api/serve/local_protocol.rs`，由现有 schema/wire 流程导出，禁止手写第二份可独立修改的 TS Schema。

**说人话：**平台只听标准 ACP；服务怎么启动、怎么请某根连接先停手，是 Tomcat 自己内部的事。

### 4.1 本机握手与优雅离开

本机流先交换一行 JSON 握手，成功后切换为原生 NDJSON 或原样 ACP 字节流。握手不进入 ACP stdout。字段缺省与 null 依下表，未列出的扩展字段不得用于授权。

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| `type` | string | 是 | 无 | 请求 | `connect` 或 `drain_connection` | 连接还是通知离开。 |
| `protocolVersion` | integer | 是 | 无 | 两类请求 | 本机协议版本，非 ACP 版本；不匹配拒绝 | 先确认彼此听得懂。 |
| `agentId` | string | 是 | 无 | 两类请求 | 与服务身份一致；data root 已由端点确定 | 不能串 agent。 |
| `mode` | string | connect 必填 | 无 | connect | `native` / `acp` | 后面说哪种协议。 |
| `env` | object<string,string> | connect 必填 | 无 | connect | `{}` 明确空环境；省略/null 拒绝；大小受限、只驻内存 | 空快照不回退到别人的环境。 |
| `skillsDir` | string | 否 | 不叠加 | ACP connect | 缺省/null=无；值必须是可读绝对目录，检查权限/包含关系 | 可选的宿主私有技能。 |
| `instanceId` | string | drain 必填 | 无 | drain | 必须对应当前实例 | 旧连接不能误停新实例。 |
| `connectionId` | string | drain 必填 | 无 | drain | 目标为握手返回的连接；一次连接内唯一身份 | 只排空要离开的那根线。 |

响应结构为带标签联合：

```text
LocalReply
 +-- connected { protocolVersion, serviceVersion, instanceId, connectionId }
 +-- drained   { instanceId, connectionId, stopped: true }
 +-- error     { code, message }
```

以上响应字段全部必填、非 null；`protocolVersion` 为整数，`serviceVersion` 为版本字符串，`stopped` 为 boolean 且成功时只能是 true，其余为非空 string。错误无 `stopped:true`，日志/错误不包含环境值。握手采用有界读取与超时；实现阶段将界限作为内部常量，不新建用户配置开关。

**离开通知的落点：**为了不往标准 ACP 流里夹私有消息，relay 遇 stdin EOF 后在**同一端点**建立一次短管理连接，发送 `drain_connection`，原数据连接仍保持读出直到完成/超时。服务将目标标记 draining，调用统一取消清理，成功才返回 drained；该短连接自身不是控制者。它不是第二个服务或常驻控制通道，也不依赖跨平台半关闭行为。硬断开直接进入同一清理函数。来自本机当前用户的管理能力与 stop 服务处于同一授权边界，不是多租户 API。

```json
{"type":"connect","protocolVersion":1,"agentId":"main","mode":"acp","env":{},"skillsDir":null}
```

```json
{"type":"connected","protocolVersion":1,"serviceVersion":"<compatible-version>","instanceId":"i1","connectionId":"c7"}
```

```json
{"type":"drain_connection","protocolVersion":1,"agentId":"main","instanceId":"i1","connectionId":"c7"}
```

```json
{"type":"drained","instanceId":"i1","connectionId":"c7","stopped":true}
```

### 4.2 ACP v1：不增加私有恢复协议

ACP Schema 以官方 SDK 为准，本文不复制整套类型。已查官方 [agent-client-protocol 3.4.0](https://docs.rs/agent-client-protocol/3.4.0/agent_client_protocol/)：提供稳定 v1、ByteStreams 与可选 native I/O；ByteStreams 使用 `futures::io`。拟用 3.x 并锁定经两个宿主测试的版本，通过 Tokio compat 接流；不因“3.x”误启用实验协议 v2，不额外打开不需要的 subprocess/stdio features。

| 操作 / 关键字段 | 类型与默认/约束 | 映射与结果 | 说人话 |
| --- | --- | --- | --- |
| `initialize.protocolVersion` | integer，必填，v1；capabilities 按 SDK 默认 | 返回实际能力；不宣告未实现的文件/终端代理 | 先协商会做什么。 |
| `initialize.clientInfo.name` | 标准可选 clientInfo 内的 string | 服务记住创建方名称；session/new 时写入 origin.clientName，仅用于显示 | 记住谁开的会话，不拿名称当权限。 |
| `session/new.cwd` | string，必填绝对路径；null/不存在拒绝 | detached 新建；绑定当前连接，保存 ACP 来源 | 不借服务进程的目录开会话。 |
| `session/load.sessionId`、`cwd` | string，必填；校验原会话/作用域 | 等旧任务停止后回放持久历史；活控制者存在则拒绝 | 继续旧聊天，不恢复旧 RPC。 |
| `mcpServers` | array，按 SDK 要求显式传；无服务器用 `[]` | 接入会话级 MCP；参数结构由 SDK 校验 | 每个会话带自己的临时工具。 |
| `session/prompt.sessionId`、`prompt` | string + ContentBlock[]，必填非 null | 标准文本/内嵌资源映射到一次任务；更新走 session/update | 提交一次交办。 |
| `session/cancel.sessionId` | string，必填；notification 无 RPC 响应 | 停完后原 prompt 返回 cancelled；内部失败走标准 error | 发取消不等于已停，终态才说明结果。 |
| 模型选择 | 采用 SDK v1 和两个宿主实际支持的会话模型/config 入口 | 只影响该会话；未支持的选项明确拒绝 | 不改全局默认模型。 |
| `session/request_permission` | SDK 的 sessionId、toolCall、options 及 outcome | 路由当前控制者；optionId 校验并消费一次 | 批准不能串到另一个请求。 |

标准请求/响应样例（省略的是其它独立消息，不是给 ACP 添加字段）：

```json
{"jsonrpc":"2.0","id":1,"method":"session/new","params":{"cwd":"/work/repo","mcpServers":[]}}
```

```json
{"jsonrpc":"2.0","id":1,"result":{"sessionId":"s1"}}
```

```json
{"jsonrpc":"2.0","id":2,"method":"session/prompt","params":{"sessionId":"s1","prompt":[{"type":"text","text":"修复登录失败"}]}}
```

```json
{"jsonrpc":"2.0","id":2,"result":{"stopReason":"end_turn"}}
```

MCP 子结构不自造：stdio 类型包含 name/command/args/env；HTTP 类型只在客户端协商支持时启用。SDK Schema 负责类型和可选字段的缺省/null；Tomcat 再做路径、执行权限和会话隔离校验。用量只映射可得数据，SDK 无法表达的完整性放适配器已有日志/未知费用，不造私有账单字段。

`request_permission` 只处理权限，不拿它冒充结构化澄清问答。本期 ACP 不承诺原生 `ask_question` 的交互能力；未支持的澄清请求明确返回不可用错误并收口，不代答，也不永久挂起。

### 4.3 原生旁观：复用打开命令与消息事件

使用现有 `switch_session → get_state → get_messages`，不增加旁观专用请求或历史读取路径。**access 与 origin 由 switch_session 回复带回**；get_state 保持既有运行状态职责，当前消息通过原有事件补发。以下是共享模式拟定字段，原 stdio 行为不变。

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| `type` | string | 是 | 无 | 请求 | 现有 `switch_session` / `get_state` / `get_messages` / `list_sessions` | 打开、看状态、读历史都不换接口。 |
| `id` | string/null | 否 | 无关联 ID | 请求 | 沿现有 `Option<String>`；客户端调用显式传 ID 关联响应 | 多个请求各认各的回应。 |
| `sessionId` | string；state/messages 也可 null | switch 必填且非 null | state/messages 可沿用当前显示 | 会话请求 | 客户端在打开流程中显式传同一目标 ID；原 stdio 缺省兼容不变 | 别把别的页的历史接进来。 |
| `scope` | string/null | 否 | live | list_sessions | `live` / `disk`；外部会话只用 live，disk 保持原目录范围 | 不去别的 worktree 扫旧会话。 |
| `params` | object | 否 | `{}` | get_messages | 沿既有 GetMessagesParams；null 不等于省略 | 分页参数照旧。 |
| `params.attachmentMode` | string | 否 | inline | get_messages | `inline` / `reference`；扩展沿用 reference | 图片不因旁观另复制一份。 |
| `activeSessionId` | string | switch 成功回复必填 | 无 | switch_session payload，已有 | 本连接这次打开的目标 | 确认打开的是哪一页。 |
| `access` | string | 共享 switch 成功回复必填 | 无 | switch_session payload【新增】 | `control` / `observe`；服务按 §3.2.5 判定，仅存扩展视图内存 | 这扇窗口能操作还是只能看。 |
| `origin` | object/null | 共享 switch 回复、Live 摘要必填 | 原生/老会话为 null | switch_session payload、Live 摘要【新增】 | 来自会话头；对象为下列 kind/clientName，不是权限结果 | 来源重启后还在。 |
| `origin.kind` | string | origin 非 null 时必填 | 无 | origin 子结构 | 本期仅 `acp`；是服务判断来源的稳定代码 | 不看平台名字猜来源。 |
| `origin.clientName` | string/null | 否 | 缺省/null 均无名称 | origin 子结构 | ACP initialize 的 clientInfo.name；未提供/空名显示“ACP 客户端” | 名称只是给人看的。 |
| `cwd` | string/null | 共享响应必填 | 无 | get_state 已有；Live 摘要【新增】 | 共享会话返回已校验的绝对路径；旧 stdio 可空 | 标出任务在哪个目录。 |
| `busy` | boolean | 是 | 无 | get_state、Live 摘要已有 | 真实运行状态，不代表修改权限 | 正在跑，还是已停下。 |

字段落点证据：`tomcat/src/api/serve/types.rs:758-794`（命令）、`tomcat/src/api/serve/types.rs:204-213`（历史参数）、`tomcat/src/api/serve/commands.rs:604-608`（switch 回复）、`tomcat/src/api/serve/commands.rs:845-862`（Live 列表）。回复外壳继续使用 `ResponseFrame`（`tomcat/src/api/serve/types.rs:1522-1558`）；共享 switch 回复缺失或未知 access 不得默认成可写。`get_messages` 的 cursor/limit、附件处理和 `upToSeq` 占位保持原契约。

**旁观命令准入白名单：**

```text
get_state               get_messages              list_checkpoints
get_session_files       get_session_file_baseline preview_rewind
get_instruction_catalog switch_session            close_session
```

统一分发入口先按 `ServeCommand::session_id()` 与连接身份解析实际目标会话，再校验；支持省略 sessionId 的会话命令须解析到本连接当前会话，不能把省略当成绕过校验。旁观连接对该会话的其它命令一律回 `read_only_session`，包括 prompt、steer、interrupt、set_model、compact、delete_session；以后新增的会话命令默认拒绝，不维护写操作黑名单。全局列表/服务管理仍走原入口，不因当前旁观一页就禁用整条连接。

校验必须在任何会话副作用之前，并覆盖优先处理的 interrupt 和确认回复路径：现有 `tomcat/src/api/serve/stdin.rs:57-87` 分流 interrupt，而 `tomcat/src/api/serve/commands.rs:260-269` 先处理控制命令，不能只在后面的普通 match 中加守门。确认仍按待批准请求绑定的控制连接检查，不因为命令被允许读取就授予批准权。

白名单里的两个视图命令有明确边界：`switch_session` 可按 §3.2.5 重新判定原生会话的 access，但不会把 ACP 会话交给 VS Code；旁观 `close_session` 只表示“我不看了”，清掉本连接对它的显示/接收状态，回复沿用 `closed: true`，**不卸载、不取消、不改控制者**。控制者关闭自己的会话保留原清理路径。已有关闭调用见 `tomcat/src/api/serve/commands.rs:2533-2546`、`tomcat-vscode-ext/src/ui/webview/provider.ts:1974-1986`。

打开请求与回复示例（每个代码块是一条消息，不是批处理）：

```json
{"type":"switch_session","id":"open-1","sessionId":"s1"}
```

共享服务对 ACP 会话的回复；下两条查询不会重新判定 access：

```json
{"type":"response","id":"open-1","success":true,"sessionId":"s1","payload":{"activeSessionId":"s1","access":"observe","origin":{"kind":"acp","clientName":"multica-agent-sdk"}}}
```

```json
{"type":"get_state","id":"state-1","sessionId":"s1"}
```

```json
{"type":"get_messages","id":"history-1","sessionId":"s1","params":{"attachmentMode":"reference"}}
```

对照：另一次打开无主且已可接手的原生会话，回复为 control、来源为 null：

```json
{"type":"response","id":"open-native-1","success":true,"sessionId":"native-1","payload":{"activeSessionId":"native-1","access":"control","origin":null}}
```

绕过界面向 s1 发写命令时，仍用原生错误外壳拒绝：

```json
{"type":"response","id":"write-1","success":false,"sessionId":"s1","error":"read_only_session"}
```

补发仍用原有事件结构（`tomcat/src/core/agent_loop/stream_handler.rs:262-297`、`tomcat/src/core/agent_loop/stream_handler.rs:542-545`）。`assistantMessageEvent` 子结构的 `kind` 为 content_delta 或 thinking_delta，`delta` 为该类型截至接收起点的完整前缀；随后同 ID 的实时事件才是后续增量。message_end 不带该子结构：

```json
{"type":"message_update","sessionId":"s1","assistantMessageId":"a1","message":{},"assistantMessageEvent":{"kind":"content_delta","delta":"正在检查登录逻辑"}}
```

```json
{"type":"message_update","sessionId":"s1","assistantMessageId":"a1","message":{},"assistantMessageEvent":{"kind":"thinking_delta","delta":"先查看入口与测试","source":"summary"}}
```

```json
{"type":"message_end","sessionId":"s1","assistantMessageId":"a1","message":{}}
```

这三条示意只适用于“消息已结束，但下一条尚未开始”的补发；若仍在生成，只补正文/思考，不提前发结束事件。用户 prompt、已完成消息和工具结果从原历史接口恢复，工具后续状态走原有 toolCallId 更新；不新增用户消息广播或工具画面协议。补发不会重新执行 prompt，也不允许旁观者作权限回复。

背压提示沿用现有原生事件，不是任务终态，也不是新的协议字段：

```json
{"type":"llm_notice","sessionId":"s1","finishReason":"backpressure","message":"serve writer dropped message deltas under backpressure"}
```

证据：`tomcat/src/api/serve/writer.rs:194-216` 生成该提示，`tomcat-vscode-ext/src/ui/webview/state.ts:3253-3255` 的分支只追加 notice，不改变 busy；共享模式沿用该展示路径，不因提示触发重连。它描述的是该连接漏收了实时片段，不代表模型调用失败或任务已取消；ACP 侧不得直接透传这条原生消息，标准输出仍遵循 §4.2。

### 4.4 持久来源与兼容性

在 `SessionHeader` 新增可选对象 `origin`：ACP `session/new` 将 `{"kind":"acp","clientName":…}` 与会话头**一次写入**，原生新建不写，老文件省略（或 null）按原生读取。未提供 clientInfo.name 时省略 clientName，不把翻译后的兜底名称写进文件；未知 kind 或非法结构不能默认为原生可写。写入点复用 `create_session_with_activation`（`tomcat/src/core/session/manager/session_impl.rs:843-863`），头结构见 `tomcat/src/core/session/transcript.rs:132-148`。

下面只示意现有会话文件的第一行；没有新增来源数据库或第二份聊天文件：

```json
{"type":"session","version":4,"id":"s1","timestamp":"2026-10-20T10:00:00Z","cwd":"/work/repo","origin":{"kind":"acp","clientName":"multica-agent-sdk"}}
```

| 信息 | 保存位置 | 生命周期与用途 |
| --- | --- | --- |
| origin | 会话文件第一行 | 只在 ACP 创建时写；宿主断开、服务重启、后续 load 均不改来源 |
| 控制连接 | 服务内存里的会话绑定 | 接手时登记，断线清空；任务停止完成之前仍禁止新执行 |
| access | switch 回复、扩展会话视图内存 | 每次打开由服务计算，不落盘；旁观不会因对方离开而自动变 control |
| 消息 role | transcript 的每条消息 | user/assistant/tool 表示谁说了这句话，与控制/旁观权限无关 |

**两个身份例子：**ACP 会话即使宿主已断开，VS Code 再打开仍是 observe；原生会话由窗口 A 控制时，B 打开为 observe，A 关闭且清理完成后 B 也不会自动接手，B 重新打开才变 control。没有 origin 的旧会话同样走原生规则。

ACP 连接自己的 `session/load` 按唯一控制者与作用域规则处理，不套用 VS Code 的“ACP 来源只能旁观”规则，也不在 load 时重写来源。header/列表投影及既有 copy/restore 路径须保留来源，不把改路径当作来源转换。此约束是当前共享服务/UI 合同，不是对同用户任意旧 CLI 的新安全沙箱；前版字符串 origin 仅是未实施草案，不为它增加磁盘迁移或兼容分支。

## 5. 文件职责总览（One-Glance Map）

下面覆盖计划落点及本轮查证的必要调用链。路径图以 `tomcat/` 为根；【新增】不是已存在。实现若未改某个节点的签名，须在此标注只复用，避免将研究范围当实际 diff。

```text
入口
 src/api/mod.rs                         注册 acp 模块【修改】
 src/api/cli/mod.rs                     Serve 子命令、Acp/skills-dir【修改】
       |
       +-> src/api/acp/mod.rs           SDK v1 请求/更新映射【新增】
       |   src/api/acp/relay.rs         转发 + stdin EOF drain【新增】
       |   [src/api/acp/tests/]         协议/转发单测【新增】
       |
       +-> src/api/serve/service.rs     端点、启动锁、start/status/stop【新增】
       |      -> src/infra/platform/process.rs  setsid/Job 脱离【修改】
       |      -> src/infra/platform/fs.rs       私有权限/安全打开【复用或补管道支持】
       |      [src/infra/platform/tests/]
       |
       +-> src/api/serve/local_protocol.rs      握手/drain 类型【新增】
           src/api/serve/connection.rs          每连接状态/取消归属【新增】
           src/api/serve/runtime.rs             服务装配/调度/清理【新增】
           src/api/serve/mod.rs                 保留 stdio 兼容入口【修改】
                  |
                  +-> registry.rs               唯一 slot/控制者、满槽淘汰【修改】
                  +-> stdin.rs / control.rs     统一只读准入含优先中断、确认回复路由【修改】
                  +-> commands.rs               switch 回复 access/origin；旁观 close 只退订【修改】
                  +-> confirmation.rs / ask_question.rs  按控制连接路由、取消【修改】
                  +-> types.rs / schema.rs      switch 访问结果、来源对象及命令白名单【修改】
                  +-> event_pump.rs            按连接筛事件、当前消息补发【修改】
                  +-> writer.rs                每连接实例/出口；背压算法【复用】，不按慢读断开
                  |   [serve/tests/event_pump_test.rs] 补发时序/消息替换/过滤【扩展】
                  |   [serve/tests/writer_test.rs] 合并、提示、关键帧与 Bash 预览【回归】
                  |   [serve/tests/]            runtime/connection/service 等单测
                  |
                  v
会话装配与持久状态
 src/api/chat/context.rs                cwd、执行环境/技能/MCP 装配【修改】
 src/api/chat/session_runtime.rs        会话私有执行依赖【修改】
 src/api/chat/resource_inventory.rs     受锁保护的 new/load 重扫【修改】
 src/api/chat/run_loop/mod.rs           prompt snapshot、合成技能视图【修改】
       +-> src/core/project_instructions.rs    AGENTS.md 发现/预算/安全读取【修改】
       +-> src/core/skill/{discovery,model,load}.rs
       |                                  连接目录合成、完整加载路径【修改】
       +-> src/core/session/transcript.rs      可选 origin 对象，缺省兼容原生【修改】
       +-> src/core/session/manager/session_impl.rs  创建头时一次写来源；复制保持【修改】
       |   src/core/session/manager/context.rs       hydrate【已加载分支不重复调用；复用】
       |   src/core/session/usage_guard.rs           原共享锁【只复用，不升级】
       +-> src/infra/events/mod.rs             原有 message_update/end 与工具事件【只复用】
       [src/api/chat/tests/、src/core/tests/、src/core/skill/tests/]
       [src/core/session/tests/、src/api/serve/tests/instruction_prompt_test.rs]
                  |
                  v
会话进程与资源
 src/core/tools/primitive/registry_builder.rs    Bash 工厂传环境【修改】
 src/core/tools/primitive/bash_task.rs           统一 spawn、stop/wait【修改】
 src/core/tools/primitive/executor/bash.rs       fallback 同源环境【修改】
 src/core/plan_runtime/{prod_reviewer,verify}.rs  子代理工厂继承【修改】
 src/ext/dispatcher/ops.rs                       executeBash 取调用会话依赖【修改】
 src/core/connector/mod.rs                       MCP 工厂：scope/会话分开【修改】
 src/core/connector/mcp/{manager,lifecycle,connection,transport}.rs
                                                环境传递、重连同源、cwd【修改】
 src/core/llm/{auth,resolver}.rs                  共享模式 managed-only【修改】
 src/infra/http_client.rs                        共享模式禁止 env proxy fallback【修改】
 src/core/llm/http_client.rs                     传递对应策略【修改】
       [primitive/tests/、executor/tests/、plan_runtime/tests/]
       [mcp/tests/、llm/tests/、ext/dispatcher/tests/]
                  |
                  v
 tomcat/tests/{serve_shared_e2e,acp_stdio_e2e,acp_observer_e2e}.rs【新增】
 Cargo.toml / Cargo.lock：SDK 3.x；tokio-util compat feature，按实际依赖验证
```

```text
服务协议/schema
  -> tomcat-vscode-ext/src/serveClient/wire.d.ts             既有生成产物，不手改
  -> src/serveClient/TomcatMessenger.ts                     连共享服务，不自有 kill
  -> src/serveClient/ServeConnectionSupervisor.ts           连接重试与显式停服分离
  -> src/serveClient/sessionRouter.ts                      switch 返回 ID + access + origin
  -> src/ui/webview/sessionPool.ts                         透传打开结果，不丢身份字段
  -> src/ui/webview/{provider,state,protocol}.ts             把 access/origin 记入视图；不加意图拦截
  -> gui/src/App.tsx 及现有会话栏/输入组件                   按 access 置灰并显示来源
  -> 各邻近 tests/ + tests/external_session_observation.test.ts【新增】
  -> src/test/suite/host.e2e.test.ts                         真服务 devhost 场景

宿主
  Paperclip packages/adapters/tomcat-local/【新增】
    -> server/src/adapters/{registry,builtin-adapter-types}.ts
    -> packages/adapter-utils/src/acpx-engine/constants.ts  仅 agent 名映射登记
    -> packages/shared/src/constants.ts                    内置类型登记
    -> cli/src/adapters/registry.ts + ui/src/adapters/registry.ts
    -> ui/src/adapters/adapter-display-registry.ts
    -> ui/src/components/new-agent/NewAgentSetup.tsx + ui/src/lib/agent-setup-fields.ts
    -> 相应 package.json/锁文件 + 适配器/UI 测试

  Multica server/pkg/agent/tomcat.go【新增】
    -> pkg/agent/{agent,models,version}.go                  工厂/模型/最低版本登记
    -> internal/daemon/{agents_probe,config}.go             探测/默认命令
    -> internal/daemon/execenv/{runtime_config,context}.go   AGENTS.md/技能目录
    -> packages/core/types/agent.ts + 新 protocol_family CHECK 迁移
    -> scripts/agent-cli-command-names.txt                  测试命令隔离名单
    -> tomcat_test.go【新增】及既有注册表/execenv 测试加行
```

阅读顺序：入口把连接转换为会话操作，会话装配再把执行上下文交给工具和资源；事件沿原生或 ACP 出口返回。**说人话：**先找谁接电话，再找谁干活，最后找谁把结果送回去。测试按 [UNIT_TEST_LAYOUT_SPEC](../openspec/specs/guides/testing/UNIT_TEST_LAYOUT_SPEC.md) 放父模块的 tests/，不在业务文件堆测试；图中并列文件须在实施时逐一核对，未实际改动的改标“只复用”。

## 6. 配置与环境变量

一般既有配置遵循 **env > config > 默认**；但必须显式记录本方案的共享模式例外，不能为了套模板重新引入启动者污染。

| 项 | 取值 / 来源 | 优先级与边界 | 说人话 |
| --- | --- | --- | --- |
| 服务身份 | 规范化数据目录 + agent | 复用现有 CLI 配置解析；不受会话 cwd/任务 env 改写 | 所有人要找到同一台服务。 |
| `primitive.auto_confirm` | 现有配置，默认 true | 沿现有加载逻辑；保存的 false 不被接入层覆盖；硬禁令不变 | 不因为接上平台就悄悄改权限。 |
| `serve.max_sessions` | 现有配置，默认 16 | 满才淘汰；任何连接当前显示的会话（含旁观）不可淘汰，不引入 idle unload 定时配置 | 用完位子才腾没人看的闲位子。 |
| `serve.delta_coalesce_ms` | 现有配置，默认 25ms | 每连接 writer 复用相邻增量合并窗口；0 禁用合并，不改变连接寿命 | 把小片段并起来，少发几帧。 |
| `serve.max_buffered_frames` | 现有配置，默认 64 | 每连接内按会话缓冲判定丢增量阈值；不是所有帧的硬上限，也不是字节上限；不触发断连 | 堆满先降级直播，保留控制和结束信息。 |
| Bash 环境 | 控制连接快照 | 清继承后应用快照，再加 Tomcat 必需内部标志；新控制者接手后换快照 | 任务工具用自己的 PATH 和变量。 |
| MCP 环境 | 快照白名单 + server 显式 env | 显式配置覆盖白名单；stdio cwd 明确绑定；不透传整份秘密 | 工具只拿它该拿的环境。 |
| 模型 key | Tomcat 保存的凭据 | **共享模式例外：不回退启动者进程 env**；删除保存 key 后不可复活旧副本 | shell 里有 key 不代表共享服务可用。 |
| 服务 HTTP 代理 | Tomcat 保存配置 | **共享模式例外：显式禁用 reqwest 自动 env proxy/NO_PROXY 影响**；旧 stdio/chat 不变 | 不借第一个宿主的代理设置。 |
| `--skills-dir` | 可选绝对目录 | 连接私有，new/load 重扫；不写共享 home | 临时技能不会跑进别人的会话。 |
| 项目规则 | AGENTS.md + 现有 alwaysApply rules | AGENTS 根→cwd 在前，原 rules 在后；各自预算不混用 | 项目手册完整读，原规则预算不变。 |
| 开发依赖 | 官方 ACP SDK 3.x | 只选稳定 v1；新依赖/feature 进入 Cargo.lock 并检查许可证、MSRV | 不为两个接口引入整套网络框架。 |

背压参数沿用 `tomcat/src/infra/config/types/runtime.rs:22-39`，不新增配置项或队列层。内部 git/rg、插件 secrets、MCP HTTP/OAuth 等仍有自己的环境读取点，不能凭主 Bash 已改就宣布“所有进程绝不受启动者影响”。共享模式装配需审计这些调用；对于本期只保持既有语义的路径明确记录，不扩大权限。项目 MCP 缓存例外不应被测试描述成完全隔离。

## 7. 错误模型、警告与取消结果

成功、取消、通信丢失和停止失败必须分开。服务内部原因映射到现有原生 error 或 SDK 标准 ACP error，不新增宿主任务状态，也不把错误文字当判断码。

```text
参数/身份/权限不合法 -> error，执行前失败
slot 被占/容量不足   -> session_in_use / too_many_sessions，不抢占
权限拒绝             -> 标准拒绝结果，工具零副作用
任务正常完成         -> 真正整轮完成后回报成功
取消并确认停止       -> cancelled；观察者显示已取消
控制连接丢失         -> 服务开始取消；客户端自身只能显示状态待确认
停止超时/残留工具    -> stop_timeout；relay 非零退出，保留占用
连接读得慢/输出积压  -> 合并增量；满时丢消息增量并提示，连接与任务保持
AGENTS.md 读取失败    -> 跳过该文件 + instruction summary 诊断
```

| 归一化原因 | 行为 | 说人话 |
| --- | --- | --- |
| protocol/version mismatch | 拒绝连接，提示升级/明确重启；不另起第二实例 | 版本不合就说清楚。 |
| invalid cwd / scope / origin | new/load 或控制操作失败；不回落服务目录 | 目录和会话身份不能猜。 |
| session_in_use | 原控制者仍活着则拒绝；停止中也不接手 | 不能两个人同时下指令。 |
| read_only_session | GUI 按 access 置灰；服务在分发入口拒绝白名单外会话命令，扩展沿原失败提示显示 | 绕过按钮也不能改，不重复加 Host 拦截。 |
| backpressure（提示，不是错误） | 每连接沿原 writer 合并、满时丢消息增量并提示；关键帧保留，不断开、不取消、不重连 | 直播可能缺片，任务仍在做。 |
| stop_timeout | 不报 stopped、不释放未停止状态；记录残留任务 ID | 退出码和“已停手”要对得上。 |
| connection_lost / service_unavailable | 按连接角色取消或退订；不直接显示成功/已取消 | 看不到结果不等于结果已完成。 |
| instructions_skipped | 沿摘要给出路径和理由，不把正文写日志 | 不能静默丢掉手册。 |

panic/JoinError 作为该连接或会话失败收口，触发相同清理；不可让一个连接解码/handler 的失败直接结束整个服务。进程级崩溃仍会中断全部任务，不承诺热恢复。

## 8. 测试矩阵与验收

**本轮只写技术文档，没有执行以下功能测试。** 状态统一 PENDING。标【新增】的函数/用例名是实施时应建立的验收锚点，不冒称当前存在；其余标“扩展/回归”的锚点来自已查文件。每条关键交付均有对应项。

| 维度 / 编号 | 用例 / 编号 | 状态 | 说人话 |
| --- | --- | --- | --- |
| P0 规则 T01 | 【新增于现有文件】`tomcat/src/core/tests/project_instructions_test.rs::agents_md_root_to_cwd_and_worktree_boundary` | PENDING | 根与子目录顺序正确，不越仓库；无 git 只看 cwd。 |
| P0 读取安全 T02 | 【新增】同文件 `agents_md_symlink_size_utf8_and_budget`；扩展 `rejects_leaf_and_ancestor_links` | PENDING | 链接越界、硬拒绝、64KiB、UTF-8、BOM、普通 YAML 分隔符；40KB 不占 rules 预算。 |
| P1 逐轮刷新 T03 | 扩展 `tomcat/src/api/serve/tests/instruction_prompt_test.rs::project_rules_refresh_without_tool_grants` | PENDING | 改手册下一轮生效，不进历史，不生成工具授权。 |
| P0 单实例 T04 | 【新增】`tomcat/tests/serve_shared_e2e.rs::concurrent_start_reuses_one_instance` | PENDING | 10 个启动者只得到一个服务；残留、权限错误、版本不符分别处理。 |
| P0 平台 T05 | 【新增】同文件 `service_survives_launcher_group_exit`；扩展 `tomcat/src/infra/platform/tests/process_test.rs` | PENDING | 真进程组/Windows Job 被杀，服务活着；服务 cwd 不占 worktree。 |
| P0 路由 T06 | 【新增】同文件 `connection_routing_and_control_handoff` | PENDING | 同请求 ID 不串线，同 session 只有一个控制者；断线清空绑定但清理完成前仍不能接手，停止超时也不能放行。 |
| P0 停止 T07 | 【新增】`tomcat/tests/acp_stdio_e2e.rs::relay_eof_waits_for_tools_before_exit` | PENDING | grace 成功退出后文件不再写；覆盖后台 bash、子 agent、排队和等批准。 |
| P0 停止失败 T08 | 【新增】同文件 `forced_exit_and_drain_timeout_are_not_success` | PENDING | 硬杀触发取消；超时不能报 stopped，也不能放行第二执行者。 |
| P0 ACP T09 | 【新增】同文件 `standard_acp_new_load_prompt_cancel` | PENDING | 官方客户端与 hermesClient 均可握手/执行/取消/load；stdout 只有协议。 |
| P0 权限 T10 | 【新增】同文件 `permission_false_requires_valid_controller_reply` | PENDING | false 保存、批准前不执行、拒绝/取消/断开、旧回复；旁观连接收不到可作答的权限请求，伪造回复也无效；同时测默认 true 和硬禁令。 |
| P1 容量 T11 | 【新增】`tomcat/tests/serve_shared_e2e.rs::evicts_only_eligible_idle_slots` | PENDING | 2 个位子跑 5 个任务；不淘汰忙/排队/任意窗口当前显示（含旁观）的会话；腾不出报错。卸载后外部 Live 项消失，自己目录内会话可重新载入。 |
| P0 环境 T12 | 【新增】同文件 `session_env_is_independent_of_service_starter` | PENDING | 两种启动顺序下主 Bash、子 agent、fallback 路径拿自己的环境。 |
| P0 MCP T13 | 扩展 `tomcat/src/core/connector/mcp/tests/transport_test.rs`；【新增】`acp_stdio_e2e.rs::session_mcp_isolated_and_project_mcp_shared` | PENDING | ACP MCP 不串环境；白名单保留；项目共享例外有明确断言。 |
| P0 插件 T14 | 【新增】`tomcat/src/ext/dispatcher/tests/session_bash_env_test.rs::plugin_bash_uses_calling_session_env` | PENDING | 同目录不同连接的插件 Bash 不用第一个会话的 registry。 |
| P1 技能 T15 | 扩展 `tomcat/src/api/chat/tests/resource_inventory_test.rs`；【新增】`serve_shared_e2e.rs::new_load_refreshes_skills_without_overlay_leak` | PENDING | 新技能被看见，目录卡片和 load_skill 同源，私有技能不进别人目录。 |
| P0 流一致 T16 | 【新增】`tomcat/tests/acp_observer_e2e.rs::join_mid_message_then_merge_history`；扩展 `tomcat/src/api/serve/tests/event_pump_test.rs` | PENDING | 正常接收、未触发背压丢弃时，按 switch→state→history 加入；正文/思考前缀不重不漏；刚结束及未落盘时补全文+结束，下一条开始才替换。工具及 prompt 从历史正确重建；慢读降级另见 T28。 |
| P0 旁观 T17 | 【新增】`tomcat/tests/acp_observer_e2e.rs::switch_session_observes_without_taking_control`；扩展 `tomcat/src/api/serve/tests/commands_test.rs` | PENDING | ACP 在宿主在线/离线及服务重启后均 observe，来源不被 load 改写；原生 A 控制时 B 为 observe，A 离开后 B 须重开才 control；旧会话头缺 origin 为原生。9 条白名单可用，其余写命令回 read_only_session，含省略目标与优先中断；旁观 close 后停收但任务继续，宿主 load 不被阻挡。开关旁观不多调模型，未加载的跨目录会话不另开入口。 |
| P1 连接 T18 | 扩展 `tomcat-vscode-ext/src/serveClient/tests/{ServeConnectionSupervisor,messenger_disposal}.test.ts` | PENDING | dispose 不 kill 服务，关窗取消本窗控制任务，显式停服不被重连拉起。 |
| P0 合流/只读 T19 | 扩展 `tomcat-vscode-ext/src/ui/webview/tests/dual_channel.test.ts`、`tomcat-vscode-ext/gui/src/App.test.tsx` | PENDING | switch 的 access/origin 经 router/pool 进入视图；get_state 不覆盖身份。observe 时输入/发送/停止置灰、来源及无名兜底正确，control 照旧；关旁观标签不停任务，服务拒绝写命令时显示原失败提示。补发与历史重叠、切回、带缓存重连无重漏，未显示且非本窗控制的会话不串流。 |
| P1 扩展集成 T20 | 【新增】`tomcat-vscode-ext/tests/external_session_observation.test.ts`；扩展 `src/test/suite/host.e2e.test.ts` 的 `shared service observation` | PENDING | 实际 CLI + 受控模型 + VS Code，中途打开、worktree、重载。 |
| P1 Paperclip T21 | 【新增】`paperclip/packages/adapters/tomcat-local/src/server/{execute,test,skills}.test.ts` | PENDING | 标准 ACP、环境探测、临时技能真正可用、取消及失败。 |
| P0 Paperclip 恢复 T22 | 【新增】同包 `execute.test.ts` 的 `skills_command_identity_and_resume`；回归共享引擎 `operator-stop.test.ts` | PENDING | 临时目录是否改变指纹必须实测；区分保存后的续聊与未保存后的新会话。 |
| P1 Multica T23 | 【新增】`multica/server/pkg/agent/tomcat_test.go::TestTomcatGracefulCancelAndResume` | PENDING | 提前报告 ID、load；关 stdin，grace 内不发 kill，超时才杀进程组。 |
| P1 登记 T24 | 扩展 `multica/server/pkg/agent/agent_supported_types_test.go`、execenv runtime_config/skills 表；【新增】Paperclip `NewAgentSetup.tomcat.test.tsx` | PENDING | 菜单、配置、模型、协议族迁移、任务说明和技能目录齐全。 |
| P0 真实旅程 T25 | 两宿主各一条受控真实执行 E2E，复用 T20 devhost；场景名 `host task → observe → stop → reload/new per host policy` | PENDING | 停止后 worktree 正常收尾；Multica 按 AGENTS.md 回报状态。 |
| P1 兼容 T26 | 回归 `tomcat/tests/{serve_stdio_e2e,serve_multi_session,serve_ask_question_tests,serve_robustness_tests,session_concurrency_tests}.rs` 及两宿主既有套件 | PENDING | 旧模式/其它 Agent 不变；注册枚举断言允许加 Tomcat 行，不改原业务预期。 |
| P1 UI T27 | VS Code、Paperclip、Multica：真实页面 PNG + ARIA + console | PENDING | 看得到、点得对、无浏览器错误，不能以 DOM 或构建代替。 |
| P0 慢连接 T28 | 【新增】`tomcat/tests/serve_shared_e2e.rs::slow_connection_does_not_cancel_tasks`；回归 `tomcat/src/api/serve/tests/writer_test.rs`，扩展 `tomcat-vscode-ext/src/ui/webview/tests/dual_channel.test.ts` 的 notice 用例 | PENDING | 原生/ACP 控制连接分别暂停读取并实际压满输出；它不被断开、任务继续，另一连接正常收流。恢复后同一连接收到提示和后续/终态，关键帧不丢；原生提示不改 busy、不触发重连，ACP stdout 保持标准协议。消息落盘后 get_messages 可读全文；真正断开仍按 T06/T07 取消。 |

### 8.1 执行批次与文档检查边界

M1 先跑新增共享/ACP 目标和最邻近单测，包含 T28 的慢控制连接与恢复读取；M2/M3 再扩前端与适配器。命令以实施时 manifest 为准，计划中已有入口如下：

- Tomcat：`cargo nextest run --test serve_shared_e2e --test acp_stdio_e2e --test acp_observer_e2e`，新增目标登记 nextest/test-groups，再按共享核心改动补已有完整门禁。
- 扩展：`npm run test:unit:core`、`npm run test:unit:gui`、定向 integration、真实 devhost，最终补 lint/build。
- Paperclip：新增适配器与 UI 的定向 Vitest，再 `pnpm -r typecheck`、`pnpm test:run`、`pnpm build`。
- Multica：沿 `scripts/go-test-with-agent-cli-guard.sh` 定向 Go 测试，agent 包按既有脚本限并行；最终 `make check`、`pnpm lint`、`make build`。

复用仍有效的同批结果，不为每个 todo 重跑全套。功能验收使用隔离 HOME、受控模型、测试数据库；真实付费调用需另行授权。此次文档交付只检查规范骨架、链接、计划范围及 diff，不以任何文档检查代替上述 PENDING。

## 9. 风险与应对

风险不是额外需求；这里记录当前边界下仍需验证或不能保证的部分。**说人话：**把可能失手的地方和处理动作提前写清，不靠“应该没事”收尾。

| 风险 | 影响 | 应对（具体动作） | 说人话 |
| --- | --- | --- | --- |
| 来源与访问方式混淆 | 宿主离线后误变可写，或原生窗口抢走控制权 | origin 只在头里保存，access 由 switch 回复给窗口；clientName/role/busy 均不参与授权。T06/T17/T19 覆盖断线、重开、老文件及 get_state 刷新；控制绑定清空不绕过停止等待 | 来源是历史事实，控制者是当前状态，窗口拿到的是判断结果。 |
| 外部 Live 列表在点击前已变化 | 点击已卸载会话失败或越界找历史 | 打开时再查同一服务注册表；失败刷新列表并说明会话已卸载，不扫描其他目录补救；当前显示会话受槽位回收保护。T11/T17 覆盖此边界 | 列表里的任务已卸载，就回宿主查看。 |
| 强杀 relay / drain 超时后宿主立即清理目录 | 工具取消与 worktree 收尾竞态 | 10s 总宽限内真实 stop/wait；超时非零、保留停止中、日志带任务 ID；T08 注入延迟和持续写文件。若宿主把失败仍当安全收尾，M1 上报边界，不宣称绝对零写入 | 正常退出能等停手，强杀不能保证这个顺序。 |
| 共享 usage 锁不能跨独立进程排斥执行 | 旧 chat/stdio 可并行打开同一历史 | 不升级锁；文档说明不要同时用旧入口执行共享服务正控制的 session。旁观已加载 slot 不重复 hydrate；T26 保留原并发/删除保护 | 本期解决一个服务内的控制权，不改整台机器的执行锁。 |
| 项目 MCP 首个创建者环境复用 | 同 cwd 多连接的项目 MCP 不完全隔离 | 明示例外，T13 锁定行为；需要不同凭据时使用会话级 MCP。出现实际冲突再评估缓存键，不能先宣称全隔离 | 共享工具和任务私有工具要分开说。 |
| 插件 dispatcher / 子代理工厂仍捕获旧环境 | 主 Bash 正确但旁路串环境 | 审计 §5 全部构造点，T12/T14 覆盖两连接同目录；不把单个 env_clear 当整体证据 | 不只改最显眼的那一条路。 |
| Windows shell/PATHEXT/Job 差异 | 路径解析仍受服务环境影响或无法脱离 | 解析程序时使用任务的 PATH/PATHEXT；实际 Windows Job 测试；不支持脱离就提示预启动服务 | Windows 不能只拿 Unix 测试代替。 |
| 补发与历史/旧缓存重叠 | 重字、消息假运行或工具状态倒退 | §3.2.4 在开始转发处先补发，复用原历史合并；T16/T19 暂停在消息结束/落盘交界，并覆盖已有部分缓存再接收。源码有按 ID 合并不等于已通过这些时序，实测前不承诺 | 刚说完和第二次进来，也得看得正确。 |
| Paperclip 临时 skills 路径进入 fingerprint | 续聊被判新配置、新会话 | 包内测试命令指纹与 session codec；保留宿主既有 fresh 行为并明确提示；若要求稳定续聊且现有扩展点做不到，实施前报告，不修改公共恢复引擎绕过 | “技能能用”和“还能续聊”要一起测。 |
| shell-only key 用户失去凭据 fallback | 共享服务不能调用模型 | 复用缺 key 引导迁入保存凭据，检查删除 key 后不复活；旧 stdio/chat 保留原行为 | 这是明确兼容变化，不是神秘连接失败。 |
| AGENTS.md 深目录累计过大 / 路径竞态 | 上下文压力或意外内容进入提示 | 单文件有界、目标权限/身份校验、现有摘要诊断；T02 覆盖链接替换/非法内容；累计超出小模型能力再修订总量策略 | 不截半份手册，也不承诺所有大仓库零成本。 |
| 协议/schema 变更及版本错配 | 老扩展误用共享接口 | Rust 类型单源生成，握手版本不合即拒绝；保留旧 stdio；回滚前明确停共享服务，不自动转私有执行器 | 不能连不上就偷偷换架构。 |
| 慢读导致直播缺片或关键帧积压 | 画面不完整；无限停读时内存不能承诺有界 | 沿 §3.2.2 每连接复用 writer，合并/丢消息增量并提示，不按慢读断开；T28 验证任务继续、连接隔离、恢复读取与落盘历史。仅当真实持续积压暴露资源问题时再修订策略，不提前加踢连接或重放机制 | 卡画面不等于停任务，也不把降级说成无损。 |
| handler panic、真实 I/O 失败、资源泄漏 | 单连接影响全服务 | 捕获任务 JoinError 和实际写出错误，交连接层执行统一清理；满槽位回收排除后台工作；T05/T06/T11/T17 故障注入；不得把慢读压力当 I/O 失败 | 断的是坏掉的连接，不是暂时读得慢的连接。 |
| Unix socket 残留 / Windows 命名管道抢占 | 连错实例或误删文件 | 锁内复查、验证目标类型/用户权限，拒绝非 socket/异常 reparse；握手实例/版本核对 | 不把任何同名文件都当垃圾删掉。 |

## 10. 历史决策、跨文档关系与后续规划

本节保留被取代方向，防止实施时把旧机制带回来。当前代码仍以现有 stdio 文档为基线，本文不是已上线声明。

- ~~宿主断开后任务继续、requestId 登记与接回原轮~~ → 否：本期接受取消，只做适配器；不实现私有恢复扩展。
- ~~Paperclip native runner ACPX profile~~ → 否：是为活动轮恢复选的；本期走普通适配器。
- ~~TCP 端口、发现文件、令牌、全局事件序号~~ → 否：本机 IPC + OS 权限、在开始转发处补当前消息足够当前需求。
- ~~独立 ObserverClient、逐进程旁观端口、跨进程独占锁升级~~ → 否：共享 Messenger 和原锁足够本期约定范围；不把遗留边界说成新安全保证。
- ~~整轮快照、`observe_session` / `unobserve_session`、`currentTurn` 响应与 `serve/observation/`~~ → 本轮删除：复用 `switch_session → get_state → get_messages`，只在 `event_pump.rs` 补发当前消息。
- ~~旁观专用只读历史查询、卸载后跨目录读宿主历史~~ → 本轮删除：外部会话只看服务 Live 列表；当前显示的会话（包括旁观）不能被槽位回收。
- ~~新建 live 基线合流协议、用户消息广播、整轮工具/等待投影~~ → 本轮删除：沿现有历史与事件合并，补发的时序保证由转发处负责，缺口通过原路径的回归测试定位。
- ~~get_state 返回 readOnly、字符串 origin、GUI/Host/服务三层重复只读判断~~ → 本轮替换：switch_session 返回 access 与来源对象；界面置灰、服务统一白名单，Host 只传结果和展示错误。
- ~~无控制者就自动把旁观窗口变成控制者~~ → 否：原生会话重新打开才重算，ACP 来源在 VS Code 始终只读；停止完成之前不得接手。
- ~~慢队列溢出就断连，再重连补发~~ → 本轮删除：沿用每连接 writer 的合并/丢增量/提示；慢读不断开，避免把画面卡顿变成控制任务取消。真实 EOF/写失败的断连语义不变。
- ~~把宿主说明拼进 prompt~~ → 否：AGENTS.md 进入现有环境规则管线，每轮刷新但不写对话。

### 10.1 跨文档修订登记

| 相邻文档 | 本方案实施后需同步的语义 | 说人话 |
| --- | --- | --- |
| [agent-server-and-ui-gateway](agent-server-and-ui-gateway.md) | 新增共享 IPC/ACP 模式；每连接复用 writer 背压、不因慢读断开；旧 stdio 仍保留，不把尚未完成方案写成现状 | 区分旧入口和新入口，复用原有降级规则。 |
| [interrupt-and-cancellation](interrupt-and-cancellation.md) | 控制连接取消、drain 确认、后台工具收口及超时 | 谁负责停，何时算停完。 |
| [transcript-stable-id-and-stream-reconciliation](transcript-stable-id-and-stream-reconciliation.md) | 当前消息前缀与结束事件补发、原历史合并的切换/重连回归 | ID 复用不变，不引入整轮快照。 |
| [skill-system](skill-system.md)、[mcp-client](mcp-client.md) | 连接技能 overlay、new/load 重扫、会话 MCP 与项目缓存例外 | 共享哪些，隔离哪些。 |
| [session-storage](session-storage.md)、[work-dir-and-data-layout](work-dir-and-data-layout.md) | 可选 header origin 对象；access 仅在回复/窗口内存；固定 serve cwd、AGENTS.md 发现范围 | 不新增第二份聊天或权限存储。 |

本文与已有目录索引只记录设计，相邻基线文档不提前改成“已实现”。宿主登记表测试允许追加 Tomcat 行，旧业务测试预期不应被改弱；这比字面“所有既有测试文件零修改”更准确。

### 10.2 后续规划：本期不实现、不进验收

```text
客户端 A -- WebSocket 连接 1 --+
客户端 B -- WebSocket 连接 2 --+--> 同一个 serve 内的 WebSocket 入口
客户端 C -- WebSocket 连接 3 --+          -> 复用连接层/运行时/当前消息补发
```

1. **多 WebSocket 连接：**一个服务维护多条连接，不另起连接器服务，不每连接一个进程。WebSocket 是传输方式，不自动等于 ACP；支持浏览器/远程时再定消息协议、Origin 与鉴权，不预建网络框架或新 Web UI。
2. **断线续跑并接回原轮：**只有实际需要且接受维护宿主改动时再立项。届时须一起解决任务身份、重复提交、活动轮重订阅、宿主取消原因、worktree/凭据保留和结果未知；不能只把 EOF 取消删掉就宣称完成。

**一句话总结：本期让三个入口共用同一个执行服务，平台照常调度、控制者离开就停手、旁观者随时来看；不要把后续的网络接入和断线续跑提前做进来。**
