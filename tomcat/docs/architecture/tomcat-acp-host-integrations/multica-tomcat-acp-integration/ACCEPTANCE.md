# Multica 接入验收清单

## A. 代码与迁移

- [ ] 公司基线、Go/Node/pnpm 版本和当前 migration ledger 已记录。
- [ ] `server/pkg/agent/tomcat.go` 未覆盖公司已有 backend 定制。
- [ ] agent factory、daemon 探测、execenv、launch 参数过滤、模型和版本登记已合并。
- [ ] `AGENTS.md`、`.agents/skills` sidecar 和 worktree 清理链路仍完整。
- [ ] `Session.CleanupRequired`、daemon handoff、`cleanup_unconfirmed` 分类与取消回报已语义合并；未确认时保留 worktree/sidecar，不自动提交或删除。
- [ ] Tomcat 迁移编号唯一，up/down/provider 白名单和迁移测试已按公司前态调整。
- [ ] 不使用全局 migrate-down 作为单项回退。

## B. 执行账号

- [ ] daemon 实际运行账号能找到包含 ACP 的 Tomcat 二进制。
- [ ] `tomcat acp --help` 包含 `--skills-dir`。
- [ ] 实际入口的 help 支持所配置四项启动策略；旧入口参数早退显示实际路径和 stderr，不被误判为清理未确认。
- [ ] daemon 服务环境拥有正确 HOME、Tomcat 数据目录、模型配置和 Key。
- [ ] `tomcat serve status`、模型设置和代理配置在同一执行账号下检查。
- [ ] 同一后端仅桌面或一个 CLI profile 执行 Tomcat；已检查运行 daemon 继承的实际路径，而非仅检查当前 shell。
- [ ] 没有把 `HOME/PATH/TMP/MULTICA_*` 通过 agent custom_env 注入。
- [ ] 模型目录使用实际runtime的 `serve models`，不创建临时ACP会话或调用付费模型；缺凭据模型不展示。
- [ ] 零模型/旧命令/坏配置/超时经原failed/error到创建页原因提示；默认/手填路径仍可用，无新增toast或页面生产改动。
- [ ] 保存/删除凭据后显式刷新可见最新目录；保存的是原始ID，ACP执行使用对应model_name。
- [ ] 已读取桌面/窄屏PNG、ARIA及console；目录成功没有被当作网关/额度健康证明。

## C. 受控 Multica 旅程

| 场景 | 必须看到 |
| --- | --- |
| runtime profile | `runtime_type=tomcat`，command 为 `tomcat`，fixed args 不含 `acp`。 |
| 新任务 | local_directory cwd 同时进入 ACP session 和真实子进程。 |
| AGENTS/skills | 模型能看到任务规则；worktree 结束后 sidecar 和目录按既有机制清理。 |
| 取消 | 关闭 forwarder stdin，实际进程退出；超时强杀明确记录，不伪称共享服务已确认停止。 |
| 清理未确认 | 超过 handoff、关闭 Result 无值、清理超时或异常退出均保留清理错误与目录；不因用户取消吞掉诊断，不自动 fresh 重试。 |
| 参数解析早退 | 仅内建 runtime、空 stdout、exit 2、未超时适用普通失败例外；三个保守反例仍为清理未确认，恢复引用不丢失。 |
| 启动策略/人工等待 | 四参数独立透传；推荐答案不是批准，保存不是开工；真实待答卡片取消后撤回，同会话可继续。 |
| 继续 | 兼容时走 `session/load`；worktree 已销毁时按宿主规则 fresh 或明确失败。 |
| 结果 | 完成、取消、恢复拒绝、usage partial/unpriced 状态准确。 |
| Windows | 预启动共享服务后再派任务；不能依赖受管 Job 内临时拉起服务。 |

## C.1 共享输入与网络接回（目标部署另验）

以下为使用支持 `shared_session_input` 的新版 Tomcat 后的部署检查，不表示本次已运行 Multica：

