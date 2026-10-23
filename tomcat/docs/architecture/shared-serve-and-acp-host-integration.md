# 共享 serve 与 ACP 宿主接入：Paperclip、Multica 和 VS Code

> **状态：实现已落地，macOS 按约定范围验收完成；Windows 实机待验。** 完成项与验证证据以运行时计划 `plan_tomcat_paperclip_multica_agent_3e12d03d` 为准；不是旧版“断线后后台续跑”方案。
> **本期结论：**一个长期运行的 serve；两个宿主只新增标准 Agent 适配器；控制连接断开即取消其任务，旁观连接断开不影响任务。
> **本轮补充：**§3.2.3 增补会话 MCP 与连接技能目录的单张 ASCII 总图，说明它们如何复用原机制及各自的生命周期；§3.3 保留 cwd 从客户端到模型提示词、再到工具执行的链路；§3.4 用 Paperclip 的真实用法和 ASCII 图解释环境变量从宿主到 acp、serve、LLM 与工具的全过程。§4.1 按当前工作树校正本机握手字段和退出通知；其余实施进度仍以运行时计划为准。
> **范围：**同机、同用户、同 Tomcat 数据目录和 agent。服务可执行不同项目/worktree，但每个 VS Code 窗口仍只列出当前项目的会话；只有打开宿主所在项目才能旁观。WebSocket、断线续跑并接回原轮均为后续规划。
> **规范：**遵循 [ARCHITECTURE_SPEC](../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md)，正文 §1–§10 与其对应。返回[架构文档地图](README.md)。
>
> 本文把计划落到职责、接口、状态和验收。标记 **【新增】** 表示相对改造前新增，不等于尚未实施；设计阶段的测试名与实际入口区别见 §8。计划负责完成状态与验证证据，本文负责技术契约，不能用源码存在代替验收通过。

## 文首导读：方案导图集

### 阅读顺序建议

1. 看 A.1：谁执行、谁控制、谁只看。
2. 看 A.2 和 B：消息怎么进来，连接断开后谁负责停工具。
3. 看 §3：关键取舍和实施顺序；MCP/Skill 与原机制的关系看 §3.2.3 总图，cwd 如何告诉模型看 §3.3，环境变量如何传递、模型如何使用看 §3.4；准确握手字段看 §4.1。
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
| 就绪连接 | 读得慢、输出积压 | 仍就绪 | 本连接 writer 合并增量；原生满时丢消息增量并提示，ACP 保留完整增量；均不触发取消 | 界面卡一下，不等于人走了。 |
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
| 背压 | 客户端读取速度赶不上服务输出 | 每连接独立 writer；内部仍按 sessionId 分缓冲 | 原生沿现有合并/丢增量/提示策略；ACP 只合并、不丢正文；不因慢读断开，区别于真实 EOF 或写失败 | 原生直播可降级，宿主结果不能缺字。 |
| 控制者 | 有权给某会话派活的连接 | slot 的控制绑定，注册表统一维护，只在服务内存中 | 每会话最多一个；断线清空绑定并取消，原任务停完才能交接 | 当前点菜的人。 |
| 旁观者 | 只能读消息和状态的连接 | 连接的当前显示 sessionId + slot 控制绑定 | 打开已加载会话不重建执行上下文、不接手控制；只改本连接显示，不参与写命令 | 只看不动。 |
| 会话 | 可跨多次 prompt 保留的对话 | `SessionManager`、`SessionSlot`、原 transcript | load 是读历史继续，不是接回旧 RPC | 一份对话记录。 |
| 进程 cwd | 操作系统记录的进程当前工作目录 | `std::env::current_dir()`；由启动者设置 | 共享 serve 固定在自己的数据目录，不随请求全局切换 | 服务自己落脚的地方。 |
| 会话 cwd | 本次对话的项目工作目录 | 既有 `SessionEntry.cwd`、JSONL 头 `cwd`；运行时 `SessionSlot.cwd` 与 `ChatContext.scope_services.agent_workspace_dir` | 共享模式由客户端明确提供、校验并绑定；据此生成模型上下文，不自动授权 | 这份任务在哪干活。 |
| 工具 cwd / 路径 | 一次工具调用实际使用的目录或文件地址 | 模型给出的工具参数，经 Tomcat 执行器解析与权限检查 | 显式路径仍受权限门控制；共享模式的默认/相对目录须绑定当前会话，见 §3.3 的现有接缝 | 说了地址，还得把工具真正带到那里。 |
| 整轮任务 | 一个 prompt 驱动的最外层执行 | `run_task`、cancel token、完成结果 | 可包含多条 assistant 消息和多次工具调用；不能以首个 agent_end 猜成功 | 一次完整交办。 |
| 当前消息补发记录 | 当前一条助手消息的累计正文、思考及是否结束 | `event_pump.rs` 转发处的会话内记录【新增】 | 下一条 message_start 才替换，slot 卸载时释放；不以 message_end 当落盘确认 | 只补正在说的这句话，不重做整场会议纪要。 |
| 会话来源 `origin` | 这个会话是否经 ACP 创建及创建方名称 | 【新增】transcript 第一行的可选对象 `{kind:"acp", clientName:…}` | 只在 ACP new 时写一次；原生/老会话缺省；名称不参与权限判断 | “谁开的会话”，不是“谁现在在线”。 |
| 本窗口访问方式 `access` | 本窗口此刻是 control 还是 observe | 【新增】switch_session 回复及扩展会话视图内存 | 服务按来源和控制者判断；不落盘，与消息 role 无关；重新打开才重新判定 | 这扇窗口现在能不能下指令。 |
| 连接环境快照 | 客户端交给会话子进程的环境；另只读 `TOMCAT_HOST_LOCALE` 和 `TOMCAT__UI__LANGUAGE` 解析本连接的界面语言 | 内存 `map<string,string>` | 不全局 `set_var`，不写聊天、日志或会话文件；缺省键不借服务进程的同名变量 | 每个任务带自己的工具箱，窗口带自己的语言。 |

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
- **提示词里有 cwd，不等于工具已经按会话解析相对路径。** 当前 `handle_bash` 直接取模型参数，`ResolvedCwd` 在省略时继承进程目录；文件原语的路径规范化也未接收会话基准。共享后只改会话字段或提示词仍可能用错目录。完整链路、源码证据与本期验收要求见 §3.3；本轮不修改产品代码。

## 3. 落地选型与实施

这里的“已定稿”指依据当前计划确定的设计方向，不表示代码已交付。涉及新本机接口的字段是本文的拟定契约，M1 须用真实 SDK/客户端验证后固化到 Rust 类型及生成产物。

### 3.1 落地选型决策表

| 维度 | 关切 | 决策 | 取自 | 入选理由 | 未入选 + 拒因 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| 执行归属 | 三个入口能否看到同一执行 | 采用共享 serve，复用完整 SessionSlot 生命周期。 | Tomcat `tomcat/src/api/serve/mod.rs`；Codex `codex/codex-rs/app-server/src/thread_state.rs` | 设计：连接层之下共用注册表；理由：不重拼 AgentLoop 周边流程 | OpenCode `opencode/packages/opencode/src/cli/cmd/acp.ts` 的每入口自带 server：旁观需额外发现 | 大家看同一锅，不各煮一锅。 |
| 寿命 | 不改宿主重试和收尾 | 控制连接断开取消，旁观断开只退订。 | Tomcat `tomcat/src/api/serve/mod.rs::cancel_and_wait`；Multica `multica/server/pkg/agent/launch.go` | 设计：由服务停任务；理由：保持宿主对 Agent 进程的生命周期假设 | Codex `codex/codex-rs/app-server/tests/suite/v2/thread_unsubscribe.rs` 的持续运行只用于旁观，不用于控制者 | 宿主以为已经停手时，不能还在偷偷改文件。 |
| 本机接入 | 并发启动、鉴权与发现 | 采用固定 IPC 路径、OS 权限及启动锁。 | Tomcat `tomcat/src/infra/platform/fs.rs`；VS Code `vscode/src/vs/base/parts/ipc/node/ipc.net.ts` | 设计：Unix socket/命名管道；理由：本机不需要端口密码登记 | OpenCode `opencode/packages/cli/src/services/daemon.ts` 的 TCP/password：增加当前不需要的状态 | 不为本机接线搭额外的网络服务。 |
| 协议 | 标准 Agent 与私有 UI 共存 | ACP v1 与原生 NDJSON 在 serve 内汇入同一运行时。 | Tomcat `tomcat/src/api/serve/types.rs`；Paperclip `paperclip/packages/adapters/gemini-local/src/server/acp.ts` | 设计：SDK 映射、relay 转发；理由：宿主不认识 Tomcat 内部协议 | 旧计划私有 ACP attach/requestId 扩展：本期不续跑，无消费者 | 平台说 ACP，界面说原生话，都交给同一个执行器。 |
| 环境 | 首个启动者污染其它会话 | 会话子进程用连接快照；模型 key 用保存凭据，出网代理按控制方环境及保存配置。 | Tomcat `tomcat/src/core/tools/primitive/bash_task.rs`、`tomcat/src/core/llm/auth.rs`；Paperclip `paperclip/packages/adapter-utils/src/acpx-engine/execute.ts` | 设计：逐会话注入；理由：保住宿主原有任务环境；项目 MCP 共享例外见 §6 | VS Code 私有 serve 继承整份父环境的旧路径（`tomcat-vscode-ext/src/serveClient/TomcatMessenger.ts`）：不能直接扩成共享默认 | 谁先开服务，不应决定别人的模型 key。 |
| 实时旁观 | 中途加入不能漏前缀 | 复用普通打开流程，在开始转发时只补发当前消息。 | Tomcat `tomcat/src/api/serve/{commands,event_pump}.rs`、`tomcat-vscode-ext/src/ui/webview/{provider,state}.ts`；Codex `codex/codex-rs/app-server/src/request_processors/thread_lifecycle.rs` | 设计：同一转发边界补正文/思考，历史按稳定 ID 合并；理由：已完成消息已有持久历史 | Codex 的整轮快照：本期不照搬；Continue `continue/extensions/cli/src/commands/serve.helpers.ts` 的历史轮询：拿不到半截正文 | 打开还是老流程，只补没听到的半句话。 |
| 控制/旁观 | 谁决定窗口能否写会话 | 服务在打开时返回 access；界面置灰、服务按白名单准入。 | Tomcat `tomcat/src/api/serve/{commands,types}.rs`、`tomcat-vscode-ext/src/serveClient/sessionRouter.ts`；计划第 5 节 | 设计：来源入会话头，控制者在服务内存，access 只作窗口显示结果；理由：三者寿命不同 | 扩展自己猜身份或另加 Host 意图拦截：重复授权逻辑，且可能阻断打开动作 | 界面负责提示，服务负责守门。 |
| 慢连接 | 界面卡顿不能取消任务或拖慢别人 | 每连接复用 writer；原生满时丢消息增量并提示，ACP 只合并、不丢正文；均不按慢读断开。 | Tomcat `tomcat/src/api/serve/writer.rs::spawn_writer`、`enqueue_frame`；`tomcat/src/api/serve/tests/writer_test.rs` | 设计：stdout 出口改为各连接 socket，背压策略复用；理由：慢读不是控制者离开 | 队列溢出即断开：会连带取消该连接控制的任务；另造重放/重连协议：本期不需要 | 卡的是画面，不该把活停了。 |
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
- checkpoint 落盘复用 CLI 原有的 30 秒预算，不与前台取消的 3 秒混用；relay 总等待上限 60 秒，Multica backend 为 65 秒、WaitDelay 为 66 秒。超时仍失败并保留未完成句柄，不把等待延长当作停止成功。
- 连接断开即清空它的控制绑定并取消任务；**无控制者不等于已经停下**。新控制者仍须等原任务清理完成，停止失败返回 busy/stop_timeout，不 abort 掉句柄后假装空闲。原生旁观窗口不会自动接手，只能重新打开后由服务重新判断；ACP 来源在 VS Code 始终只读。
- 同一个连接可控制多个会话，另外当前显示一个会话（可为旁观）；关闭连接取消其全部控制会话，不按“目前显示哪一个”缩小取消范围。

**强杀例外：**宿主强杀 relay 或服务通信故障时，relay 无法在退出前取得确认；服务发现断开后再取消。宿主清理 worktree 与取消之间仍可能有窗口。本期不改宿主收尾状态机，不能承诺强杀/超时后目录绝对零写入；M1 必须测量并暴露失败，见 §9。

满槽位时，注册表串行决定淘汰最久未用的合格 slot：无运行任务、command job、排队输入、待确认、后台工作；不由 ACP 连接持有，也不是任何 VS Code 连接当前显示的会话，**当前显示为旁观也受保护**。扫描各连接的当前显示 sessionId 即可，不加旁观引用计数。选择与卸载期间禁止并发新准入，锁内不执行耗时 I/O；卸载复用现有清理并释放控制绑定，不加空闲定时器。

被卸载的会话从服务 Live 清单消失，但仍留在自己项目的磁盘列表中；宿主会话也可在同项目窗口通过原有打开路径重新载入，只读身份不变。不扫描或打开其它项目的历史。两个槽位都被运行中或当前显示会话占住时返回 `too_many_sessions`，不能把正在看的旁观页踢回历史来腾位。

**连接输出与慢读（背压）。** 一个服务有多条连接后，不能共用同一个输出队列；也不能因某个窗口暂时读不动就把它断开，否则会误取消它控制的任务。

```text
会话事件 -> 按连接筛选 -> 原生 writer -> socket（读得慢）
                      |    合并增量 -> 堆满时丢消息增量 + 提示
                      |    重新载入历史可补全；连接与任务保持
                      +-> ACP writer -> relay stdout -> 宿主
                           合并增量，不丢正文、不插提示；连接与任务保持

真正 EOF / 写失败 / 明确离开 -> 原有断连收尾；不是慢读策略
```

- **复用，不重写背压。** 原 `spawn_writer` 已接收任意 AsyncWrite（`tomcat/src/api/serve/writer.rs:56-65`）；共享模式在各连接建立时各建 writer，原生出口从唯一 stdout 改为各自 socket。每个 writer 仍按会话排队，写 socket 不阻塞其它连接的事件入队；ACP 出口仍经 SDK 输出标准帧，复用背压分类时不能把原生帧直接混进 ACP 流，T28 覆盖两种入口。
- **慢读不算断连。** 原生连接沿现有 `enqueue_frame` 中先 `try_coalesce_tail`、再判阈值的顺序：能合并则合并；仍达到阈值时丢该连接的消息增量，并排入 `llm_notice(finishReason="backpressure")`，同段积压不逐帧刷屏。控制/响应/生命周期帧不按消息增量丢弃；Bash 预览沿原合并/截断规则。恢复读取后在原连接收到提示、保留的帧和后续事件，不强制重连、不取消任务。
- **ACP 保留完整结果。** 宿主把增量拼成最终回复，不能依靠 VS Code 的历史重载补字。`tomcat/src/api/acp/mod.rs::run_connection` 仅覆盖 `max_buffered_frames = usize::MAX`，复用原 writer 和合并窗口，不新增模式；正文不再插入 transport notice。同一工具调用的 shell 预览仍原位替换。`acp_stdio_e2e.rs::slow_host_still_receives_the_complete_reply` 用约 1MB 输出和真实 stdout 停读验证全文一致。
- **不把丢帧说成无损。** `serve.max_buffered_frames` 是现有丢增量阈值，不是全连接帧数或字节数的硬上限；关键帧仍可积压。本文不承诺客户端无限停读时内存有界，也不新增慢读超时、踢连接或重放机制。实际写出失败仍须由连接层按断连清理，不能只记录日志后留下失效连接。

