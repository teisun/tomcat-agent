# Tomcat VSCode 扩展 · 04 运行时参考（配置 / 错误 / 测试 / 风险 / 历史）

> 总览见 [`../tomcat-vscode-extension.md`](../tomcat-vscode-extension.md)（含定位、阅读顺序与文首导图集）。
> 本文对应 [`ARCHITECTURE_SPEC.md`](../../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) 的 **§6 配置**、**§7 错误模型**、**§8 测试矩阵**、**§9 风险**、**§10 历史决策**。
> 协议字段见 [`03-protocol-and-file-map.md`](03-protocol-and-file-map.md) §4；决策表 R1–R10 见 [`02-implementation-details.md`](02-implementation-details.md) §3.1。

---

## 6. 配置与环境变量

> 专业：扩展侧配置经 `package.json::contributes.configuration` 暴露；Tomcat serve 参数走 Tomcat 自身配置（`[serve]` 子表，`tomcat/src/infra/config/types/runtime.rs::ServeConfig`），扩展只透传/影响进程启动。总则：env > 扩展设置 > 默认。
> 说人话：扩展自己有几项设置（tomcat 在哪、起几个会话），serve 的细节参数由 Tomcat 配置文件管，扩展尽量不重复造。

| 变量 | 取值 | 含义 | 优先级 | 说人话 |
|------|------|------|--------|--------|
| `tomcat.path`（扩展设置） | string | `tomcat` 可执行文件路径 | 扩展设置 | 告诉扩展去哪找 tomcat。 |
| `tomcat.serve.extraArgs`（扩展设置） | string[] | 透传给 `tomcat serve` 的附加参数 | 扩展设置 | 给后端启动加料。 |
| `tomcat.session.defaultCwd`（扩展设置） | string | 新会话默认工作目录 | 扩展设置 < `new_session.params.cwd` | 新对话默认在哪干活。 |
| `TOMCAT_SESSION_MODE`（env） | `code`/`claw` | 会话默认模式 | env（最高，见 `serve/mod.rs::default_mode`） | 环境变量直接钉死模式。 |
| `[serve] transport`（Tomcat 配置） | `stdio`/`ws` | 传输；本扩展固定 `--stdio` 覆盖 | 命令行 `--stdio` > config | 我们只用 stdio。 |
| `[serve] max_sessions`（Tomcat 配置） | usize | 并发会话上限（默认=`MAX_CONCURRENT_AGENTS`） | config | 最多同时开几个对话。 |
| `[serve] delta_coalesce_ms`（Tomcat 配置） | u32（默认 25） | 相邻 delta 合并窗口 | config | 多少毫秒内的增量合并成一帧。 |
| `[serve] max_buffered_frames`（Tomcat 配置） | usize（默认 64） | 背压缓冲上限，超限丢 delta | config | 缓冲满了就丢中间字。 |
| `[serve] schema_out_dir`（Tomcat 配置） | path | `--print-schema` 输出目录（默认 `<agent_trail_dir>/serve-schema`） | config | 生成的类型放哪。 |

> 构建期生成 `wire.d.ts`：`scripts/gen-wire.ts` 调 `tomcat serve --print-schema`（打印输出目录），读取其中 `serve.d.ts` 拷为 `src/serveClient/wire.d.ts`。CI 校验"生成物与提交一致"，防漂移。

---

## 7. 错误模型 / 截断 / 降级

> 专业：列出扩展侧所有归一化结局：哪些是协议级 `error` 码、哪些是事件级降级、哪些进程级失败。避免静默吞错。
> 说人话：把"会出什么岔子、各自怎么收场"摊开说清。

```text
未握手即发命令      → ResponseFrame{success:false, error:"not_initialized"}（扩展：先 initialize 再重发）
未知/已关闭会话     → ResponseFrame{success:false, error:"unknown_session"}（扩展：清理本地 sessionId 映射 + 提示）
未知控制子类型      → ResponseFrame{success:false, error:"unknown_command: control_request/<subtype>"}
非法 NDJSON 行      → serve 回 ResponseFrame{success:false,error:<parse msg>}；扩展：丢弃并记日志（不崩）
背压（慢消费者）    → 事件 llm_notice{finishReason:"backpressure"}；serve 丢 message_update（仅 delta，生命周期/控制帧必达）
模型终局错误        → 事件 llm_error{reason,errorCode?,errorMessage}（扩展：渲染错误气泡，turn 收尾）
用户中断            → 命令 interrupt → 事件 agent_interrupted + agent_end（扩展：标记已中断）
ask_question 超时/取消 → 扩展发 control_cancel → serve 兜底 AskQuestionResult{cancelled:true}（turn 不卡死）
子进程崩溃 / stdout EOF → child 'exit' 事件（扩展：会话置 failed + 「重启 serve」入口；serve 侧 EOF→级联取消所有会话）
serve 可执行缺失/启动失败 → spawn error（扩展：引导用户配置 tomcat.path）
```