- [ ] 同项目 VS Code 可以持续收流、发待发消息、立即发送插话、停止并准备附件；改模型/模式仍须等当前轮次结束。
- [ ] 空闲写入接手不改 sessionId/planId/计划文件；计划引用可原地修订，不自动开工；下一 Multica 任务 load 同一会话。
- [ ] ACP 网络中断的 error.message 原样保留，data 命中当前 Multica `provider_network` 分类；429/5xx/额度/鉴权没有网络诊断。
- [ ] 在实际 daemon/task 配置下确认允许重试的任务会重派、保留会话引用，且结果/issue 评论/通知符合宿主策略；受控 ACP peer 不能代替此项。
- [ ] 已向用户说明 README §6.1 的七条限制：插话原文、Stop→aborted、后续轮次不回流、忙时 load 失败、删除后 fresh 且不自动关联旧计划、重试策略/五分钟竞争、CLI 登录身份。
- [ ] 未运行的外部宿主 DevHost 旅程与 Windows 部署验收单独记录，不以 Tomcat/macOS 的模拟重派结果代签。

## C.2 Tomcat 侧共享输入实现验证

2026-11-14，按 `plan_acp_vs_code_multica_acp_host_composer_d91ea310` 完成 Tomcat 侧实现与验收。只修改本仓库的 Rust 服务/ACP 和 VS Code 扩展；没有修改或运行真实 Multica，没有安装产品或重启用户共享服务。以下结果不能代签 C.1 的目标部署检查。

```text
真实 ACP 进程运行 → VS Code 持续刷新、排队/立即插话、停止真实 shell
  → ACP 创建计划 → VS Code 引用原文和发送修改要求 → 原计划原地更新
  → sessionId / planId / 文件不变，仍是规划模式

另有真实进程回归：ACP → Native → 新 ACP；网络失败 → 新 ACP load 原会话
```

### 检查与结果

按影响范围分批覆盖 Rust `gate-fast` 和扩展 `gate:fast`，复用仍有效的结果；另跑受影响的扩展集成和真实 DevHost。下面如实保留首次失败和针对性复验，不声称两个非零整批任务后来重新全量通过。

| 检查 | 结果 | 本地任务日志 ID |
| --- | --- | --- |
| Rust 准入、计划身份、迟到取消、schema、ACP 失败映射聚焦用例 | 17 通过 | `1794588734423-u73zfi` |
| Rust 全库单测 | 3539 通过、1 失败、7 ignored；唯一失败是搜索测试的子进程未获得 `rg` 路径。使用计划指定 PATH 后该项单独复验 1 通过，未重跑整个库 | 原批 `1794589753422-eoqnyi`；复验 `1794591772428-i2il2s` |
| Rust 并行集成组 | 398 通过、1 失败、28 skipped；失败为本次 wire 变更漏更新 schema fixture。用已有生成器更新两个 fixture 后，原用例复验 1 通过 | 原批 `1794591772428-i2il2s`；复验 `1794592620625-qku2hd` |
| Rust 串行集成组 | 94 通过、3 skipped；包含真实 ACP 插话/停止、接力、网络失败接回及鉴权负例 | `1794590436954-e28hri`、`1794591772428-i2il2s` |
| Rust 严格静态检查 / 文档测试 | `cargo clippy --all-targets -- -D warnings` 通过；`cargo test --doc` 成功退出（0 个用例） | `1794590436954-e28hri`；`1794592620625-qku2hd` |
| 扩展 core 单测 | 3 文件 193 通过；其余 55 文件 758 通过，合计 951 | `1794588033043-ilpiyd`、`1794589753346-m5maky` |
| GUI 全部单测 | 83 文件 908 通过，包含修正后的旧版只读编辑夹具 | `1794589753346-m5maky` |
| 扩展集成 | `external_session_observation`、`serve_plan_review_decision` 两文件 6 通过；同一 shell 后续的早期 DevHost 失败不抹去此前结果 | `1794589565525-m16d6s` |
| 类型检查、wire 与构建 | 扩展/GUI lint、生成与 check:wire 通过；最终 DevHost 重新完成 GUI build 与 extension compile；生成的扩展 wire 与 Rust fixture 逐字相同 | `1794588989653-6n67e4`、`1794588402372-u7kdf4`、`1794592962917-2a6abh` |
| 真实 VS Code DevHost | 1 通过（18 秒），exit 0；附加断言确认选中原文和修改要求实际进入模型请求，而不只停在引用 chip | `1794592962917-2a6abh` |

日志位于执行机器的 `~/.tomcat/agents/main/tool-results/bash-<任务 ID>.log`。早期失败已处理：测试参数/字段和旧只读夹具按真实契约修正；DevHost 恢复宽窗口后再打开计划，并等待实际预览 webview 就绪，未删除控件边界/同计划断言；新增请求断言从捕获的 HTTP 报文中解析 JSON body，而非把整段报文当 JSON。未把测试操作修正描述为产品布局修复。