**测试锚点：**T28 在真实共享服务上暂停一条控制连接的读取，确认任务继续、另一条连接照常收流，再恢复读取。已有 `serve_writer_coalesces_deltas_under_pressure`、`serve_writer_never_drops_lifecycle`、`serve_writer_backpressure_notice_emitted_once`（`tomcat/src/api/serve/tests/writer_test.rs:101-183`）只证明 writer 的局部行为，不能替代这个多连接旅程。

#### 3.2.3 环境、MCP 和技能的归属

共享进程不能把启动者的环境当作所有会话的默认值。连接传环境快照，会话控制权交接完成后才绑定新快照；子 agent 和 reviewer/verifier 必须继承所属任务的执行上下文。不熟悉“快照”和“环境注入”的读者先看 §3.4 的完整例子，再回到本节看生命周期与共享边界。

```text
连接环境快照 -> 会话执行上下文 -> Bash registry -> bash 子进程
                           +--> 会话级 MCP 管理器 -> MCP 子进程
                           +--> 连接技能 overlay -> prompt / load_skill / 子 agent

Tomcat 保存配置 -> 模型凭据（不读首个宿主的 key）
出网 HTTP 代理  -> llm.proxy → 系统代理 → 直连（MCP 例外见 §6.3）
项目级 MCP 缓存 -> 同 cwd 共享，首个创建者的环境是已知例外
```

MCP 仍沿现有白名单从快照继承 PATH/HOME 等启动变量，再叠加 server 显式 env；**不是把整份快照里的所有 token 都交给每个 MCP**。stdio MCP 缺省 cwd 必须落到会话/项目根，不能继承服务自己的 `serve/` 目录。ACP 传入的 MCP 独立于 cwd 缓存，结束后回收，项目信任门不变。

项目 MCP 保持既有按目录共享，使用首个创建它的会话快照；同 cwd 不同控制者无法保证不同项目 MCP 环境。连接私有 MCP 与 Bash 必须隔离，不能把这个例外扩散到它们。

**一图看懂：会话 MCP 与连接技能目录如何复用原有机制。** 下图按当前工作树说明同一项目内的会话 A/B；“新增”指相对原机制增加的部分，不代表另造执行系统。一个控制连接可控制多个会话，技能目录来源相同，但各会话保存自己的有效技能表；“私有”描述配置与发现范围，不是文件访问沙箱。

```text
====================== 同一个共享 tomcat serve ======================

  核心关系：复用原来的执行代码，只把“共享状态”和“会话私有状态”分开。


  [1] MCP：同一种管理器，两份独立实例
  ------------------------------------------------------------------

  原有：全局 / 项目 mcp.json          新增：ACP session/new、load
             |                                mcpServers
             |                                     |
             v                                     v
      +------------------+                +--------------------+
      | 共享 MCP 管理器  |                | A 私有 MCP 管理器  |
      | 同项目会话共用   |                | 只属于会话 A       |
      +--------+---------+                +---------+----------+
               |                                    |
               |       共用同一套 McpManager 代码    |
               |       各自维护连接、凭据、运行状态  |
               |                                    |
               +----------------+-------------------+
                                |
                                v
                       +------------------+
                       | McpToolView      |  <-- 新增
                       | 合成本会话工具表 |
                       +--------+---------+
                                |
                                v
                  原有 tool_search / describe / call
                                |
                                v
                     路由到对应的 MCP 管理器

  规则：不写项目配置；共享与私有 MCP 同名时报错。
  会话 B：共享同一个项目管理器，但不会拿到 A 的私有管理器。


  [2] Skill：同一个扫描器，生成“基础表”和“会话有效表”
  ------------------------------------------------------------------

  原有：项目 / agent / managed 技能目录
             |
             +----> 原有扫描器 ----> 共享基础技能表
             |
             |                       新增：tomcat acp --skills-dir
             |                                  |
             |                           连接保存目录路径
             |                                  |
             |                          new/load 时绑定到 A
             |                                  |
             +----------------+-----------------+
                              |
                              v
                  原有扫描器再次扫描这组目录
                              |
                              v
                    +----------------------+
                    | A 的有效技能表       |  <-- 新增
                    | 基础技能 + 连接技能  |
                    +---------+------------+
                              |
                    +---------+---------+
                    |                   |
                    v                   v
              原有提示词技能清单    原有 load_skill

  规则：只读取，不安装、不复制技能文件；目录不是 ACP 标准字段。
  同名优先级：连接目录 > 项目 > agent > managed，并记录覆盖诊断。
  未提供连接目录：A 的有效技能表直接使用共享基础表的副本。
  会话 B：有自己的有效表，不会自动发现 A 的连接目录。


  [3] 生命周期：新增的是绑定与清理，不是另一套执行系统
  ------------------------------------------------------------------

  新建 / 加载 / 重新取得控制权
          |
          +-- MCP   -> 校验 -> 停旧私有连接 -> 换新配置 -> 启动
          |
          +-- Skill -> 清旧私有视图 -> 绑定当前连接目录 -> 重扫

  控制连接断开
          |
          +-- MCP   -> 停止私有连接，不因此关闭项目共享 MCP
          |
          +-- Skill -> 不删除目录里的文件；下次接手时重绑视图

====================================================================
  真正增加的东西：
    MCP   = 私有管理器实例 + 工具合并入口 + 生命周期绑定
    Skill = 连接目录路径   + 会话有效技能表 + 绑定与刷新
====================================================================
```

图中实现对应：`tomcat/src/core/connector/mcp/session.rs`（`McpToolView`）与 `tomcat/src/api/chat/session_mcp.rs` 管理私有 MCP；`tomcat/src/core/skill/discovery.rs::discover_with_connection` 与 `tomcat/src/api/chat/resource_inventory.rs::sync_resource_inventory` 复用技能扫描；`tomcat/src/api/serve/ownership.rs::open_session` 负责控制接手时的替换和绑定。生命周期与失败边界继续以下文为准，图不表示已通过全部验收。

**会话 MCP 的生命周期。** `session/new/load.mcpServers` 只保存在该会话的内存管理器，不写全局/项目 MCP 配置或会话头。工具发现与调用使用“既有共享管理器 + 当前会话私有管理器”，不把私有配置塞进目录缓存。

```text
项目文件里的 MCP ── 原项目信任检查 ── 按目录共享
宿主显式传入 MCP ── 校验配置与重名 ── 当前会话私有
                                          |
                     取消/断开/卸载 ── 停连接并等进程退出
                     load 替换      ── 先停旧的，再启新的
                     load([])       ── 清除私有 MCP
```

- 宿主明确交来的配置不是从项目文件发现的配置；它不替项目获得信任，也不改变工具调用的确认规则。`McpManager::project_trusted_for` 分开这两种来源。
- 同一连接再次 load 也替换 MCP；忙碌中的会话不接受替换。另一控制者仍在时，原 `session_in_use` 检查先拒绝，不能先启动请求者的 MCP。
- 成功完成一轮不关闭会话；取消会关闭私有连接，下一轮可用同一控制者的配置重建。标准 stdio 与 Streamable HTTP 复用现有传输，未支持的 SSE 明确拒绝。停止远端连接不等于杀掉远端服务器。
- 私有来源与已有来源同名时拒绝配置；共享配置后来添了同名来源时，发现与调用均报冲突或隐藏歧义项，不悄悄改用另一套凭据。旧进程的退出要用真实 PID 测试证明，不能只断言发出了 cancel。

**MCP 生命周期先例（只读调研，未运行外仓测试）：**

| 实现与证据 | 采用或不采用的部分 |
| --- | --- |
| Codex `../codex/codex-rs/codex-mcp/src/runtime.rs::McpRuntime`；`../codex/codex-rs/rmcp-client/tests/process_group_cleanup.rs::shutdown_kills_initialized_stdio_server_with_in_flight_operation` | 按线程持有配置/环境，显式 shutdown；借鉴“返回已停之前检查真实子进程”的测试。Tomcat 复用已有 retirement receipts，不复制其整套连接身份框架。 |
| OpenCode `../opencode/packages/opencode/src/acp/service.ts::registerMcpServers`、`../opencode/packages/opencode/src/mcp/index.ts` 的 `add`；`../opencode/packages/opencode/test/mcp/lifecycle.test.ts` 的 `local stdio timeout terminates the real server process` | 借鉴 ACP 复用原执行入口和真实 PID 用例；不采用仅按目录/name 放连接的做法，否则同目录会话会替换彼此。 |
| Continue `../continue/core/context/mcp/MCPManagerSingleton.ts::setConnections`、`../continue/core/context/mcp/MCPConnection.ts::disconnect` | 可参考配置替换与 transport close；`disconnect` 等待 close，但 `setConnections` 的移除分支只 `void connection.client.close()`，不能据此保证会话隔离或真实进程已退出。 |

选择独立会话管理器而非扩展共享缓存，是因为宿主已经会给同目录的不同任务传不同配置/凭据。只有未来明确要求跨会话共享宿主 MCP，并定义了凭据身份与共同寿命，才重新考虑共享；目前不加连接复用池。对应回归位于 `tomcat/src/core/connector/mcp/tests/session_test.rs`、`tomcat/src/api/acp/tests/setup_test.rs` 和 `tomcat/tests/acp_stdio_e2e.rs`，最终通过情况以运行记录为准。

技能在 new/load 时经现有 inventory 同步锁重扫，即使复用已加载 slot 也要检查。同名技能优先级为：连接私有目录 → 项目 → agent → managed；仍共用原来的解析、禁用名单与数量上限。共享 `ScopeContainer` 只存基础目录，每个共享会话持有自己的有效技能视图；提示词、`load_skill` 和审查依赖都引用这一份。接手时先清旧私有目录再同步；新控制连接没有传目录就不继承旧目录，也不重写已存的聊天历史。

`--skills-dir` 必须是绝对、可读取的目录，启动服务前及 new/load 时检查；这只声明技能来源，不授予文件工具读取权限。控制者仍须经过原权限门；覆盖保留诊断，不写共享 home。回归见 `tomcat/tests/support/acp_skills.rs`、`tomcat/tests/serve_shared_e2e.rs` 和 `tomcat/src/core/skill/tests/discovery_test.rs`。

服务级配置不读启动者的模型/日志覆盖，也不读旧的 `TOMCAT_REVIEWER_MAX_TURNS`、`PI_PLUGIN_DISABLE`、`PI_CHAT_SHOW_THINKING` 或搜索兜底期限覆盖；Tomcat 自身 HTTP 使用保存的代理配置或系统代理，不使用窗口/ACP 连接的代理变量，详见 §6.3。独立 CLI 保持旧行为。MCP OAuth 中显式配置的 `client_secret_env` 仍从该管理器持有的控制连接快照读取，缺失不回落到服务进程环境；这条凭据规则不是代理选择规则。

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

**转发范围：**每条连接只接收“它控制的会话”与“它当前显示且属于本窗口项目的会话”的并集。切到后台但仍由本窗控制的会话继续收；切走仅旁观的会话就停止收；不在连接建立时广播全部宿主会话。跨项目打开在改变显示之前即被拒绝，不另做一套旁观过滤器。只有从“不接收”变成“开始接收”时补发，已经在收的会话不能重复补前缀。

**补发的最小状态：**`event_pump.rs::MessageReplay` 累计当前 assistantMessageId 的正文、思考与结束标记，放在原 `SessionSlot` 内。`ServeRuntime::emit_event` 与连接开始接收共用这把短锁：先排补发，再排后续增量；磁盘/socket I/O、事件监听注册/注销不在锁内。一条原有 `message_update` 的 `message` 放完整正文与思考，`assistantMessageEvent` 为 null，明确表示替换而非追加；之后仍是普通 delta。不保存全会话事件日志。

下一条 `message_start` 替换这条记录；`message_end` 只标记结束，不清空。它先于最终持久化完成也可能到达（`tomcat/src/core/agent_loop/stream_handler.rs:542`），因此“刚说完、下一条还没开始”的连接仍先收到全文，再收到结束事件。任务异常收尾也标为结束；历史改写后经原 context 重载入口清除旧补发，slot 卸载时释放记录。不建立已完成消息、工具状态和等待项的整轮副本。

**扩展侧复用与已核实的缺口：**原 `WebviewStateStore::applyEvent` 只读增量，忽略 `message` 中的全文；按 ID 合并只能防重复行，不能防“旧前缀 + 全文”重字。因此只在原函数补全文替换，继续用既有 upsert patch；历史确认同一 assistant 已落盘后停止追加迟到增量，身份取已有 `historyEntries`，不加另一份完成表。复用 `rebuildHistoryTimeline`、`upsertTool` 和普通打开流程，不另造 renderer、偏移量/全局 seq 或独立合流协议。定向用例在 `tomcat/src/api/serve/tests/connection_test.rs`、`tomcat/src/api/serve/tests/writer_test.rs`、`tomcat-vscode-ext/src/ui/webview/tests/dual_channel.test.ts`；真实 IPC + ACP 旅程在 `tomcat/tests/acp_observer_e2e.rs`，用可释放的本地 SSE 暂停点而非定时猜测中途加入。

**慢读与补发的边界：**本节“不重不漏”针对正常接收时的中途加入、历史合并与重连，不是背压下的无损承诺。触发 §3.2.2 的丢增量后，该连接的实时正文/思考（包括以 message_update 补发的前缀）可能不完整；恢复读取会收到提示和后续事件，不因慢读重连或取消任务。已完成并落盘的内容仍可通过原 `get_messages` 读取；不新增逐帧补传，也不承诺提示一到就自动补齐尚未落盘的消息。

#### 3.2.5 VS Code：原界面加只读来源

扩展用同一个 Messenger 连接共享服务；supervisor 负责连接重试，不把 dispose 等同于 kill/shutdown。明确 stop 的客户端停用自动重启；其它连接看到停服不应无条件把服务拉起，用户主动连接才进入启动流程。