> 说人话：核心原则两条——(1) **协议错误用 `error` 码归一化**，扩展按码处理而非猜；(2) **降级只丢中间字，绝不丢"结束/审批/错误"这些关键帧**，所以即使 UI 卡顿，turn 也总能正确收尾。

---

## 8. 测试矩阵（验收）

> 专业：每条 §3 可观察交付映射到锁死它的测试。扩展侧测试分四层：`TS 单元`、`spawn 真实 serve 的集成`、`真实宿主 E2E`、`文档/流程验收`；协议侧行为仍以 Tomcat 自带测试为事实源回指。当前条目已按实际实现状态更新，便于后续分支验收与回归复用。
> 说人话：这些测试现在不是“将来要补”，而是已经落进仓库并在本分支跑过，可以直接拿来回归。

| 维度 | 用例 / 编号 | 状态 | 说人话 |
|------|-------------|------|--------|
| 单元（桥接分帧） | `src/serveClient/tests/ndjson_framing.test.ts`：半行、粘包、多帧一批次、空行跳过、非法 JSON 恢复 | ✅ | 锁住“一行一帧”的最底层读管道。 |
| 单元（桥接协议） | `src/serveClient/tests/messenger_request_response.test.ts`：`id -> Promise`、未知 `id` 丢弃、超时清理 | ✅ | 锁住命令回执，不让请求表越跑越乱。 |
| 单元（控制回环） | `src/serveClient/tests/messenger_control_roundtrip.test.ts`：`initialize`、`ask_question`、`control_cancel`、能力位校验 | ✅ | 锁住审批/提问这条最容易出错的带外通道。 |
| 单元（渲染映射） | `src/ui/participant/tests/render_message_stream.test.ts`：`message_update`/`tool_execution_*`/`agent_end` → `ChatResponseStream` 调用序列 | ✅ | 锁住事件怎么被画进原生聊天框。 |
| 单元（审批 UI） | `src/ui/participant/tests/ask_question_commands.test.ts`：问题渲染、推荐按钮、QuickPick fallback、答复 payload 组装 | ✅ | 锁住按钮点下去以后回给 serve 的内容。 |
| 单元（编辑落地） | `src/ide/tests/diff_apply_edit.test.ts`：虚拟只读文档、`vscode.diff` 参数、`WorkspaceEdit` 应用 | ✅ | 锁住“看 diff”和“真正改文件”是同一份改动。 |
| 集成（spawn happy path） | `tests/serve_e2e.test.ts`：spawn 真实 `tomcat serve --stdio`，跑 `initialize -> prompt -> message_update -> agent_end` | ✅ | 真起子进程跑通最短链路。 |
| 集成（ask_question） | `tests/serve_ask_question_integration.test.ts`：真实 `control_request{subtype:\"ask_question\"}` → UI 答复 → `control_response` 回环 | ✅ | 审批不只单测过，还要和真实 serve 串起来。 |
| 集成（多会话/生命周期） | `tests/session_lifecycle.test.ts`：双会话不串台、kill→restart、`interrupt` 生效 | ✅ | 锁住多开、中断和崩溃恢复。 |
| 集成（背压/降级） | `tests/backpressure_notice.test.ts`：`llm_notice{finishReason:\"backpressure\"}` 提示与生命周期帧不丢 | ✅ | UI 再慢也不能把“结束/错误/审批”吃掉。 |
| 关键承诺（R3 防漂移） | `npm run check:wire` / `gen-wire.ts`：产物与 `tomcat serve --print-schema` 生成的 `serve.d.ts` 一致性校验 | ✅ | 协议一变，生成校验就红，不能靠肉眼追。 |
| 协议事实源（Tomcat 侧，回指） | `tomcat` `src/api/serve/control.rs::tests`（握手/not_initialized/interrupt/unknown_command）、`writer.rs::tests`（轮转/合并/背压 notice 一次）、`ask_question.rs::tests`（control 回环/按会话路由）、`schema.rs::tests`（serve_dts 命名）、`tests/serve_stdio_e2e.rs` | ✅（Tomcat 现有） | 协议行为已被 Tomcat 自己测死，扩展直接信。 |
| E2E（真实宿主 / VSCode Dev Host） | `E2E-VSCEXT-001`：真实 VSCode Dev Host 下执行一问一答、审批、diff、interrupt | ✅ | 不只测 test host，要在真的 VSCode 宿主里走主链路。 |
| E2E（真实安装 / VSCode 本机） | `E2E-VSCEXT-002`：本地打包并安装扩展到 VSCode，再跑同一组宿主断言 | ✅ | 真装一遍，避免“开发宿主能跑、安装版翻车”。 |
| E2E（多会话真实交互） | `E2E-VSCEXT-004`：真实宿主里验证聊天线程到 `sessionId` 的稳定映射 | ✅ | 多会话不能只在集成里测，真实宿主也要看路由。 |
| E2E（恢复/中断真实交互） | `E2E-VSCEXT-005`：真实宿主里验证 interrupt + restart 后继续可用 | ✅ | 真正站在用户角度验“挂了还能不能接着用”。 |
| 文档 / 流程 | 本组架构文档、`docs/status/feature-tomcat-vscode-extension.md`、任务卡/看板状态与实际实现一致 | ✅ | 字、测试、流程三本账要对得上。 |