验收还发现 OpenAI 流内 provider error 被忽略、到 EOF 后误归网络中断的问题：在解析源头复用现有 `LlmError` / `FinishReason` 事件修复，首帧与部分输出后的分类回归已在全库单测通过，真实 ACP 鉴权负例在串行组通过。未削弱“429/5xx/额度/鉴权不附网络诊断”的要求。最后一次 `ownership.rs` 调整仅纠正旁观命令注释，没有额外改变准入。

### 最终视觉与身份记录

最终目录：`.agents/acceptance/shared-input/gui-5HTLdG/`。`result.json` 为 passed / exitCode 0，`observer-journey.json` 记录 10 次脚本化模型调用及下列身份：

- `sessionId = 1794592997006_ec5084c4aaa03070`
- `planId = plan_shared_revision_fixture_26f35287`
- 计划文件是该隔离 HOME 中的 `plan_shared_revision_fixture_26f35287.plan.md`；完整临时路径保存在旅程 JSON 中，测试始终编辑这一份文件。

已读取下面 **8 组** PNG、ARIA 和 console。工作台为 1440×900 / 390×844，实际聊天面板为 1381×794 / 331×738；计划预览为 786×780。提示正常折行，待发项、引用 chip、发送/停止与提问控件可见且不互相遮挡；运行中设置禁用、空闲后可用。八份 console 在采集范围内 errors/events 均为空，不将其说成全进程零警告。

1. `observer-desktop.png`：桌面待发与他端运行提示。
2. `observer-queued-narrow.png`：窄屏待发、立即发送和停止。
3. `observer-narrow.png`：实际工具运行、插话仍在当前轮内。
4. `observer-question-desktop.png`：桌面非宿主提问卡片。
5. `observer-question-narrow.png`：窄屏提问卡片。
6. `shared-plan-quote-desktop.png`：桌面原计划引用与修改要求。
7. `shared-plan-quote-narrow.png`：窄屏引用、修改要求和发送。
8. `shared-plan-revised.png`：原计划更新为 “Test before implementation.”，仍显示待执行条目和 Build。

### 审查与未运行边界

- 对照 T1–T16 复核 diff：复用事件多播、接手流程、静止检查、插话和五分钟恢复；未引入服务端队列、轮次编号/受理状态、订阅层、重试计时器、新配置或 Reviewer 入口。删除仍独立检查，不为删除接手；跨项目与审批边界继续由原有流程约束。
- 未运行真实 Multica daemon/task/issue 评论与通知全链路、`TOMCAT_E2E_EXTERNAL_OBSERVATION` 外部宿主旅程、Windows 部署或付费模型测试。网络接回测试是受控 ACP peer，不证明真实 Multica 的任务策略配置。
- Multica 轮次内继续接收 Agent 回复；插话用户原文及 VS Code 后续新轮不承诺显示在其时间线。VS Code 重试成功不会把原 Multica 失败任务改成功。其余边界见 README §6.1。

## C.3 追加：显示当前宿主名称并缩短提示

2026-11-14，按用户追加要求，将运行提示改为“正在 {host} 中运行” / `Running in {host}`，删除后半句操作说明；未知名称时使用简短通用提示，空闲隐藏。Multica 当前上报 `multica-agent-sdk`，直接使用，不做品牌映射；Native 连接尚未上报名时不猜作 VS Code。

```text
当前活控制连接的名称 → 打开回复 / session_access → 既有会话视图 → 简短运行提示
宿主切换或断开       → 同一通知更新 / 清空名称（旁观者身份不变也刷新）
```

`controllerName` 为可选显示字段，来自当前连接而非持久化 `origin`。复用既有事件、接手点和断开流程；没有新增查询、订阅、落盘状态或配置。覆盖第三个旁观窗口、当前宿主与创建来源不同、Native 接手清空旧名、断开清空、先通知后新轮事件、跨项目不泄漏，以及旧服务缺字段回退。原准入、插话、停止和计划身份契约不变。