**共享连接的初始化顺序（已接线）：**

```text
清空本窗口上一代连接的临时画面
  -> CLI serve start（首次／用户主动连接）；自动重试只用 serve status
  -> 版本及实例校验 + 本机 socket 握手（只带协议与环境，不带项目字段）
  -> 原 initialize
  -> new_session({cwd: 窗口目录, resumeCurrent: true})
       已有 current：走原 switch_session，按服务端规则控制或旁观
       尚无 current：走原新建流程
  -> get_state / get_messages + 实时消息
```

`initialize.ts` 显式提供窗口目录；`commands.rs::resume_current_command` 复用原项目 key、`SessionManager::current_session_entry`、`SessionScope` 与打开流程。`resumeCurrent` 默认 false，普通“新建会话”及 ACP 的新建行为不变；它仅保留旧私有 serve 的“恢复当前／没有才新建”，不每次重连制造空会话，不新增连接项目字段、握手字段或第二套项目标识。若已有 current 来自 ACP，也不抢控制权。

`TomcatMessenger` 只持有 socket，dispose 半关闭并等连接收尾，不给共享 PID 发信号；超时如实说明服务收尾未确认。`ServeConnectionSupervisor` 的自动重试不启动服务，用户点重新连接才允许启动；原重启命令明确命名为“重启共享服务（停止所有任务）”，调用 CLI restart，不加二次确认。重连前清理临时画面，不能在初始化之后清理，否则会删掉初始化期间刚补发的消息。读取使用 UTF-8 流解码；本机握手结束时 readline 会暂停 socket，移交给原 NDJSON reader 后必须 resume。版本不兼容直接报错，不回落私有 stdio。

主输入框的 `/restart` 复用 `restartSharedService → tomcat.restartServe → tomcat serve restart`，只对无参数、无附件/引用的精确纯文本命令生效；先同步空草稿再断开，命令不进入模型或队列。它不放开输入禁用规则，无法发送时仍用命令面板。扩展管理命令的外层期限为 stop 80 秒、restart 150 秒，覆盖 CLI 的连接/握手、60 秒停止以及启动探测/锁/就绪阶段并留出余量；version/status/start 仍为 35 秒，Rust 清理期限与 supervisor 初始化计时不变。

定向证据入口：`tomcat/src/api/serve/tests/connection_test.rs` 的 `session_bootstrap_*`；扩展 `src/serveClient/tests/{sharedConnection,messenger_disposal,ServeConnectionSupervisor}.test.ts`；真实多窗口、重载、停服与工具取消放在 `tomcat-vscode-ext/tests/serve_disposal_integration.test.ts`。真实 GUI 与只读开关另归 T20/T27，不能用这些连接测试替代。

**项目边界沿用原设计。** `session_key_for_agent` / `project_root`（`tomcat/src/core/session/scope.rs`）仍按 git 根聚合：仓库根与子目录同项目，worktree 自成项目，无 git 时按目录。侧边栏继续 `sessionPool.ts::refresh` 的 `listSessions("disk")`，不合并 Live、不新增外部会话区。复用连接已有的 `SessionScope` 和 cwd 初始化流程；磁盘列表取本连接的 scope，旁观不改它。已加载会话的入口也核对原来的 key，跨项目回 `unknown_session`；直接按 ID 读取历史/状态同样不能绕过。既有聚合和存储不重写，不另造项目绑定协议。

```text
窗口目录 -> 原 session key -> 原项目磁盘列表
                                  |
                                  +-- 同项目宿主会话 -> 原打开流程 + 只读
                                  +-- 其它仓库/worktree -> 不列出、不打开、不收流
```

**身份判断只在服务端。** 原生客户端打开会话时按下面的顺序判断；ACP 客户端自己的 `session/load` 仍走单控制者准入，不套用 VS Code 的旁观规则。

```text
switch_session(X)
  |
  +-- 不属于本窗口项目？ ------------ 是 --> unknown_session
  |           否
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

**只读界面已接线。** `sessionRouter.ts::switchSession` 返回 `SessionOpenResult`，`sessionPool` / `provider` 把 access/origin 传入会话视图；`state.ts::applySessionState` 刷新时保留服务端 access，已有 owned 标记由它派生，不是第二份控制权。`get_state` / `get_messages` 的失败明确抛出，不把跨项目拒绝伪装成空状态。

现有 `switch_session` 的已加载分支仍会持久化 current 选择（`tomcat/src/api/serve/commands.rs:572-578`），共享模式必须区分本连接的显示与会话控制状态；“复用旧接口”不表示将这个单窗口副作用原样照搬给旁观者。

原项目磁盘列表中的宿主会话可标来源；它卸载后仍可在同项目窗口只读打开。要旁观任务 worktree，就打开该 worktree，而非主仓库。Live 仍只回答“服务加载了哪些会话”，不拿来填侧边栏，也不为本次旁观重定义这个接口。切走、关闭旁观视图只结束该连接对它的接收，不触发全局 slot 清理；关闭当前控制会话仍走原有停止/清理路径。

```text
本窗口打开：.../task-worktree
会话 [ 修复登录 · multica-agent-sdk · 运行中 v ]
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

**界面与旅程证据：**打开原会话下拉框时按需刷新同项目 disk 列表，不加轮询或 Live 合并。旁观时输入、发送/停止、模型/推理配置、回滚、文件恢复和删除不可用；切回自己的会话恢复可写。`gui/src/components/ModelPicker.tsx` 的旧测试兼容入口也遵循 disabled，已打开的配置菜单在只读时关闭。来源标注与只读提示沿原组件和 i18n。

真实旅程位于 `tomcat-vscode-ext/src/test/suite/support/sharedObservationScenario.ts`，由 `host.e2e.test.ts` 调用；使用真实 CLI、共享服务、ACP 子进程与 VS Code，只有外部模型使用本地脚本 HTTP 响应。在扩展目录执行 `TOMCAT_E2E_REAL_OBSERVATION=1 TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR=<项目制品目录>/shots/shared-observation npm run test:e2e:vscode-devhost`。覆盖生成中加入、真实 webview 重载、真实 shell 运行/完成、忙闲只读、其它项目不可见/不可读、回到自己的会话；同时保留 PNG、ARIA、console 与旅程断言 JSON。重载测试等新页面出现后重新连接 CDP，只重试读取探针，不重发点击或业务命令。此旅程不是两个完整宿主的验收，后者仍归 M3。

#### 3.2.6 两个宿主只加标准适配器

Paperclip 新增 `packages/adapters/tomcat-local/`，复用 acpx 引擎的环境、MCP、权限、取消和 session codec。包内补临时技能目录生命周期和 CLI 参数引用；不改共享执行/恢复算法，不走 native runner profile。命令覆盖、模型探测和菜单表单沿 Gemini/Kimi 结构。

Multica 新增 `server/pkg/agent/tomcat.go` 实现既有 `Backend`，复用 hermesClient 的 JSON-RPC，不复制 Hermes 的 `session/resume` 方言；Tomcat 使用标准 `session/load`。setup 成功后、prompt 前上报 session ID；沿既有异步 pin 流程，不新增持久化事务。Cancel 自管：关 stdin→等待 relay→超时杀进程组，不直接复用默认立即 SIGKILL，也不另发适配器私有 cancel 协议。

任务说明登记为 AGENTS.md，技能目录登记为 `.agents/skills`；不加入 inline system prompt 提供者。Paperclip 的 CLI 临时技能目录只能作为实现参考：若每轮随机目录进入 `agentCommand`，acpx fingerprint 会改变，可能触发新会话。该组合列为 M1/M3 兼容验证项，不把“保留平台默认恢复行为”夸成“每次都原 ID 续聊”，也不擅改公共指纹规则。

### 3.3 CWD 全链路：谁选目录、谁告诉 LLM、谁真正执行

#### 3.3.1 需求背景：地址要交给模型，也要交给工具

CWD 是 current working directory，即工作目录。例如 `/work/shop` 表示本次任务在这个项目里干活，不表示宿主知道整台电脑的目录结构。**LLM 不会自动读取 VS Code、进程 cwd 或会话文件；Tomcat 把会话目录写成系统提示词，随主 Agent 的模型请求发送。** 模型返回工具调用后，真正读文件、启动命令的仍是 Tomcat。

```text
客户端确定项目目录 -> Tomcat 会话记住 cwd
                              |
                 +------------+----------------+
                 |                             |
                 v                             v
       Tomcat 生成系统提示词             工具执行需要的会话基准
       “工作目录是 /work/shop”            （共享模式须贯穿到底）
                 |                             |
                 v                             |
          LLM 读到说明、决定下一步              |
                 |                             |
                 +-- 返回工具名和参数 ----------+
                                               v
                                  Tomcat 解析路径、检查权限
                                               |
                                               v
                                  在执行机器上操作文件/运行命令
                                               |
                                               +-> 结果回给 LLM，继续下一步
```

**说人话：**Tomcat 一边给模型写“工作地址”，一边负责把工具带到实际地址。模型看到一句目录说明，不会让操作系统自动切目录；模型服务即使在云端，也不是在云端直接打开你的本地文件。

#### 3.3.2 改造前的流程：VS Code 私有 stdio serve

下面以首次打开 `/work/shop`、需要建立初始会话为例，说明本次改造前的 VS Code 路径。独立 `serve --stdio` 仍保留这种目录兜底；改造后的扩展改走 §3.2.5 的显式初始化，不再拥有私有 serve。

```text
① 你用 VS Code 打开 /work/shop
   扩展取 session.defaultCwd；未配置则取第一个工作区文件夹
                 |
                 v
② 扩展启动自己的 tomcat serve --stdio
   spawn(..., cwd=/work/shop)，设置的是 serve 的进程目录
                 |
                 v
③ serve 建立初始会话，参数未单独传 cwd
   resolve_session_paths → 使用进程目录 /work/shop
                 |
                 v
④ 会话保存 cwd，并建立该会话的 ChatContext
   磁盘：sessions.json 条目 + <sessionId>.jsonl 首行
   内存：SessionSlot.cwd；scope_services.agent_workspace_dir
                 |
                 v
⑤ Tomcat 构建系统提示词（不是让 LLM 自己读会话文件）
   workspace_context → SystemPromptSnapshot → WorkspaceContextSection
   把 /work/shop 填进 workspace_context.txt 的占位符
                 |
                 v
⑥ Tomcat 发出主 Agent 模型请求
   系统提示词 + 对话/工具历史 + 当前用户消息等 → LLM
                 |
                 v
⑦ LLM 看到目录说明，生成工具调用，例如
   bash(command="pwd", cwd="/work/shop")
                 |
                 v
⑧ Tomcat 校验目录与权限，给 shell 子进程设置 cwd=/work/shop
   取得真实执行结果 → 回给 LLM → LLM 决定下一步或回答用户
```

当前模板中的相关片段，替换示例目录后如下（省略了其它行）：

```text
Agent workspace directory (agent_workspace_dir): /work/shop
- Interpret "current directory", "this project", and relative paths as this directory.
- This directory is NOT automatically authorized for file access. Use tools normally; if access is not yet authorized, the runtime will ask the user to grant `workspace_roots` or a session-only grant.
```

这就是 **“LLM 怎么知道目录”** 的直接答案：**Tomcat 的提示词组装代码告诉它，而不是 VS Code 或宿主直接调用模型告诉它。** 源模板是 `tomcat/src/core/prompts/templates/system/workspace_context.txt:1-5`；“项目目录”与“授权目录”是不同事实。

几个容易混淆的分支：

- **现在点击“新建会话”时已经传 cwd。** `SessionRouter.newSession` / `createDetachedSession` 把目录放进 `new_session.params.cwd`，因此不是所有会话都靠进程默认值。见 `tomcat-vscode-ext/src/serveClient/sessionRouter.ts:197-215`。
- **已有会话会使用保存的记录。** 创建时 cwd 同时进入索引与 JSONL 头；加载后由 `resolve_agent_workspace_dir` 取出。当前旧路径在目录失效时仍回落进程 cwd，这是共享模式要去掉的兜底，不是重建存储。见 `tomcat/src/api/chat/context.rs:340-355`。
- **不是每发一句话就再往聊天文件插一条“当前目录”。** 开始外层任务时检查并刷新提示词快照，主 Agent 每次模型请求再把系统提示词组装到请求中；这部分不作为普通用户消息反复写进对话历史。见 `tomcat/src/api/chat/run_loop/mod.rs:710-735`、`tomcat/src/core/agent_loop/reasoning_loop.rs:40-77`。

| 环节 | 当前源码证据 | 说人话 |
| --- | --- | --- |
| VS Code 选目录并启动进程 | `tomcat-vscode-ext/src/extension.ts:465-476`；`tomcat-vscode-ext/src/serveClient/TomcatMessenger.ts:186-198` | 编辑器把自己知道的目录交给子进程。 |
| 建立会话并保存 cwd | `tomcat/src/api/serve/mod.rs:151-164`、`tomcat/src/api/serve/mod.rs:288-365`；`tomcat/src/core/session/manager/session_impl.rs:843-885` | 接收显式目录或旧模式默认目录，记入会话。 |
| 从会话形成模型工作区上下文 | `tomcat/src/api/chat/context.rs:594-616`；`tomcat/src/api/chat/run_loop/mod.rs:141-178` | 使用该会话的目录，不让模型猜。 |
| 渲染系统提示词并交给模型循环 | `tomcat/src/core/llm/system_prompt.rs:168-197`、`tomcat/src/core/llm/system_prompt.rs:514-529`；`tomcat/src/api/chat/run_loop/mod.rs:990-998` | 将目录填进模板，交给 AgentLoop。 |
| 发请求、收回复 | `tomcat/src/core/agent_loop/reasoning_loop.rs:46-77`、`tomcat/src/core/agent_loop/reasoning_loop.rs:217-280` | 系统提示词随主 Agent 请求发送；LLM 返回文本或工具调用。 |
| 工具实际执行 | `tomcat/src/core/agent_loop/tool_exec/branches/bash.rs:10-51`；`tomcat/src/core/tools/primitive/bash_task.rs:767-771` | 使用工具参数，权限通过后设置子进程目录；不是改整个服务的目录。 |

#### 3.3.3 改造后：换目录来源，复用“告诉模型”的路径

共享 serve 自己固定在 `<agent 数据目录>/serve/`，不属于任何项目。创建/载入会话等需要确定项目范围的入口显式接收绝对 cwd；后续已经指定 sessionId 的操作使用绑定的会话上下文，不给标准 ACP 的每个 prompt 另加私有 cwd 字段。

