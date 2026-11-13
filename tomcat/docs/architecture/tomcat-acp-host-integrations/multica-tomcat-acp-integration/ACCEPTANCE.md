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

## D. 公司分支结论

受控协议测试通过后，还必须在公司 daemon、数据库和实际 runtime profile 下跑一次完整任务。未跑完前，不要写“公司 Multica 已接入成功”。