| 追加检查 | 结果 | 本地任务日志 ID |
| --- | --- | --- |
| Rust schema 聚焦 | 11 通过；同批新增名称用例首次因测试 `flush()` 的人为 response 标记被误计成泄漏而失败，修正为检查真实事件，未放宽跨项目事件断言 | `1794593504812-epcvrh` |
| Rust serve 模块（复用已通过 schema） | 261 通过、1 ignored；包含修正后的完整宿主名称切换/断开用例 | `1794593744949-221tbn` |
| 扩展 core | 名称路径两文件 29 通过，其余 56 文件 922 通过，合计 951 | `1794593504878-yb7jap`、`1794593699765-6qrc4j` |
| GUI 全组 | 83 文件 910 通过；首次新增用例失败是测试快照构造器漏传 controllerName，补齐后保留原断言并全组复验 | 初次 `1794593504878-yb7jap`；复验 `1794593699765-6qrc4j` |
| wire / lint / build | 更新现有 Rust fixtures 和扩展生成文件；gen:wire、check:wire、扩展/GUI 类型检查、Rust binary build、DevHost 内 GUI build/extension compile 通过；两份 serve.d.ts 字节一致 | `1794593744949-221tbn`、`1794594315801-6ebnmu`、`1794594278983-62ron3` |
| Rust 受影响集成 / clippy | nextest 的 serve_schema_fixture、serve_shared_e2e、acp_observer_e2e 共 17 通过、2 skipped，沿用项目默认四并发及 serial 分组；cargo clippy --all-targets -- -D warnings 通过 | `1794594343461-ihijqk` |
| 扩展集成 | external_session_observation、serve_plan_review_decision 共 6 通过 | `1794594343511-6py06h` |
| 真实 VS Code DevHost / 视觉产物 | 1 通过（17 秒）；8 组 PNG/ARIA/console 均已读取，运行中为具名 status、无后半句，空闲后消失；console 在采集范围内 errors 为空 | `1794594278983-62ron3`、`1794594394665-emu7q5` |

本次最终视觉目录为 `.agents/acceptance/shared-input/gui-srRWLd/`，8 个 PNG stem 与 C.2 列表相同；**本批图片替代 C.2 旧文案截图作为当前提示的视觉证据**。测试宿主名为 `ACP test host`，不是实际 Multica。聊天面板为 1381×794 / 331×738，具名短句完整可见，待发项、提问卡片和发送/停止按钮不互相遮挡；计划引用时提示已隐藏。原计划修订旅程也通过，仍未自动开工。

旅程身份为 `sessionId = 1794594312250_a5965ba4f528d437`、`planId = plan_shared_revision_fixture_d28e4c34`；完整临时路径保存在 `observer-journey.json`。沿用 C.2 的未受影响测试结果及未运行边界；不把此前两次非零整批改记为全绿，也不把本批模拟宿主验收写成真实 Multica 验收。没有安装或重启用户正在使用的 Tomcat，没有改动 Multica 仓库。

## C.4 追加：普通 ACP 续接提示词实时补气泡

2026-11-14，按 `plan_multica_acp_vs_code_transcript_serve_id_b9a29f4f` 修复普通 `prompt` 保存后没有用户正文通知的问题。范围仅限 Tomcat 服务和扩展，不改 Multica、模型循环或历史重建规则。首轮曾保留扩展 0.2.13 / CLI 0.1.95 并用 `acp-live-prompt` 文件后缀区分，用户指出同版本两包容易混淆；现升为扩展 0.2.14 / CLI 0.1.96，重新构建标准文件名整包，不覆盖旧产物。

```text
保存原始输入 → 沿既有流发 session.user_message → 缺少同 ID 则补普通用户气泡
             → 本端回执按 ID 更新；后续历史仍按原逻辑加载
```

复用 reference 历史投影和用户行/附件 URI 转换，事件先于成功回执和新轮输出；不重读 transcript，不轮询，不新建订阅或历史协调集合。事件不覆盖已有气泡的发送状态，也不清草稿、出队或切换宿主。保存前失败不发事件，保存后模型失败仍显示已保存输入，ACP 不收到本条通知的 user_message_chunk 回显。

### 检查与失败复验