```text
VS Code 扩展             Paperclip 执行端              Multica 本机 daemon
知道打开的项目           准备好的任务工作区            准备好的任务目录/worktree
  /work/shop               /tasks/job-1                  /tasks/job-2
     |                           |                            |
     | 原生会话请求              | 宿主侧适配器 + ACP          | 宿主侧适配器 + ACP
     |                           v                            v
     |                     tomcat acp 薄转发器          tomcat acp 薄转发器
     |                           |                            |
     +---------------------------+----------------------------+
                                 |
                                 v
                   同一个 serve：校验 cwd，绑定到相应会话
                   A=/work/shop  B=/tasks/job-1  C=/tasks/job-2
                                 |
                  +--------------+----------------+
                  |                               |
                  v                               v
          各会话自己的 ChatContext        各会话自己的工具执行基准
                  |                               |
                  v                               |
          原有提示词组装链不另造一套              |
          主 Agent 的 LLM 收到本会话目录          |
                  |                               |
                  +--- 工具调用 -> 权限检查 -> 实际执行

serve 自己的进程 cwd 始终不变；不以全局 chdir 切换会话目录。
```

**宿主不用猜你的文件系统。** 它只传自己已选择或准备好的任务目录；目录必须在实际执行机器上存在，且 Tomcat 能访问同一份文件。云服务器上的 `/work/shop` 不自动等于用户电脑上的 `/work/shop`；容器内外也不能仅靠同名路径互通。本期不新增文件同步、挂载或远程目录映射。

| 组件 / 所在仓库 | cwd 从哪里来、交给谁 | 本期职责与证据 | 说人话 |
| --- | --- | --- | --- |
| VS Code 扩展 / Tomcat 仓库 | 打开的工作区或扩展配置 → 原生会话请求 | 复用目录选择，接到共享连接；现有 `tomcat-vscode-ext/src/serveClient/sessionRouter.ts:197-203` 已显式传 cwd | 编辑器知道你打开了哪个项目。 |
| Paperclip 侧适配器 / Paperclip 仓库 | 现有 `context.paperclipWorkspace.cwd` → 共享 ACP 引擎的 `ensureSession({cwd,…})` | 新增 `tomcat-local` 及登记，不重写工作区准备；现有 `../paperclip/packages/adapter-utils/src/acpx-engine/execute.ts:1847-1862`、`../paperclip/packages/adapter-utils/src/acpx-engine/execute.ts:4567-4573` | 宿主准备好了任务地址，适配器负责递过去。 |
| Multica 侧适配器 / Multica 仓库 | daemon 的 `env.WorkDir` → `ExecOptions.Cwd` → ACP new/load | 新增 Tomcat backend 及登记；现有 `../multica/server/internal/daemon/daemon.go:8582-8598`、`../multica/server/pkg/agent/agent.go:25-30`；标准 ACP 先例 `../multica/server/pkg/agent/traecli.go:261-300` | 本机 daemon 知道自己准备的任务目录。 |
| 薄转发器与 ACP 接收端 / Tomcat 仓库 | 转发器原样转发协议；serve 内 ACP 实现读取 cwd → 会话 | 两者均须在 Tomcat 实现；不由转发器猜目录或调用模型 | 宿主侧适配器不是 Tomcat 的接收端，两边都要接好。 |

#### 3.3.4 实现边界与测试用例清单

**关键决策：复用会话 cwd 与系统提示词流程，补齐工具执行目录，不新增另一份目录状态或提示词协议。** 下列边界已接线；测试证据与尚缺的平台验收仍分别记录，不把实现完成等同于全平台验收通过。

- **会话目录不能猜。** 共享模式下缺失、无效或与加载作用域不符的目录明确报错，不回落服务数据目录；旧 chat/stdio 兼容路径保留。
- **告诉模型和实际执行必须一致。** `workspace_context.txt` 明确称其为“当前会话的工作目录”，不再称为启动 chat 时的目录；`.`、默认和相对工具路径均以会话为基准。绝对工具路径或显式其它 cwd 仍沿原权限门检查，不把 cwd 当成文件访问白名单。
- **工具落点在执行器落实，不靠模型猜。** `ResolvedCwd::from_raw_in_environment` 把省略/空值当作会话目录，展开相对路径与控制者 HOME 后再校验；独立调用未提供会话基准时保留旧的进程 cwd 兼容行为。文件原语经 `DefaultPrimitiveExecutor::resolve_path` / `path_in_session` 解析，再进入 `gate_check_path` 的权限门。源码：`tomcat/src/core/tools/primitive/bash_task.rs`、`executor/mod.rs`、`executor/gate.rs`；没有通过全局 `set_current_dir` 切换服务目录。

**测试用例（并入 §8，不另立进度表）：**

1. **模型真的收到正确目录。** `tomcat/src/api/serve/tests/session_cwd_test.rs::session_cwd_reaches_model_and_tools`（T12）从两个会话的真实模型请求中检查目录及其会话语义；只替换模型响应，保留真实上下文、权限门与文件工具。`tomcat/src/core/llm/tests/system_prompt_test.rs::workspace_context_section_describes_three_directories` 锁定说明文字，不只检查 GUI 或 `SessionSlot.cwd`。
2. **工具真的在正确目录执行。** 同一 T12 旅程让两个会话分别读取同名但内容不同的相对文件，并运行显式 cwd、`.`、省略 cwd 的 Bash；核对真实文件与子进程结果，服务进程目录始终不变，未授权目录仍被权限门阻止或要求确认。
3. **原入口与恢复不退化。** T12/T26 覆盖 VS Code 显式新建、初始/已保存会话、共享模式非法目录拒绝和旧 stdio 兼容。T21/T23 分别验证宿主已有任务目录经适配器到 ACP new/load，不根据转发器或服务的启动目录猜测。

上述测试入口已实现；最终结果以运行时计划与验收记录中的命令和任务日志为准。提示词断言证明模型收到什么，文件与 `pwd` 断言证明工具在哪里执行，两者不能相互替代。

### 3.4 环境变量全链路：谁准备 K-V、谁传递、LLM 怎么用

本节把 §3.2.3 的环境归属拆成可读的执行链路。Paperclip 示例来自其现有 ACP 引擎，Tomcat 接收链路按当前工作树说明；这不是两个宿主适配器都已完成验收的声明。除源码片段外，图中的任务编号、凭据和路径均为虚构示例，不代表真实账号。

```text
                     同一个 Tomcat 后台服务
                     |
                     +-- 服务设置：Tomcat 保存的模型凭据、代理配置
                     |
Paperclip 控制连接 --+-- 会话 A：Paperclip 的 bash 环境、会话 MCP、私有技能
                     |
VS Code 控制连接 ---+-- 会话 B：VS Code 的 bash 环境、自己的工具和技能

共享执行服务 != 共用所有任务的环境
MCP 只继承白名单变量；项目 MCP 按目录共享的例外见 §3.2.3。
```

**说人话：**大家请同一个后台干活，但各自带自己的工具配置。Tomcat 调用大模型所用的账号，仍由 Tomcat 自己管理，不由最先连接的宿主决定。

#### 3.4.1 需求背景：环境变量是 K-V，但不是整台电脑的一份公共表

环境变量是一组名称和值，例如 `PAPERCLIP_TASK_ID=task-123`。宿主可以在启动子进程时给它设置这组变量，**不需要修改操作系统的全局设置，也不需要先修改宿主自己的全局进程环境**。

```text
Paperclip 准备一组 K-V
  PAPERCLIP_API_KEY = 本次任务的凭据
  PAPERCLIP_TASK_ID = task-123
  PAPERCLIP_RUN_ID  = run-456
          |
          | 启动子进程时，通过 env 参数交给它
          v
     tomcat acp 的进程环境
          |
          | 复制成一份 K-V 数据，称为“环境快照”
          v
     后续通过连接发给 serve
```

`PAPERCLIP_API_KEY` 是 K（变量名），实际凭据字符串是 V（变量值）。**这个凭据用于访问 Paperclip，不是 Tomcat 调用大模型的 API key。** `PATH` 也是环境变量，含义是运行 `node`、`python` 等命令时去哪些目录找程序。

**说人话：**宿主给这次启动的 Agent 发一张“任务配置单”，不是把整台电脑的配置改成这个任务的配置。快照是在连接建立前复制的当时值，不是实时同步；宿主后来修改变量，不会自动改写已有连接或已经启动的工具进程。

#### 3.4.2 为什么要传快照：真正执行命令的不是转发器

Paperclip 的现有引擎会把任务编号、执行编号和访问凭据交给 Agent，并提供用 bash/curl 给当前任务留言的示例。对应代码是 `../paperclip/packages/adapter-utils/src/acpx-engine/execute.ts:2004-2074` 的环境赋值、`../paperclip/packages/adapter-utils/src/acpx-engine/execute.ts:3059-3077` 的 API 使用说明，以及 `../paperclip/packages/adapter-utils/src/acpx-engine/execute.ts:4573` 的 `sessionOptions.env`。这是宿主的现有工作方式，不是 Tomcat 为演示虚构的需求。

```text
原本：宿主 -> Agent 子进程 -> bash
               带着任务环境   可以继承同一份环境

共享：宿主 -> tomcat acp ----本机连接----> 早已运行的 tomcat serve -> bash
               有任务环境                  不会自动得到这份环境
               只转发，不执行命令          真正启动工具进程
```

如果服务早已由 VS Code 启动，它可能根本没有 Paperclip 的任务变量；如果误用第一次任务的环境，后续任务可能拿到旧任务编号或旧凭据。仅把 prompt 转发过去，不能修复这件事。

同样的问题也会影响普通开发命令。下面是帮助理解的假设，不是对本机安装版本的断言：

```text
服务启动者的 PATH -> Node 20
VS Code 的 PATH  -> Node 22

VS Code 会话执行 node
  错误：继承服务环境 -----------------> Node 20
  正确：使用 VS Code 连接交来的快照 --> Node 22
```

**说人话：**你委托后台干活，后台要带着你交给它的工具配置干，而不是拿第一个委托人的配置干。“保住宿主传来的任务环境”是兼容要求；“通过本机握手传快照”是 Tomcat 共享服务的实现办法，不是要求宿主增加一种 ACP 消息。

#### 3.4.3 解决方案：acp 把环境当作数据发给 serve

`tomcat acp` 在加载 Tomcat 自己的 dotenv/凭据之前捕获进程环境；然后连上本机服务，把 K-V 表放进 `ConnectionHello.environment`。服务按连接保存在内存中，获得会话控制权后，才把它绑定到相应的执行上下文。

```text
Paperclip
    | ① 启动 tomcat acp，并给子进程设置任务环境
    v
tomcat acp
    | ② capture：读取自己的环境变量，复制成 K-V 表
    | ③ 连上 serve，先发送一行包含 environment 的 JSON 握手
    |    Unix socket / Windows 命名管道；不是发给 LLM
    v
tomcat serve
    | ④ 校验握手，把快照保存在对应连接的内存中
    | ⑤ new/load 取得控制权，把快照交给该会话的工具执行上下文
    | ⑥ 启动 bash 时清除默认继承，再应用快照和必要内部标志
    v
bash 子进程
    可以读取当前任务的 PAPERCLIP_API_KEY、PAPERCLIP_TASK_ID 等变量
```

**说人话：**acp 把环境变量变成普通消息传过去；serve 到启动工具时，再把这份消息里的数据变成那个子进程的环境。服务不全局 `set_var`，不靠每执行一个会话就切换整个服务的环境。

| 环节 | 当前源码证据 | 说人话 |
| --- | --- | --- |
| 捕获宿主环境 | `tomcat/src/api/cli/mod.rs:567-570`；`tomcat/src/infra/platform/environment.rs:23-38`（`capture`） | 先收下宿主给的原始配置，避免混入 Tomcat 自己加载的凭据。 |
| 本机握手发送 | `tomcat/src/api/acp/forwarder.rs:14-25`；`tomcat/src/api/serve/service.rs:246-295`（`connect_acp`、`open_management`） | 用连接消息传 K-V，不靠跨进程自动继承。 |
| 服务接收并保存 | `tomcat/src/api/serve/service.rs:453-507`；`tomcat/src/api/serve/connection.rs:71-100`（`connect_with_source`、`child_environment`） | 每条连接保管自己的那一份。 |
| bash 使用环境 | `tomcat/src/core/tools/primitive/bash_task.rs:809-815`；`tomcat/src/infra/platform/environment.rs:74-76`（`apply`） | 设置新子进程的环境，不修改整个 serve。 |

完整握手字段、JSON 样例和返回值统一见 §4.1，避免这里再维护一份不同的协议。一个控制连接可以控制多个会话，它们使用同一份连接快照；旁观者只看，不用自己的环境覆盖被旁观会话。新控制者接手须等旧任务停完，再绑定新快照，见 §3.2.2。

#### 3.4.4 LLM 怎么知道并使用：提示说明与真实值走两条路

**LLM 不会自动看到进程环境。** Paperclip 的 `renderPaperclipEnvNote` 列出本轮可用的 `PAPERCLIP_*` 变量名，`renderApiAccessNote` 给出带 `$变量名` 的 curl 用法，`buildPrompt` 将这些说明加入任务 prompt；外部聊天分支会跳过这两段说明。这里的任务 prompt 经 ACP 交给 Tomcat，再随模型请求送给 LLM，不能把它误称为 Tomcat 自动将环境值塞进系统提示词。

```text
                         Paperclip
                             |
           +-----------------+------------------+
           |                                    |
           v                                    v
给模型看的说明                           给执行程序的环境
“有 PAPERCLIP_API_KEY，                   PAPERCLIP_API_KEY=实际凭据
 用 $PAPERCLIP_API_KEY 调接口”            PAPERCLIP_TASK_ID=task-123
           |                                    |
           | ACP session/prompt                 | acp 本机握手
           v                                    v
Tomcat 把任务说明交给 LLM                 serve 保存连接快照
           |                                    |
           v                                    |
LLM 返回 bash 工具调用                          |
命令里仍然写 $PAPERCLIP_API_KEY                  |
           |                                    |
           +-----------> Tomcat 权限检查 <------+
                              |
                              v
                    启动带该会话环境的 shell
                              |
                    shell 用真实值替换 $变量名
                              |
                              v
                    curl 发请求 -> 接口返回结果
                              |
                              +-> 工具结果返回 LLM
```

对应宿主源码：`../paperclip/packages/adapter-utils/src/acpx-engine/execute.ts:3047-3153`。这两段提示只说明变量名和用法，不插入凭据的真实值。环境快照负责“工具确实拿得到值”，提示说明负责“模型知道如何引用”，两者不能互相替代。

下面把宿主给出的查询身份示例展开为可读的 shell 命令；它只是文档示例，本轮不访问真实 Paperclip 服务：

```sh
PAPERCLIP_API_BASE="${PAPERCLIP_API_URL%/}"
PAPERCLIP_API_BASE="${PAPERCLIP_API_BASE%/api}"
curl -s \
  -H "Authorization: Bearer $PAPERCLIP_API_KEY" \
  "$PAPERCLIP_API_BASE/api/agents/me"
```