补充口径：

1. **VSCode 是“真实安装”强门禁**：因为本机已装 VSCode，且本任务产物本质是 VSCode 扩展，`E2E-VSCEXT-001/002` 视为本期强制验收项。
2. **Cline / Continue 参考不只体现在实现，也体现在验收**：`Cline` 侧重点对齐 diff / 审批 / 原生宿主交互，`Continue` 侧重点对齐 NDJSON bridge、spawn 子进程与 IDE 宿主联调。

---

## 9. 风险与应对

> 专业：覆盖兼容性 / 进程 / schema / 并发 / 上架合规 / 资源泄漏。应对落到具体动作。
> 说人话：最可能翻车的点 + 具体怎么防。

| 风险 | 影响 | 应对（具体动作） | 说人话 |
|------|------|--------------------|--------|
| 误用 proposed API 导致不可上架 | 高 | `package.json` 严禁 `enabledApiProposals`；CI lint 扫 `vscode.proposed.*` 引用即失败；§2.3 已给全部稳定替代 | 一碰 proposed 就 CI 拦下。 |
| Tomcat wire schema 漂移 | 高 | 类型由 `--print-schema` 生成；CI 校验生成物一致；TS 编译期暴露字段变更 | 协议变了编译就报错。 |
| 原生 participant 能力不足以表达富交互 | 中 | Phase1 用稳定 part 拼装（progress+markdown+button+filetree）；不够再上 Phase2 webview（桥接核心复用，无返工） | 原生不够看就换自画 UI，底层不重写。 |
| serve 子进程崩溃/僵死 | 中 | child 'exit' 监听→会话 failed + 「重启 serve」；可选心跳（定时 `get_state` 超时判活） | 后端挂了能感知能重启。 |
| stdout 背压致 UI 卡顿/乱序 | 中 | 不在扩展侧二次缓冲重排；快读入队异步渲染；尊重 serve 单写者轮转；读 `llm_notice{backpressure}` 仅提示 | 快读快画，丢字交给后端策略。 |
| 多会话状态串台 | 中 | 所有命令带 `sessionId`；本地 `Map<chatThread, sessionId>` 唯一映射；事件按 `sessionId` 过滤分发 | 每条消息都贴会话标签。 |
| 进程/监听器泄漏 | 中 | `deactivate` 关闭 stdin（触发 serve EOF 级联取消）+ kill child + 注销所有 disposable/event listener | 关扩展时把进程和监听都收干净。 |
| 大附件/大结果阻塞管道 | 低 | 附件走 `fileId`/base64 控制大小；大工具结果由 serve 侧已截断/落盘（`tool_result_truncated`/`tool_result_persisted` 事件） | 大块头交给后端截断落盘。 |
| VSCode 版本兼容 | 低 | `engines.vscode` 取 Cline/Continue 同档稳定基线（如 `^1.84.0`）；仅用稳定 d.ts 符号 | 基线对齐已上架扩展。 |

---

## 10. 历史决策 / 跨文档修订

被取代/否决的方案（留痕）：

- ~~形态 B：把 Tomcat 注册为 `LanguageModelChat` 模型供 Copilot 调用~~ → 否：会绕过 Tomcat 自身 agent loop/工具/权限/多会话，且 `lm.registerChatModelProvider`（`chatProvider`）为 proposed、第三方不可上架。
- ~~形态 C：fork `vscode-copilot-chat` / Copilot，删模块后复用其 agent/编辑 UI~~ → 否：体量大、涉许可与商标；其招牌 UI（agent 模式/编辑 diff/thinking/审批卡）由 VSCode core + proposed API 驱动并受**扩展身份门禁**（`extensionsProposedApi.ts`：仅 `isBuiltin` 或 `product.json` 名单放行），fork 出的第三方扩展仍拿不到权限——copy 代码 ≠ 拿到能力。
- ~~自造新 IPC 协议（JSON-RPC/gRPC over stdio）~~ → 否：`tomcat serve` 的 NDJSON wire 已实现握手/多会话/审批桥/背压/schema 导出，复用即可，自造徒增漂移面。
- ~~`--ws`（WebSocket 传输）~~ → 暂否：Tomcat 侧 `serve/mod.rs` 明确 `serve transport ws is deferred to Phase 2`，本方案固定 `--stdio`。

跨文档修订：本文为新增文档，不改写相邻方案语义；与 Tomcat [`agent-server-and-ui-gateway.md`](../../../../tomcat/docs/architecture/agent-server-and-ui-gateway.md) 为"服务端能力 ↔ 客户端接入"互补关系，后者任何 wire 变更应回链本组文档 §4 更新生成物。