| 检查 | 结果 | 本地任务日志 ID |
| --- | --- | --- |
| 修复前服务端红测 | 1 通过 / 1 失败；保存成功通知断言实得 0 条、期望 1 条，保存失败负例通过 | `1794624839296-eqnk0k` |
| 修复前状态红测 | 3 通过 / 3 失败 / 17 skipped；复现缺气泡、误建未打开视图与富输入不出现 | `1794624874923-ukzwp4` |
| 修复后服务端聚焦 | 4 通过：成功与模型失败顺序、保存失败负例、富输入逐字段对比 reference 历史、schema 必填字段；生成 wire/fixtures 成功 | `1794625527942-atpbqd` |
| 其余 serve 模块 | 258 通过，显式跳过已通过的 4 条通知用例，包含 busy/附件拒绝、rewind 和现有跨项目订阅隔离 | `1794626074549-a7pm6m` |
| 扩展 core | 两个改动文件 122 通过，其余 56 文件 836 通过，合计 958；未把同一家族重复计数 | `1794625484354-edmifa`、`1794626395380-27re6o` |
| GUI / 扩展集成 | GUI 83 文件 910 通过；external_session_observation 2 通过 | `1794626395380-27re6o` |
| lint / 构建 / 协议 | 扩展与 GUI 类型检查通过，GUI build 与 extension compile 通过；wire 与 Rust fixture 逐字一致，版本核对通过 | `1794625952237-mguz4f`、`1794626546337-ee8v6l`、`1794626721749-q7dv2z` |
| 真实 DevHost | 首轮新提示词与草稿断言已通过，后续旧 Stop 旅程因保留的测试草稿令主按钮保持 Send 而失败；验证草稿保留后显式清空夹具草稿，保持产品按钮及原 Stop 断言不变；复验 1 通过（19 秒）、exit 0 | 首轮 `1794625952237-mguz4f`；复验 `1794626546337-ee8v6l` |
| PNG/ARIA/console 核对 | 10 组产物非空；新场景两屏 DOM 均含唯一完整提示词、旧回复、保留草稿与具名宿主提示，且尚无模型提问；各份 console errors/events 均为空 | `1794626683348-u8offp` |
| Rust 严格 lint / 其余库测试 | `cargo clippy --all-targets -- -D warnings` 完成；其余库测试 3283 通过 / 7 ignored，跳过前述已通过的 262 条 serve 用例，库测试合计 3545 通过 / 7 ignored | `1794626629582-2yddq7`（后续 doctest 编译阶段非零，见下） |
| doctest / Rust 集成续跑 | doctest 成功（0 条）；nextest 并行组 399 通过 / 28 skipped、串行组 94 通过 / 3 skipped，包含 schema fixture、新 ACP load/prompt 首回复前通知与不回显、shared serve、rewind；release（Mach-O x86_64）构建成功，整批 exit 0 | `1794628918262-yxoi8q` |

类型检查曾在生成 wire 前失败（`1794625507236-efph81`），生成后复验通过；不是放宽事件类型。产物核对脚本首次误按失败截图的多目标格式解析普通单帧 ARIA 而报错（`1794626629633-aqkvg1`），改用产物已有 `dom/tree` 格式后核对通过，未改写采集内容。上述非零任务保留，不改记为全绿。

Rust 最终批 `1794626629582-2yddq7` 在 clippy 和库测试通过后，doctest 编译输出文件写入失败（exit 101），该失败批次未执行后续集成和 release，不计整批通过。当时磁盘可用仅约 478MiB、容量 100%；`target/debug/incremental` 占 129G。经用户明确同意，仅删除该可再生缓存（`1794628850288-q8b0sx`，exit 0），恢复约 100GiB 可用，保留源码、deps、release 和旧包。后续批使用临时 `CARGO_INCREMENTAL=0` 只续跑未完成项目，不改项目配置，不重复已通过的 lint/库测试。

### 视觉、身份与边界

最终产物目录 `.agents/acceptance/acp-live-prompt/gui-z9Nqye/`，`result.json` 为 passed / exitCode 0；10 份 PNG、ARIA 和 console 均已读回。新增 `acp-live-prompt-{desktop,narrow}` 在放行模型首个片段前采集，长提示词窄屏正常换行，草稿和具名宿主提示可见且无重叠。其余 8 个 stem 与 C.3 相同，覆盖待发、运行工具、提问及原计划引用/修订；空闲时无宿主运行提示。工作台 1440×900 / 390×844，实际聊天面板 1392×808 / 342×752；计划预览 CSS 792×786（PNG 为 2×）。console 结论仅限采集范围，不声称全进程无警告。首轮失败目录 `gui-tdmezc/` 已被复验截图替代，不作为最终通过证据。

旅程 `sessionId = 1794626595225_f4321b726324b70b`，`planId = plan_shared_revision_fixture_e8fe6f07`，模型夹具 10 次请求。新 ACP 在 VS Code 始终停留原视图时 load/prompt，完整提示词在首个回复前已显示一次；之后回答、Stop 和原计划修订均完成。当时未安装、重启用户服务、升版、提交或发布；后续仅按用户反馈修正发布版本与包身份，见下文。