前两行去掉 URL 末尾的 `/` 和已有 `/api`，避免重复拼路径。LLM 生成的命令使用变量名；shell 执行时才取真实值，再交给 curl。宿主还提供了向 `/api/issues/$PAPERCLIP_TASK_ID/comments` 发评论、携带 `X-Paperclip-Run-Id` 的示例，见上述 `renderApiAccessNote`。

```text
LLM：    我写“用 $PAPERCLIP_API_KEY”，不必知道凭据内容
Tomcat： 我把当前会话的环境交给这个 shell
shell：  我取出对应的 value，展开命令后执行
```

**说人话：**模型负责写操作步骤，执行器负责准备材料，shell 负责按名称取材料。无需让模型先打印密钥再把密钥粘回命令。

**安全边界：**不主动把快照写入提示词、聊天、日志或会话文件，不等于模型绝对无法得到其中的值。如果工具运行 `env`、打印凭据或开启 shell 跟踪，秘密仍可能进入输出；不得把这套传递方式宣传成秘密隔离沙箱。排错优先检查变量是否存在，不打印真实凭据。

#### 3.4.5 MCP 与服务设置：同一来源，不同使用范围

bash 通常直接用连接快照执行命令；**stdio MCP 只从快照取 PATH/HOME 等白名单启动变量，再叠加该 MCP 明确配置的 env**。需要凭据时，由宿主或用户显式配置给对应 MCP；不能认为快照里有 `PAPERCLIP_API_KEY`，所有 MCP 就会自动收到它。

```text
连接快照 ------> bash：应用快照 + Tomcat 必需内部标志
    |
    +-- 只取白名单变量 --+
                         +--> MCP 子进程 -> 程序自行读取环境、使用凭据
MCP 显式 env -----------+          ^
                                  |
                         LLM 调它公开的工具
                         不必自己拼 curl 或读取密钥

Tomcat 保存的凭据 --------> Tomcat 调用大模型（不从会话环境取 key）
出网 HTTP 代理 -----------> llm.proxy → 系统设置 → 直连（详见 §6.3）
```

证据：`tomcat/src/core/connector/mcp/transport.rs:217-227`（先清除环境，再白名单继承，最后叠加显式 env）。MCP 的显式 env 值按字符串传入，不经过 shell 展开；把 value 写成 `$PAPERCLIP_API_KEY` 不等于自动引用快照。HTTP MCP 则走 headers/OAuth 等配置，不属于“启动本机 MCP 子进程”的流程。

**说人话：**给 shell 的任务工具箱和给每个 MCP 的工具箱不是同一套发放规则。共享模式的模型 key 只认 Tomcat 保存凭据，Tomcat HTTP 代理也统一选择，不跟随窗口或宿主；迁移办法见 §6.3。旧 chat/stdio 行为不变。项目 MCP 同目录共享、采用首个创建者环境的限制仍见 §3.2.3，本节不把它说成完全会话隔离，也不改变 stdio 工具自己的联网环境。

#### 3.4.6 测试用例清单与本次文档检查

本次只补说明和图，不新增产品机制或付费模型调用。下面把本节承诺对应到已存在的测试入口或 §8 的既有验收项；测试结果仍以运行时计划证据为准，不因读过测试源码就记为通过。

| 用例 | 文件 / 验收项 | 说人话 |
| --- | --- | --- |
| 两连接的 bash 拿各自 PATH/变量，接手后用新控制者的环境 | `tomcat/tests/acp_stdio_e2e.rs::connection_environment_reaches_bash_and_is_replaced_on_live_session_takeover`（T12） | 后来者不误用第一个人的工具和变量。 |
| 同目录的会话 MCP 私有且交接前旧进程停止；白名单仍生效 | `tomcat/tests/acp_stdio_e2e.rs::session_mcp_same_cwd_is_private_uses_controller_environment_and_stops_before_handoff`；`tomcat/src/core/connector/mcp/tests/transport_test.rs`（T13） | 不串任务凭据，也不扩大 MCP 的继承范围。 |
| 服务不使用启动者模型 key/代理兜底，保存的配置仍有效 | `tomcat/tests/serve_shared_e2e.rs::shared_service_ignores_starter_model_config_proxy_and_keeps_saved_credentials`、`shared_service_does_not_fall_back_to_a_starter_key_when_saved_key_is_missing`（T12） | 模型账号不由“谁先启动服务”决定。 |
| Paperclip prompt 告知变量名/用法而不插入凭据值；任务环境传到 Agent | `../paperclip/packages/adapter-utils/src/acpx-engine/execute.test.ts`；宿主适配器端到端仍归 T21/T25 | 模型收到使用说明，工具收到正确值，不能只验证其中一半。 |
| 本次 Markdown、JSON 握手样例、shell 示例语法与源码引用一致 | 本文 §3.4、§4.1；按 §8.1 做文档检查 | 文档检查不替代真实工具和宿主验收。 |

## 4. 协议、数据与调用样例

标准 ACP 与 Tomcat 本机协议分开：前者给宿主，后者只在 relay/扩展与 serve 之间。**§4.1 按当前工作树的 `tomcat/src/api/serve/service.rs`（`ConnectionHello` / `ServiceInfo`）和 `tomcat/src/api/acp/{mod,forwarder}.rs` 校正**，不再沿用旧草案的 `mode` / `env` / `drain_connection` 消息。原生命令的单一事实源仍为 `tomcat/src/api/serve/types.rs`，沿现有 schema/wire 流程维护；本次文档补充不新增协议字段、生成管线或产品行为。

**说人话：**平台只听标准 ACP；服务怎么启动、怎么请某根连接先停手，是 Tomcat 自己内部的事。

### 4.1 本机握手与优雅离开

本机流先交换一行 JSON 握手；native/acp 成功后在同一连接传相应协议消息，status/stop 用于管理服务。握手只存在于本机 IPC，不进入宿主的 ACP stdout，也不送给 LLM。下面列出当前 `ConnectionHello` 的全部字段；客户端应显式发送环境快照。

| 字段 | JSON 类型 | 必填 | 默认值 | 适用场景 | 说明 | 说人话 |
| --- | --- | --- | --- | --- | --- | --- |
| `service` | string | 是 | 无 | 所有握手 | 必须为 `tomcat`；null 拒绝 | 确认连的是 Tomcat。 |
| `protocolVersion` | integer（u32） | 是 | 无 | 所有握手 | 当前为 1，非 ACP 版本；不匹配拒绝 | 先确认内部协议兼容。 |
| `clientVersion` | string | 是 | 无 | 所有握手 | 客户端二进制版本；仅 native 要与服务版本相同，ACP 只受本机协议版本约束 | 界面版本要匹配，中转不必同版本。 |
| `protocol` | string | 是 | 无 | 所有握手 | `native` / `acp` / `status` / `stop`；未知值/null 拒绝 | 这条连接接下来做什么。 |
| `environment` | object<string,string> | 否（客户端应传） | `{}` | 连接环境 | 当前反序列化允许省略；null 拒绝；空表不回退服务环境 | 没带变量就是没有，不借别人的。 |
| `skillsDir` | string/null | 否 | null | ACP 连接 | 省略/null 不叠加；非空时须是可读绝对目录且 protocol 为 acp | 可选宿主私有技能目录。 |

`environment` 是 K-V 对象，不是字符串化 JSON，也不是 ACP `mcpServers[].env` 的数组。名称/值必须能表示为 UTF-8；名称不许为空或含 `=`/NUL，值不许含 NUL；Windows 额外拒绝忽略大小写后的重名。源码：`tomcat/src/infra/platform/environment.rs::capture`、`validate`。

握手整行（含换行）上限为 1MiB，连接/握手等待为 5 秒。`write_json` 序列化后追加 `\n`；`read_json` 有界读到换行再解析。非法 JSON/缺必填字段等解码失败可直接结束连接；解码成功后的版本、环境或技能目录校验失败，回复带 `error` 后结束，不承诺所有失败都有统一 JSON 错误外壳。

```text
acp 的进程环境
    -> capture 得到 EnvironmentSnapshot（K-V）
    -> ConnectionHello.environment
    -> JSON + 换行 --本机 socket/命名管道--> serve 校验
    -> ServeConnection.environment（内存）
    -> 会话执行上下文 -> 工具子进程环境

握手成功后：同一条连接继续传 ACP；宿主只看见标准 ACP 输入/输出。
```

**说人话：**首次接线时先递一次“任务配置单”，之后才转接对话。握手是 Tomcat 内部交接，不要求 Paperclip 或 Multica 会讲这套消息。

下例值均为虚构；为便于阅读排成多行，实际线上压成一行并以换行结束。`<binary-version>` 须替换成双方实际相同的版本，路径随平台改变：

```json
{
  "service": "tomcat",
  "protocolVersion": 1,
  "clientVersion": "<binary-version>",
  "protocol": "acp",
  "environment": {
    "PATH": "/usr/local/bin:/usr/bin:/bin",
    "PAPERCLIP_API_URL": "https://paperclip.example.invalid",
    "PAPERCLIP_API_KEY": "example-only-not-a-real-token",
    "PAPERCLIP_TASK_ID": "task-123",
    "PAPERCLIP_RUN_ID": "run-456"
  },
  "skillsDir": null
}
```

成功回复为 `ServiceInfo`，不回显环境值：

```json
{
  "service": "tomcat",
  "protocolVersion": 1,
  "serverVersion": "<binary-version>",
  "instanceId": "example-instance-id",
  "pid": 12345,
  "agentId": "main",
  "endpoint": "/example/agent/serve/serve.sock",
  "running": true
}
```

回复中的字段均由服务填写：`service`、`serverVersion`、`instanceId`、`agentId`、`endpoint` 是字符串，`protocolVersion`、`pid` 是 u32，`running` 是 boolean；没有缺省/null。`endpoint` 在 Windows 为命名管道名称。校验失败时同一对象额外带 `error` 字符串，成功时省略。客户端核对 service、端点、agent、协议和服务版本，环境值不参与日志输出。证据：`tomcat/src/api/serve/service.rs::ServiceInfo`、`open_management`、`handle_connection`。

**优雅离开也按当前实现校正。** relay 在宿主 stdin EOF 后，沿**原本的本机数据连接**发送私有记录；不另建旧草案中的 `drain_connection` 管理连接。下面的 `NUL` 表示一个零字节，`LF` 表示换行，不是把这些字母作为 JSON 发送：

```text
宿主关闭 stdin
    -> acp 发送 [NUL]tomcat.close[LF]
    -> serve 在交给 ACP SDK 之前识别并消费
    -> 取消该连接任务，等待清理
    -> 成功：[NUL]tomcat.closed[LF]
       失败：[NUL]tomcat.close_failed[LF]
    -> acp 消费确认，不转发到宿主 stdout；成功才正常退出
```

确认等待上限为 60 秒；超时、断流或失败确认返回错误，不谎称工具已停止。这些记录不是标准 ACP 方法，只用于 Tomcat 私有本机这一段；宿主仍只需关闭 stdin。`protocol=stop` 则是另一个明确的全服务管理动作，不能与上述单连接退出混淆。证据：`tomcat/src/api/acp/mod.rs:33-169`，`tomcat/src/api/acp/forwarder.rs:42-95`。

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
| `scope` | string/null | 否 | live | list_sessions | `live` / `disk`；VS Code 显式用 disk，保留原项目范围；不合并 Live | 旁观也在原来的项目列表里。 |
| `params` | object | 否 | `{}` | get_messages | 沿既有 GetMessagesParams；null 不等于省略 | 分页参数照旧。 |
| `params.attachmentMode` | string | 否 | inline | get_messages | `inline` / `reference`；扩展沿用 reference | 图片不因旁观另复制一份。 |
| `activeSessionId` | string | switch 成功回复必填 | 无 | switch_session payload，已有 | 本连接这次打开的目标 | 确认打开的是哪一页。 |
| `access` | string | 共享 switch 成功回复必填 | 无 | switch_session payload【新增】 | `control` / `observe`；服务按 §3.2.5 判定，仅存扩展视图内存 | 这扇窗口能操作还是只能看。 |
| `origin` | object/null | 共享 switch 回复必填；项目列表可投影 | 原生/老会话为 null | switch_session payload、项目磁盘摘要【新增】 | 来自会话头；对象为下列 kind/clientName，不是权限结果 | 来源重启后还在。 |
| `origin.kind` | string | origin 非 null 时必填 | 无 | origin 子结构 | 本期仅 `acp`；是服务判断来源的稳定代码 | 不看平台名字猜来源。 |
| `origin.clientName` | string/null | 否 | 缺省/null 均无名称 | origin 子结构 | ACP initialize 的 clientInfo.name；未提供/空名显示“ACP 客户端” | 名称只是给人看的。 |
| `cwd` | string/null | 共享响应必填 | 无 | get_state 已有 | 共享会话返回已校验的绝对路径；旧 stdio 可空；不为旁观扩展 Live 摘要 | 标出任务在哪个目录。 |
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

补发仍用原有事件外壳；当前实现见 `tomcat/src/api/serve/event_pump.rs::MessageReplay`。`message.content` 与 `message.thinking_text` 是截至开始接收时的完整前缀，`assistantMessageEvent` 为 null，表示按稳定 ID 替换而非追加。后续实时事件才使用 `content_delta` / `thinking_delta`；message_end 不带该子结构：

```json
{"type":"message_update","sessionId":"s1","assistantMessageId":"a1","message":{"role":"assistant","content":"正在检查登录逻辑","thinking_text":"先查看入口与测试"},"assistantMessageEvent":null}
```

```json
{"type":"message_end","sessionId":"s1","assistantMessageId":"a1","message":{}}
```

这两条示意适用于“消息已结束，但下一条尚未开始”的补发；若仍在生成，只补正文/思考全文，不提前发结束事件。用户 prompt、已完成消息和工具结果从原历史接口恢复，工具后续状态走原有 toolCallId 更新；不新增用户消息广播或工具画面协议。补发不会重新执行 prompt，也不允许旁观者作权限回复。

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
       |   src/api/acp/forwarder.rs     转发 + stdin EOF 私有 close/确认（当前实现）
       |   [src/api/acp/tests/]         协议/转发单测【新增】
       |
       +-> src/api/serve/service.rs     端点、启动锁、start/status/stop【新增】
       |      -> src/infra/platform/process.rs  setsid/Job 脱离【修改】
       |      -> src/infra/platform/fs.rs       私有权限/安全打开【复用或补管道支持】
       |      [src/infra/platform/tests/]
       |
       +-> src/api/serve/service.rs             ConnectionHello / ServiceInfo（握手类型已落地于此）
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
       +-> src/core/llm/system_prompt.rs        会话目录渲染进系统提示词【只复用】
       |   src/core/prompts/templates/system/workspace_context.txt  会话目录表述【修改】
       |   src/core/agent_loop/reasoning_loop.rs 模型请求组装【只复用】
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
 src/core/tools/primitive/bash_task.rs           统一 spawn、stop/wait；会话 cwd【修改】
 src/core/tools/primitive/executor/bash.rs       fallback 同源环境与目录【修改】
 src/core/tools/primitive/executor/gate.rs       相对路径基准，权限检查前补齐【修改】
 src/infra/platform/paths.rs                    路径规范化【复用；会话基准由调用方明确提供】
 src/core/plan_runtime/{prod_reviewer,verify}.rs  子代理工厂继承【修改】
 src/ext/dispatcher/ops.rs                       executeBash 取调用会话依赖【修改】
 src/core/connector/mod.rs                       MCP 工厂：scope/会话分开【修改】
 src/core/connector/mcp/{manager,lifecycle,connection,transport}.rs
                                                环境传递、重连同源、cwd【修改】
 src/core/llm/{auth,resolver}.rs                  共享模式 managed-only【修改】
 src/infra/http_client.rs                        保留系统代理发现，启动者变量在 service.rs 清除【修改】
 src/core/llm/http_client.rs                     普通客户端与配置代理【只复用】
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
| `serve.max_buffered_frames` | 现有配置，默认 64 | 原生连接按会话缓冲判定丢增量阈值；ACP 覆盖为 `usize::MAX`；不是全连接硬上限或字节上限；不触发断连 | 原生直播可降级，ACP 正文保留完整。 |
| Bash 环境 | 控制连接快照 | 清继承后应用快照，再加 Tomcat 必需内部标志；新控制者接手后换快照 | 任务工具用自己的 PATH 和变量。 |
| MCP 环境 | 快照白名单 + server 显式 env | 显式配置覆盖白名单；stdio cwd 明确绑定；不透传整份秘密 | 工具只拿它该拿的环境。 |
| 模型 key | Tomcat 保存的凭据 | **共享模式不回退 shell 或宿主智能体环境里的 key**；删除保存 key 后不可复活旧副本 | 只靠环境变量的用户须在 Tomcat 设置里保存一次。 |
| 出网 HTTP 代理 | `llm.proxy` → 系统设置 → 直连 | 不读取窗口/ACP 连接的代理变量；启动时删除启动者 8 个代理变量；MCP 不用 `llm.proxy`，localhost 保持直连；测试例外见 §6.3 | 所有入口共用服务的网络配置。 |
| `--skills-dir` | 可选绝对目录 | 连接私有，new/load 重扫；不写共享 home | 临时技能不会跑进别人的会话。 |
| 项目规则 | AGENTS.md + 现有 alwaysApply rules | AGENTS 根→cwd 在前，原 rules 在后；各自预算不混用 | 项目手册完整读，原规则预算不变。 |
| 开发依赖 | 官方 ACP SDK 3.x | 只选稳定 v1；新依赖/feature 进入 Cargo.lock 并检查许可证、MSRV | 不为两个接口引入整套网络框架。 |

**JSON 键顺序约束。** `agent-client-protocol 3.4.0` 与 `agent-client-protocol-schema 1.12.0` 都启用了 `serde_json/preserve_order`（可用 `cargo tree -e features -i serde_json` 核对）。Cargo feature 全局合并，因此 JSON 对象保留插入顺序，不再默认按键名排序。凡是要求字节稳定的 JSON，例如缓存前缀和指纹输入，必须显式递归排序。现有 `tomcat/src/core/llm/system_prompt.rs::ToolSurface::from_plugin_tools_with_runtime_policies` 已调用 `sort_all_objects()`，由 `tool_surface_is_byte_stable_when_registry_observation_order_changes` 和 `tool_surface_normalizes_nested_object_keys_without_invalidating_the_prompt` 守住。工具目录文档保持便于阅读的书写顺序，不改生成器；文档与生成结果的一致性继续由 `tomcat/tests/tool_catalog_doc.rs` 检查。

背压参数沿用 `tomcat/src/infra/config/types/runtime.rs:22-39`，不新增配置项或队列层。内部 git/rg、插件 secrets、MCP HTTP/OAuth 等仍有自己的环境读取点，不能凭主 Bash 已改就宣布“所有进程绝不受启动者影响”。共享模式装配需审计这些调用；对于本期只保持既有语义的路径明确记录，不扩大权限。项目 MCP 缓存例外不应被测试描述成完全隔离。

### 6.1 本机部署、首次启动与升级

```text
同一台执行机器 / 同一用户 / 同一 Tomcat 数据目录与 agent
  ├─ Paperclip 本地执行端 ─┐
  ├─ Multica daemon ──────┼─> 一个共享 serve
  └─ VS Code 同项目窗口 ──┘      └─ 各会话保留自己的 cwd、环境与控制者
```

**首次使用。** 先安装本次实现的 CLI，并用 `tomcat init` / 既有模型配置入口保存模型与凭据；共享服务不使用启动者 shell 的模型 key 兜底。VS Code 要求其所选 CLI 与服务版本一致；宿主 ACP 中转只需本机协议兼容，不要求三个入口的 CLI 数字版本相同。开发版即使版本号相同，也要确认 `acp --help` 与 `serve status` 可用。

```sh
tomcat --version
tomcat acp --help
tomcat serve status
tomcat serve start
```

`status` 只检查，不启动；`start` 启动或复用已有实例。宿主允许进程脱离时，也可以省略显式 start，让首次 ACP / VS Code 连接按已有启动流程拉起服务；Windows + Multica 的预启动要求见下文。Paperclip 的环境检查把“CLI 可用、服务尚未启动”作为信息，不当成阻止 Agent 就绪的警告；检查本身不会起服务或调用模型。

| 入口 | 设置与操作 | 不能省略的边界 |
| --- | --- | --- |
| Paperclip | 选择 **Tomcat** 本地适配器；沿原项目工作区设置目录；需要时配置可执行路径与模型 | 执行端必须能访问该目录；云端执行端不会因此获得用户电脑的目录。 |
| Multica | 在执行机器安装 CLI；必要时给 daemon 设置 `MULTICA_TOMCAT_PATH`；沿原 runtime / Agent / 项目资源流程派任务 | 本地目录、临时 worktree、持久工作区的寿命由宿主决定；Windows 上派任务前须先启动共享服务（见下文）。 |
| VS Code | 使用本次扩展及其所选 CLI、同一数据目录；打开宿主实际工作的项目，再从原会话列表打开 | 别的项目不可见；临时 worktree 要开它自己的窗口；旁观不能发送、停止、改模型或回滚。 |

**升级或回退。** 停服是明确的全局动作，会取消这个服务上的全部任务；先安排好宿主任务，再操作，不以“关闭某个旁观窗口”代替停服。

```text
通知各控制方并安排任务
  -> tomcat serve stop（确认成功）
  -> 更新 CLI 与 VS Code 扩展 / 调整宿主可执行路径
  -> tomcat serve start
  -> 各客户端重新连接；由宿主决定后续重试
```

CLI 已更新时，也可明确执行 `tomcat serve restart`。旧连接不会因为别的窗口停服就自动拉起服务；VS Code 版本不匹配立即停止重试，给出带“会停止所有任务”后果的重启按钮，复用已有重启命令、不二次确认；不偷偷退回私有 stdio。回退同样先停共享服务；不要让旧 chat/stdio 与共享服务同时执行同一份历史。当前运行证据来自 macOS；Windows 命名管道、系统代理和 Job 生命周期转由 GitHub Windows CI 实际验证，个人 PC 体验可选，未取得云端结果前不计通过；Paperclip 包内启动命令使用 POSIX shell，不宣称原生 Windows 已支持。

**Windows + Multica 必须预启动。** 在独立终端执行 `tomcat serve start`，或先打开使用同一 Tomcat 数据目录与 agent 的 VS Code 窗口，再派发 Multica 任务。Multica 的 Job 只有 `KILL_ON_JOB_CLOSE`、没有 `BREAKAWAY_OK`（`multica/server/pkg/agent/proc_windows.go::ownProcessTree`）；Tomcat 用 `CREATE_NO_WINDOW | CREATE_BREAKAWAY_FROM_JOB` 启动共享服务（`tomcat/src/infra/platform/process.rs::spawn_service_process`），服务拥有供子进程继承的隐藏控制台，避免 git、bash 和 stdio MCP 弹出终端窗口；在这种 Job 内启动仍会被 Windows 拒绝。已运行的服务可直接连接，任务结束后仍保持运行。

```text
Windows Multica 的受管 Job
  └─ tomcat acp ── 服务已运行 ──> 直接连接
               └─ 服务未运行 ──> 不允许脱离，报错提示先 tomcat serve start
```

本期接受限制，不绕过宿主进程管理。只有 Windows 用户实际反馈预启动太麻烦时，再与 Multica 协商仅对 Tomcat 后端开放 `BREAKAWAY_OK`。按用户最新验收范围，GitHub 只运行 Tomcat 仓库的测试：`serve_shared_e2e.rs` 的真实 Win32 Job 用例、假 ACP 宿主或直接协议代码验证连接/取消/预启动后服务存活，以及真实 VS Code 退出后服务存活。两宿主仅在本机验证与 Tomcat 有关的适配器与任务，不创建 fork、不接入宿主远端 CI。模拟宿主不等于验证完整 Multica Windows 运行环境；必须准确记录此边界，本机 macOS 也不代签 Windows 结果。