**明确保留的边界：**旧历史快照晚到或分页重建仍可能暂时移除新气泡，重新读取最新历史恢复；若在实际使用中频繁复现，再在历史重建层统一处理。本次不扩展 steer/retry/rewind，不承诺异常 PDF 转引用永远不传内联内容；新旧 CLI/扩展混配不会获得本条修复。未运行真实 Multica daemon/task/issue/通知全链路、外部宿主旅程、Windows 或付费模型；不以可控 ACP/DevHost 代签这些项目。

### 首轮同版本包（已由下方 0.2.14 替代，不再推荐）

- 产物：`tomcat-vscode-ext/tomcat-vscode-ext-0.2.13-darwin-x64-acp-live-prompt.vsix`，10,533,583 bytes（10.05 MiB），240 entries，目标 darwin-x64；扩展 0.2.13，内置本次 release CLI 0.1.95。旧同版本包保留且 SHA-256 未变，新包 CLI 和消息状态模块均与旧包不同。
- SHA-256：`4e42c867f93ad1a6bff3e26e39c0d85142333aa425c5ec790a97be63bef929dc`。
- 构建 `1794631476680-qezmbc` exit 0：现有 package:vsix 完整 build（wire → GUI → extension），`--target darwin-x64 --bundle-binary <本次 release CLI> --out <新路径>`；现有安装器同款解压读取校验成功，官方 `check:wire` 通过。Vite 的大 chunk 和 vsce 的未合并 JS 文件数为警告，不记为测试失败，也未借机更改构建策略。
- 产物核对 `1794631553782-mcmun9` exit 0：ZIP CRC 正常；manifest 平台、版本、publisher 正确；内置 CLI 与 release 逐字节相同且具执行位；227 个 GUI/extension 构建文件与本地逐字节一致，CHANGELOG 匹配并含新消息处理；无源码、测试、scripts 或 node_modules。release CLI 在隔离 HOME 导出的 schema/d.ts 与 Rust fixtures 逐字节一致，且包含新事件；版本与 diff 空白检查通过。
- 未安装、重启、提交或发布；要让现有服务使用新 CLI，仍需用户在确认运行任务后自行明确重启。最终视觉证据为上列 10 套，辅助 HTTP 服务已停止。

### 最终交付版本纠正：扩展 0.2.14 / CLI 0.1.96

用户指出两个 0.2.13 包容易混淆后，改用正常 patch 版本，不再靠文件后缀区分修复内容；两个旧包均保留，但不再推荐安装。本次只纠正发布身份、CHANGELOG 和本记录，不改功能行为。

- 产物：`tomcat-vscode-ext/tomcat-vscode-ext-0.2.14-darwin-x64.vsix`，10,534,716 bytes（10.05 MiB），240 entries；内置本次构建的 Intel macOS CLI 0.1.96。
- SHA-256：`372e8e6390cb3ae3bc181b34b7568adfc8a891a2713554ccf07d518f2ed41a71`。
- `1794635533573-2mp2rv` exit 0：沿用仓库 `release-version.mjs bump --all patch` 同步五个版本文件，再以 `cargo build --release --locked --bin tomcat` 重建；版本输出为 0.1.96，Mach-O x86_64。
- `1794636199838-2wh6h0` exit 0：逐项对比 HEAD，五个版本文件仅含预期版本变化，依赖声明与解析未变；功能代码、测试和 wire 的 diff 指纹与本次升版前一致。因此复用上列功能及视觉验收，本次不重复跑测试套件或采集截图。
- `1794636479634-moc5eb` exit 0：完整 package:vsix 构建（wire → GUI → extension），标准 darwin-x64 文件名，内置新 release CLI；安装器同款解压校验、官方 check:wire 与版本一致性检查通过。沿用的 chunk/JS 文件数警告未阻断构建。
- `1794636799476-2590kx` exit 0：manifest 版本/平台正确，ZIP CRC 正常，CLI 与 release 逐字节一致且具执行位，227 个 GUI/extension 文件同时与本地和上一轮已验收修复包逐字节一致；新 CLI 在隔离 HOME 导出的 schema/d.ts 与 fixtures 一致，修复处理存在，两个旧包 SHA-256 未变，未打入源码/测试/开发依赖。
- 未安装、重启、提交或发布；用户安装时应选择本节 0.2.14 包，并在确认运行任务后自行明确重启共享服务，重启会停止所有任务。

## D. 公司分支结论

受控协议测试通过后，还必须在公司 daemon、数据库和实际 runtime profile 下跑一次完整任务。未跑完前，不要写“公司 Multica 已接入成功”。