**Windows CI 启动边界。** 系统代理由原 Node 脚本串行保存/设置/恢复，两个 Rust 对照用例改由现有 nextest 直接运行，而非 Node 执行裸测试 exe；真实 VS Code 仍走原启动器。nextest 的 [`runner/windows.rs::create_job`](https://github.com/nextest-rs/nextest/blob/main/nextest-runner/src/runner/windows.rs) 给测试自己的 Job 设置 `BREAKAWAY_OK`；两个用例实际查询并记录当前 Job 标志，不符合就失败。[Node 22.23.3 的 `uv__init_global_job_handle`](https://github.com/nodejs/node/blob/v22.23.3/deps/uv/src/win/process.c#L69-L121) 还设置 `SILENT_BREAKAWAY_OK`，会影响下一级 Rust 子进程的继承，所以 runner 必须直接拥有被测 Rust 进程，而不是再包一层整个 Node 脚本。按 [Windows 嵌套 Job 规则](https://learn.microsoft.com/en-us/windows/win32/procthread/nested-jobs)，脱离内层 Job 不代表脱离不允许脱离的外层 Job；本次没有修改 runner 的外层 Job、产品启动参数或受限宿主用例。先前自制 Node detached 包装未解决 CI 启动拒绝，已删除，恢复标准 `execFile`。若 nextest 或 runner 的 Job 条件改变，应重新核对这个测试入口，不能放宽宿主权限或跳过启动/存活断言；实际通过与否仍看同修订 CI 证据。

### 6.2 停止后继续：原会话不等于原任务续跑

```text
控制连接断开 -> Tomcat 停任务与工具 -> 历史保留
                                      |
                 宿主原有恢复规则 -----+
                   ├─ 目录、会话与恢复条件仍有效 -> load 原会话，开始新一轮
                   └─ 宿主要求 fresh / 删除了目录 -> 新会话或明确失败
```

- **Paperclip：**Tomcat 须登记进现有会话型适配器名单（`server/src/services/conversation-continuation.ts` 与 `ui/src/components/TaskChatThread.tsx`），否则失败后的消息会按未知执行器进入核对流程。这只是标准登记，不改变停止证明、工作区恢复或重试算法。显式恢复可能按宿主规则强制 fresh，不能把所有 Retry 都说成原 ACP ID 续聊。
- **Multica：**原生 `local_directory` 的一次性 worktree 正常收尾后会删除；下一轮换目录时，宿主会放弃旧 session ID。不能为了测试续聊而保留/重建旧路径、改数据库或绕过恢复门。worktree 的旁观与清理、持久工作目录的同会话继续分别验证。依据：`server/internal/daemon/daemon.go` 的 prior-workdir / resume 判定及 `execenv/local_worktree.go` 的 finalize；`execenv/tomcat_test.go` 保留删除旧 workdir 的断言。

**测试入口。** 原有共享服务/ACP 测试负责并发启动、版本冲突、明确停服和断连停工具。完整宿主运行中，可把 `homePath`、`workspacePath`、`binary`、真实 `sessionId`、预期 `prefix` 与 `visible` 写入隔离夹具 JSON，以 `TOMCAT_E2E_EXTERNAL_OBSERVATION=<绝对路径> TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR=<制品目录> npm run test:e2e:vscode-devhost` 启动独立窗口验证；夹具只连接现有宿主会话，不替代宿主、不另建执行会话。测试文件为 `src/test/suite/support/externalHostObservationScenario.ts`；PNG、ARIA、console 与断言记录仍须实际读取，不能仅凭启动成功记为验收通过。

### 6.3 行为变化与迁移

```text
旧：每个 VS Code 窗口各有执行进程
新：窗口 / ACP 宿主 --> 同一个共享服务
                         ├─ 模型 API key：Tomcat 保存的凭据
                         ├─ Tomcat HTTP 代理：保存配置或系统设置
                         ├─ bash / MCP stdio 任务环境：仍沿原控制方规则
                         └─ 项目列表：仍按窗口原有项目范围，只叠加只读旁观
```

| 变化 | 用户怎么迁移 / 边界 | 代码证据 |
| --- | --- | --- |
| 窗口连接共享后台，不再拥有私有服务进程 | 关窗不等于停服务，但仍会取消本窗控制的任务；显式重启会停止所有客户端的任务 | `tomcat/src/api/serve/service.rs`；`tomcat-vscode-ext/src/serveClient/sharedConnection.ts` |
| 模型 API key 不再从环境兜底 | 仅在 shell 导出 key、或仅在 Paperclip/Multica 智能体环境配置 key 的用户，须在 **Tomcat 设置里保存一次**；原来已保存的不受影响。缺 key 会明确提示，旧 chat/stdio 不变；任务工具仍可使用宿主传入的环境 | `tomcat/src/core/llm/auth.rs::process_env_value`、`tomcat/src/api/cli/mod.rs::preload_runtime_env` |
| “允许此文件夹”跨会话保留 | 原来 VS Code 只在当前会话生效；现在将所批准目录写入 `workspace.workspace_roots`，与 CLI 一致。仅持有控制权且目录与所提供选项一致的回复能保存 | `tomcat/src/api/serve/confirmation.rs::ServeConfirmationProvider::confirm_decision` |
| 版本不一致时不再无效重试 | VS Code 给出重启共享服务按钮，文字写明会停止所有任务；点击沿已有命令直接执行。ACP 中转不要求 CLI 数字版本相同，但本机协议不兼容仍拒绝 | `tomcat/src/api/serve/service.rs::handle_connection`；`tomcat-vscode-ext/src/serveClient/ServeConnectionSupervisor.ts`、`tomcat-vscode-ext/gui/src/App.tsx` |
| 同项目宿主会话可只读旁观 | 仍走原会话列表；其它项目不可见。独立 worktree 需打开它自己的窗口，旁观关闭不取消宿主任务 | `tomcat/src/api/serve/ownership.rs`；`tomcat-vscode-ext/src/ui/webview/state.ts` |

**代理需要迁移的用户。** 如果此前只在窗口终端或宿主智能体环境里设置 `HTTP_PROXY` / `HTTPS_PROXY` / `ALL_PROXY` / `NO_PROXY`，正式共享服务不再使用它们。请改用系统代理，或在 Tomcat 配置中保存 `llm.proxy`；模型、网页工具和插件 HTTP 复用普通客户端，不再维护会话代理句柄、代理路由键或接管刷新。服务启动时删除 8 个大小写代理变量，防止第一个启动者影响全部连接。

远程 MCP HTTP/OAuth 只保留系统代理，不扩大 `llm.proxy` 的作用范围；回环目标（`localhost`、`127.0.0.0/8`、`::1`）保持直连。LLM 未配置显式 `llm.proxy`，且主地址与可选 `llm.api_base_fallback` 全部为回环时，也绕过环境／系统自动发现的代理；显式代理语义不变。主地址为回环而 fallback 为远端时仍保留自动发现，因为它们共用一个 client。网页工具与插件 HTTP 不受此规则影响。bash、MCP stdio 子进程的环境、cwd、权限和接管行为不变；它们自身的联网行为不属于“Tomcat HTTP 统一代理”的承诺。独立 CLI / `serve --stdio` 除上述回环规则外仍使用原环境代理逻辑。

**仅窗口测试入口的例外。** `scripts/run-vscode-devhost.ts` 可显式指定 `TOMCAT_E2E_WINDOW_PROXY_ENV=HTTPS_PROXY`（或一个 HTTP/ALL 的大小写别名），将该变量的地址交给 `scripts/realServeFixture.ts`，在新建的隔离 HOME 中写入 `llm.proxy` 后启动正式服务。明确的 `TOMCAT_E2E_LLM_PROXY` 测试配置优先；不选择时不读取开发机代理，所选变量缺失时使用原无代理夹具，非法地址直接报错且不输出凭据。不模拟完整多协议/NO_PROXY 优先级，不改用户 HOME，不给 ACP 或普通扩展加开关。仅适用于隔离真实服务测试，假服务和外部宿主观察入口不能借此修改已有后台。

```text
窗口测试输入 --显式选择--> 隔离配置 llm.proxy --> 正式 Tomcat 客户端
窗口 / ACP 正式环境 --------------------------------X（不参与选择）
```

代码证据：`tomcat/src/api/serve/service.rs::service_command` 清除启动者变量；`tomcat/src/infra/http_client.rs::build_outbound_client` 保留普通代理与系统发现；`tomcat/src/core/connector/mcp/transport.rs::http_client_builder` 保留 MCP 系统/localhost 边界。只有日后明确要求正式入口使用不同代理时才重新讨论，不为测试恢复动态代理；实际系统代理验收另行记录，不由这些源码说明代签。

本机 macOS 系统代理项按用户最新要求仅只读打印 IP/端口，不修改全局设置；该项不声称模型/MCP 请求已实测经代理。Windows 仍在一次性 CI runner 的显式步骤验证系统设置与受控请求，真实模型费用授权则只用于本机 Multica 两轮问答，两者不混称。

**发布前必须做的事。** CLI 版本必须高于已发布且没有共享服务/ACP 的 `0.1.80`，不能沿用同一个数字冒充新构建；发版时按既有流程同步更新 CLI、扩展和内置 CLI 的版本记录，再由 `.github/workflows/version-sync.yml` 检查一致性；这个 CI 工作流只校验，不替人升版本。CHANGELOG 暂用 `Unreleased`，版本号由发版流程填写；本整改不手改版本、不发布；按用户条件授权，本地 macOS 验收和差异检查通过后可提交、推送本任务功能分支以触发 CI，不含无关改动、合并主分支或强推。升级与回退的停服次序见 §6.1。

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
连接读得慢/输出积压  -> 原生可丢增量并提示；ACP 合并但不丢正文；均保持连接与任务
AGENTS.md 读取失败    -> 跳过该文件 + instruction summary 诊断
```

| 归一化原因 | 行为 | 说人话 |
| --- | --- | --- |
| protocol/version mismatch | 所有连接拒绝本机协议不兼容；仅原生拒绝 CLI 版本不同，VS Code 立即给重启入口；不另起第二实例 | 区分协议不兼容和界面版本不匹配。 |
| invalid cwd / scope / origin | new/load 或控制操作失败；不回落服务目录 | 目录和会话身份不能猜。 |
| session_in_use | 原控制者仍活着则拒绝；停止中也不接手 | 不能两个人同时下指令。 |
| read_only_session | GUI 按 access 置灰；服务在分发入口拒绝白名单外会话命令，扩展沿原失败提示显示 | 绕过按钮也不能改，不重复加 Host 拦截。 |
| backpressure（原生提示，不是错误） | 原生沿原 writer 合并、满时丢消息增量并提示；ACP 只合并、不丢正文、不插提示；两者均不断开、不取消 | 原生直播可能缺片，宿主结果不能缺字。 |
| stop_timeout | 不报 stopped、不释放未停止状态；记录残留任务 ID | 退出码和“已停手”要对得上。 |
| connection_lost / service_unavailable | 按连接角色取消或退订；不直接显示成功/已取消 | 看不到结果不等于结果已完成。 |
| instructions_skipped | 沿摘要给出路径和理由，不把正文写日志 | 不能静默丢掉手册。 |

panic/JoinError 作为该连接或会话失败收口，触发相同清理；不可让一个连接解码/handler 的失败直接结束整个服务。进程级崩溃仍会中断全部任务，不承诺热恢复。

## 8. 测试矩阵与验收

**实现已落地，本机验收结论见 §8.2。** 下表保留设计场景和编号，不再维护第二份进度状态。标【新增】的用例名是设计锚点，实际用例可能合并在同一测试文件内；具体命令、任务日志和制品证据见开发计划。此前 macOS 两宿主完整任务已实际运行，使用的是受控模型；Windows CI 实际验收和本轮代理回退的回归不能用这些历史结果代替。

| 维度 / 编号 | 用例 / 编号 | 结论索引 | 说人话 |
| --- | --- | --- | --- |
| P0 规则 T01 | 【新增于现有文件】`tomcat/src/core/tests/project_instructions_test.rs::agents_md_root_to_cwd_and_worktree_boundary` | 见 §8.2 | 根与子目录顺序正确，不越仓库；无 git 只看 cwd。 |
| P0 读取安全 T02 | 【新增】同文件 `agents_md_symlink_size_utf8_and_budget`；扩展 `rejects_leaf_and_ancestor_links` | 见 §8.2 | 链接越界、硬拒绝、64KiB、UTF-8、BOM、普通 YAML 分隔符；40KB 不占 rules 预算。 |
| P1 逐轮刷新 T03 | 扩展 `tomcat/src/api/serve/tests/instruction_prompt_test.rs::project_rules_refresh_without_tool_grants` | 见 §8.2 | 改手册下一轮生效，不进历史，不生成工具授权。 |
| P0 单实例 T04 | 【新增】`tomcat/tests/serve_shared_e2e.rs::concurrent_start_reuses_one_instance` | 见 §8.2 | 10 个启动者只得到一个服务；残留、权限错误、版本不符分别处理。 |
| P0 平台 T05 | 【新增】同文件 `service_survives_launcher_group_exit`；扩展 `tomcat/src/infra/platform/tests/process_test.rs` | 见 §8.2 | 真进程组/Windows Job 被杀，服务活着；服务 cwd 不占 worktree。 |
| P0 路由 T06 | 【新增】同文件 `connection_routing_and_control_handoff` | 见 §8.2 | 同请求 ID 不串线，同 session 只有一个控制者；断线清空绑定但清理完成前仍不能接手，停止超时也不能放行。 |
| P0 停止 T07 | 【新增】`tomcat/tests/acp_stdio_e2e.rs::relay_eof_waits_for_tools_before_exit` | 见 §8.2 | grace 成功退出后文件不再写；覆盖后台 bash、子 agent、排队和等批准。 |
| P0 停止失败 T08 | 【新增】同文件 `forced_exit_and_drain_timeout_are_not_success` | 见 §8.2 | 硬杀触发取消；超时不能报 stopped，也不能放行第二执行者。 |
| P0 ACP T09 | 【新增】同文件 `standard_acp_new_load_prompt_cancel` | 见 §8.2 | 官方客户端与 hermesClient 均可握手/执行/取消/load；stdout 只有协议。 |
| P0 权限 T10 | 【新增】同文件 `permission_false_requires_valid_controller_reply` | 见 §8.2 | false 保存、批准前不执行、拒绝/取消/断开、旧回复；旁观连接收不到可作答的权限请求，伪造回复也无效；同时测默认 true 和硬禁令。 |
| P1 容量 T11 | 【新增】`tomcat/tests/serve_shared_e2e.rs::evicts_only_eligible_idle_slots` | 见 §8.2 | 2 个位子跑 5 个任务；不淘汰忙/排队/任意窗口当前显示（含旁观）的会话；腾不出报错。卸载后外部 Live 项消失，自己目录内会话可重新载入。 |
| P0 环境/目录 T12 | 【新增】`tomcat/tests/serve_shared_e2e.rs::session_env_is_independent_of_service_starter`、`session_cwd_reaches_model_and_tools`；扩展 `tomcat/src/api/serve/tests/registry_test.rs`、`tomcat/src/core/llm/tests/system_prompt_test.rs` | 见 §8.2 | 两种启动顺序下主 Bash、子 agent、fallback 路径拿自己的环境；§3.3 验证模型请求中的会话目录与实际文件/子进程一致，覆盖显式/相对/省略 cwd、已保存会话、无效目录及权限；不以提示词正确替代执行正确。 |
| P0 MCP T13 | 扩展 `tomcat/src/core/connector/mcp/tests/transport_test.rs`；【新增】`acp_stdio_e2e.rs::session_mcp_isolated_and_project_mcp_shared` | 见 §8.2 | ACP MCP 不串环境；白名单保留；项目共享例外有明确断言。 |
| P0 插件 T14 | 【新增】`tomcat/src/ext/dispatcher/tests/session_bash_env_test.rs::plugin_bash_uses_calling_session_env` | 见 §8.2 | 同目录不同连接的插件 Bash 不用第一个会话的 registry。 |
| P1 技能 T15 | 扩展 `tomcat/src/api/chat/tests/resource_inventory_test.rs`；【新增】`serve_shared_e2e.rs::new_load_refreshes_skills_without_overlay_leak` | 见 §8.2 | 新技能被看见，目录卡片和 load_skill 同源，私有技能不进别人目录。 |
| P0 流一致 T16 | 【新增】`tomcat/tests/acp_observer_e2e.rs::join_mid_message_then_merge_history`；扩展 `tomcat/src/api/serve/tests/event_pump_test.rs` | 见 §8.2 | 正常接收、未触发背压丢弃时，按 switch→state→history 加入；正文/思考前缀不重不漏；刚结束及未落盘时补全文+结束，下一条开始才替换。工具及 prompt 从历史正确重建；慢读降级另见 T28。 |
| P0 旁观 T17 | 【新增】`tomcat/tests/acp_observer_e2e.rs::switch_session_observes_without_taking_control`；扩展 `tomcat/src/api/serve/tests/commands_test.rs` | 见 §8.2 | ACP 在宿主在线/离线及服务重启后均 observe，来源不被 load 改写；原生 A 控制时 B 为 observe，A 离开后 B 须重开才 control；旧会话头缺 origin 为原生。9 条白名单可用，其余写命令回 read_only_session，含省略目标与优先中断；旁观 close 后停收但任务继续，宿主 load 不被阻挡。开关旁观不多调模型，未加载和已加载的跨项目会话均不得打开；`tomcat/src/api/serve/tests/connection_test.rs::project_observation_*` 覆盖同仓子目录、独立仓库、worktree、无 Git、无项目上下文及显式 ID 读取，旁观前后项目 key、历史和控制者不变。 |
| P1 连接 T18 | 扩展 `tomcat-vscode-ext/src/serveClient/tests/{ServeConnectionSupervisor,messenger_disposal}.test.ts` | 见 §8.2 | dispose 不 kill 服务，关窗取消本窗控制任务，显式停服不被重连拉起。 |
| P0 合流/只读 T19 | 扩展 `tomcat-vscode-ext/src/ui/webview/tests/dual_channel.test.ts`、`tomcat-vscode-ext/gui/src/App.test.tsx` | 见 §8.2 | switch 的 access/origin 经 router/pool 进入视图；get_state 不覆盖身份。observe 时输入/发送/停止置灰、来源及无名兜底正确，control 照旧；关旁观标签不停任务，服务拒绝写命令时显示原失败提示。补发与历史重叠、切回、带缓存重连无重漏，未显示且非本窗控制的会话不串流。 |
| P1 扩展集成 T20 | 【新增】`tomcat-vscode-ext/tests/external_session_observation.test.ts`；扩展 `src/test/suite/host.e2e.test.ts` 的 `shared service observation` | 见 §8.2 | 实际 CLI + 受控模型 + VS Code；同项目中途打开、重载；其它项目窗口不列出也打不开；worktree 必须用它自己的窗口。 |
| P1 Paperclip T21 | 【新增】`paperclip/packages/adapters/tomcat-local/src/server/{execute,test,skills}.test.ts` | 见 §8.2 | 标准 ACP、环境探测、临时技能真正可用、取消及失败；任务工作区 cwd 经共享引擎进入 ACP new/load，不误用服务/转发器启动目录。 |
| P0 Paperclip 恢复 T22 | 【新增】同包 `execute.test.ts` 的 `skills_command_identity_and_resume`；回归共享引擎 `operator-stop.test.ts` | 见 §8.2 | 临时目录是否改变指纹必须实测；区分保存后的续聊与未保存后的新会话。 |
| P1 Multica T23 | 【新增】`multica/server/pkg/agent/tomcat_test.go::TestTomcatGracefulCancelAndResume` | 见 §8.2 | 提前报告 ID、load；`ExecOptions.Cwd` 原样进入 ACP new/load；关 stdin，grace 内不发 kill，超时才杀进程组。 |
| P1 登记 T24 | 扩展 `multica/server/pkg/agent/agent_supported_types_test.go`、execenv runtime_config/skills 表；【新增】Paperclip `NewAgentSetup.tomcat.test.tsx` | 见 §8.2 | 菜单、配置、模型、协议族迁移、任务说明和技能目录齐全。 |
| P0 真实旅程 T25 | 两宿主各一条受控真实执行 E2E，复用 T20 devhost；场景名 `host task → observe → stop → reload/new per host policy` | 见 §8.2 | 停止后 worktree 正常收尾；Multica 按 AGENTS.md 回报状态。 |
| P1 兼容 T26 | 回归 `tomcat/tests/{serve_stdio_e2e,serve_multi_session,serve_ask_question_tests,serve_robustness_tests,session_concurrency_tests}.rs` 及两宿主既有套件 | 见 §8.2 | 旧模式/其它 Agent 不变；注册枚举断言允许加 Tomcat 行，不改原业务预期。 |
| P1 UI T27 | VS Code、Paperclip、Multica：真实页面 PNG + ARIA + console | 见 §8.2 | 看得到、点得对；Tomcat 接入链路的浏览器错误必须处理，宿主无关噪声另记，不能以 DOM 或构建代替。 |
| P0 慢连接 T28 | 【新增】`tomcat/tests/serve_shared_e2e.rs::slow_connection_does_not_cancel_tasks`、`tomcat/tests/acp_stdio_e2e.rs::slow_host_still_receives_the_complete_reply`；回归 `tomcat/src/api/serve/tests/writer_test.rs`，扩展 `tomcat-vscode-ext/src/ui/webview/tests/dual_channel.test.ts` 的 notice 用例 | 见 §8.2 | 原生/ACP 控制连接分别暂停读取并实际压满输出；它不被断开、任务继续，另一连接正常收流。原生恢复后收到提示和后续/终态，关键帧不丢，提示不改 busy、不触发重连；ACP 恢复后收到完整正文和终态，stdout 保持标准协议且不插提示。消息落盘后 get_messages 可读全文；真正断开仍按 T06/T07 取消。 |

### 8.1 执行批次与文档检查边界

M1 先跑新增共享/ACP 目标和最邻近单测，包含 T28 的慢控制连接与恢复读取；M2/M3 再扩前端与适配器。命令以实施时 manifest 为准，计划中已有入口如下：

- Tomcat：`cargo nextest run --test serve_shared_e2e --test acp_stdio_e2e --test acp_observer_e2e`，新增目标登记 nextest/test-groups，再按共享核心改动补已有完整门禁。
- 扩展：`npm run test:unit:core`、`npm run test:unit:gui`、定向 integration、真实 devhost，最终补 lint/build。
- Paperclip：Tomcat 适配器、登记、环境检查、技能/会话续接、取消及受影响 UI 的 Vitest/typecheck/build；复用有效结果，不再补跑无关宿主全量套件。
- Multica：沿 Agent CLI guard 验证 Tomcat backend、注册/探测、说明/技能、迁移、取消/worktree 与受影响 UI；agent 包保持既有并发限制，不以无关宿主测试全绿为门槛。

复用仍有效的同批结果，不为每个 todo 重跑全套。功能验收使用隔离 HOME、受控模型、测试数据库；真实付费调用需明确授权。文档检查只证明规范骨架、链接、计划范围及 diff 的一致性，不代替功能或平台验收。

### 8.2 本轮验收结论与边界

本节保留**上一轮接入阶段**的验收记录，不是本次复核整改的全绿声明。代理、ACP 输出、版本策略、提问能力等新增回归及三平台门禁，以当前整改计划的执行证据为准；旧结果只在仍有效时复用，未完成项不计通过。

```text
Tomcat / VS Code：完整检查已执行，复用仍有效的通过结果 + 修复项所属模块复验
两个宿主：只要求 Tomcat 接入及受影响链路通过
Windows：必做项迁到 GitHub Windows CI；个人 PC 可选，本机不代签
```

用户已明确收窄两个宿主的范围：无关既有测试失败保留记录，不再调查、修复或阻塞交付。该口径取代原计划“两个宿主既有测试全部通过”的要求，不缩减共享服务、权限、项目隔离和真实停止的保证。

| 范围 | 本机证据与结论 |
| --- | --- |
| Tomcat 后端 | 最后完整库运行 3412 PASS / 1 FAIL / 5 SKIP；唯一失败为中断测试的定时竞态，基线故障注入同样复现。仅修测试夹具后，完整所属模块 15/15 与调度暂停实验 8/8 通过；复用其余有效结果，不把原批次 exit 100 改写为全绿。严格 Clippy、格式、schema/catalog 复验通过。 |
| VS Code | 完整 core 798、GUI 864、集成 236（另 1 SKIP）通过；真实共享服务中途旁观、重载、只读、异项目拒绝及 worktree 独立窗口旅程通过。 |
| Paperclip | Tomcat 适配器与环境检查、登记/配置 UI、技能与 ACP 会话续接通过；恢复相关三文件 492 条通过；真实 Stop、宿主重启、原会话继续和同项目旁观通过。全仓 build 通过；原全量测试失败与主动停止的无关补验不计为通过。 |
| Multica | 全部 Tomcat backend 测试（含 race/真实服务集成）、daemon 探测、execenv/技能与迁移包通过；运行时配置保存/重载、worktree 停止删除、daemon 重启后原会话继续及状态回报通过。无关 provider/任务领取失败仅留档。 |
| 权限与界面 | 默认 true、保存 false、批准/拒绝/取消/断开与迟到回复由真实 ACP 权限门覆盖。宿主 false 实测为 Paperclip deny-all 拒写、Multica allow_once 自动批准，不宣称人工弹窗。三端 PNG/ARIA/console 已捕获并读取；宿主可选文档/日志 404 和更新检查网络错误保留，不宣称控制台全程零错误。 |
| 平台与发布 | 当前证据为 macOS；Windows 系统代理/管道/Job/真实后端与窗口生命周期待云端实际验证，Paperclip POSIX shell 限制见 §6.1。条件提交/推送授权见 §6.3；此历史批次未提交、推送或发布。 |

## 9. 风险与应对

风险不是额外需求；这里记录当前边界下仍需验证或不能保证的部分。**说人话：**把可能失手的地方和处理动作提前写清，不靠“应该没事”收尾。

| 风险 | 影响 | 应对（具体动作） | 说人话 |
| --- | --- | --- | --- |
| 模型看到项目 A，工具却使用服务目录 | 读错文件或在错误位置执行命令；只检查会话 cwd 无法发现 | 按 §3.3 将已绑定的会话目录贯穿提示词与工具执行，补齐现有相对路径/Bash 默认目录接缝；T12 同时检查模型请求与实际结果，禁止全局 chdir，保留权限门 | 告诉模型地址，不等于工具已走到那里。 |
| 来源与访问方式混淆 | 宿主离线后误变可写，或原生窗口抢走控制权 | origin 只在头里保存，access 由 switch 回复给窗口；clientName/role/busy 均不参与授权。T06/T17/T19 覆盖断线、重开、老文件及 get_state 刷新；控制绑定清空不绕过停止等待 | 来源是历史事实，控制者是当前状态，窗口拿到的是判断结果。 |
| 共享内存绕过原项目边界 | 另一个项目的会话被打开、读历史或收到流 | 列表保留 disk；共享会话入口复用现有 session key 核对，旁观不改本连接 scope。T17/T20 覆盖热/冷载入及同仓子目录/worktree/无 Git；不加第二套项目注册表 | 服务共用，窗口的项目边界不变。 |
| 强杀 relay / drain 超时后宿主立即清理目录 | 工具取消与 worktree 收尾竞态 | 60s 总宽限内真实 stop/wait；超时非零、保留停止中、日志带任务 ID；T08 注入延迟和持续写文件。若宿主把失败仍当安全收尾，M1 上报边界，不宣称绝对零写入 | 正常退出能等停手，强杀不能保证这个顺序。 |
| 共享 usage 锁不能跨独立进程排斥执行 | 旧 chat/stdio 可并行打开同一历史 | 不升级锁；文档说明不要同时用旧入口执行共享服务正控制的 session。旁观已加载 slot 不重复 hydrate；T26 保留原并发/删除保护 | 本期解决一个服务内的控制权，不改整台机器的执行锁。 |
| 项目 MCP 首个创建者环境复用 | 同 cwd 多连接的项目 MCP 不完全隔离 | 明示例外，T13 锁定行为；需要不同凭据时使用会话级 MCP。出现实际冲突再评估缓存键，不能先宣称全隔离 | 共享工具和任务私有工具要分开说。 |
| 插件 dispatcher / 子代理工厂仍捕获旧环境 | 主 Bash 正确但旁路串环境 | 审计 §5 全部构造点，T12/T14 覆盖两连接同目录；不把单个 env_clear 当整体证据 | 不只改最显眼的那一条路。 |
| Windows shell/PATHEXT/Job 差异 | 路径解析仍受服务环境影响或无法脱离 | 解析程序时使用任务的 PATH/PATHEXT；Windows + Multica 的 Job 不许脱离，须先在独立终端执行 `tomcat serve start` 或打开 VS Code；按 §6.1 测允许/禁止脱离两条路径，不绕过宿主策略 | Windows 不能只拿 Unix 测试代替。 |
| 补发与历史/旧缓存重叠 | 重字、消息假运行或工具状态倒退 | §3.2.4 在开始转发处先补发，复用原历史合并；T16/T19 暂停在消息结束/落盘交界，并覆盖已有部分缓存再接收。源码有按 ID 合并不等于已通过这些时序，实测前不承诺 | 刚说完和第二次进来，也得看得正确。 |
| Paperclip 临时 skills 路径进入 fingerprint | 续聊被判新配置、新会话 | 包内测试命令指纹与 session codec；保留宿主既有 fresh 行为并明确提示；若要求稳定续聊且现有扩展点做不到，实施前报告，不修改公共恢复引擎绕过 | “技能能用”和“还能续聊”要一起测。 |
| shell-only key 用户失去凭据 fallback | 共享服务不能调用模型 | 复用缺 key 引导迁入保存凭据，检查删除 key 后不复活；旧 stdio/chat 保留原行为 | 这是明确兼容变化，不是神秘连接失败。 |
| AGENTS.md 深目录累计过大 / 路径竞态 | 上下文压力或意外内容进入提示 | 单文件有界、目标权限/身份校验、现有摘要诊断；T02 覆盖链接替换/非法内容；累计超出小模型能力再修订总量策略 | 不截半份手册，也不承诺所有大仓库零成本。 |
| 协议/schema 变更及版本错配 | 老扩展误用共享接口 | Rust 类型单源生成，握手版本不合即拒绝；保留旧 stdio；回滚前明确停共享服务，不自动转私有执行器 | 不能连不上就偷偷换架构。 |
| 慢读导致直播缺片或关键帧积压 | 画面不完整；无限停读时内存不能承诺有界 | 沿 §3.2.2 每连接复用 writer，原生合并/丢消息增量并提示，ACP 只合并、不丢正文，均不按慢读断开；T28 验证任务继续、连接隔离、恢复读取与落盘历史。仅当真实持续积压暴露资源问题时再修订策略，不提前加踢连接或重放机制 | 卡画面不等于停任务，也不把降级说成无损。 |
| handler panic、真实 I/O 失败、资源泄漏 | 单连接影响全服务 | 捕获任务 JoinError 和实际写出错误，交连接层执行统一清理；满槽位回收排除后台工作；T05/T06/T11/T17 故障注入；不得把慢读压力当 I/O 失败 | 断的是坏掉的连接，不是暂时读得慢的连接。 |
| Unix socket 残留 / Windows 命名管道抢占 | 连错实例或误删文件 | 锁内复查、验证目标类型/用户权限，拒绝非 socket/异常 reparse；握手实例/版本核对 | 不把任何同名文件都当垃圾删掉。 |

## 10. 历史决策、跨文档关系与后续规划

本节保留被取代方向，防止实施时把旧机制带回来。当前代码仍以现有 stdio 文档为基线，本文不是已上线声明。

- ~~宿主断开后任务继续、requestId 登记与接回原轮~~ → 否：本期接受取消，只做适配器；不实现私有恢复扩展。
- ~~Paperclip native runner ACPX profile~~ → 否：是为活动轮恢复选的；本期走普通适配器。
- ~~TCP 端口、发现文件、令牌、全局事件序号~~ → 否：本机 IPC + OS 权限、在开始转发处补当前消息足够当前需求。
- ~~独立 ObserverClient、逐进程旁观端口、跨进程独占锁升级~~ → 否：共享 Messenger 和原锁足够本期约定范围；不把遗留边界说成新安全保证。
- ~~整轮快照、`observe_session` / `unobserve_session`、`currentTurn` 响应与 `serve/observation/`~~ → 本轮删除：复用 `switch_session → get_state → get_messages`，只在 `event_pump.rs` 补发当前消息。
- ~~旁观专用只读历史查询、跨项目合并 Live 列表或打开历史~~ → 删除：保留原项目磁盘列表和打开流程，宿主会话卸载后仍可在同项目窗口只读打开；不另做跨项目发现或隔离系统。
- ~~新建 live 基线合流协议、用户消息广播、整轮工具/等待投影~~ → 本轮删除：沿现有历史与事件合并，补发的时序保证由转发处负责，缺口通过原路径的回归测试定位。
- ~~get_state 返回 readOnly、字符串 origin、GUI/Host/服务三层重复只读判断~~ → 本轮替换：switch_session 返回 access 与来源对象；界面置灰、服务统一白名单，Host 只传结果和展示错误。
- ~~无控制者就自动把旁观窗口变成控制者~~ → 否：原生会话重新打开才重算，ACP 来源在 VS Code 始终只读；停止完成之前不得接手。
- ~~慢队列溢出就断连，再重连补发~~ → 本轮删除：沿用每连接 writer；原生合并/丢增量/提示，ACP 合并但不丢正文；慢读不断开，避免把画面卡顿变成控制任务取消。真实 EOF/写失败的断连语义不变。
- ~~为传项目说明而反复拼接 Multica AGENTS.md 到 prompt~~ → 否：AGENTS.md 进入现有环境规则管线，每轮刷新但不写对话；这不删除 Paperclip 既有的任务 prompt 和变量使用说明，后者见 §3.4。
- ~~本机 `mode` / `env` 握手和短管理连接 `drain_connection`~~ → §4.1 按当前代码替换为 `ConnectionHello.protocol` / `environment`，以及原本机连接内的私有 close/确认记录；不改变宿主侧标准 ACP。

### 10.1 跨文档修订登记

| 相邻文档 | 本方案实施后需同步的语义 | 说人话 |
| --- | --- | --- |
| [agent-server-and-ui-gateway](agent-server-and-ui-gateway.md) | 新增共享 IPC/ACP 模式；每连接复用 writer 背压、不因慢读断开；旧 stdio 仍保留，不把尚未完成方案写成现状 | 区分原生的增量降级与 ACP 的完整结果。 |
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
