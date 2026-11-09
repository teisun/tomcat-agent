| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tomcat | 2026-11-09 15:47 +0800 | ACTIVE | feature/multica | - |

### ✅ DONE (已完成/进行中)

```text
Mika → planner 写计划 / 独立评审 → 附件审阅 / 明确批准
                                             ↓
Mika 汇报 ← coder 验证并交付 ← 执行、更新同一 PlanFile
                      manual 问题 → 同项目 VS Code 旁观答题
```

- ACP 通过现有自定义参数接入四项独立设置：初始 Chat/Plan 模式、计划评审、普通问题答题和明确开工时的确认策略。新建及载入在首轮任务前应用设置，复用标准 `configOptions` / `session/set_config_option`；握手核对后台已接受启动设置，旧后台不支持时明确失败，不静默降级或自动重启。
- 默认普通问题为 manual；recommended 记录真实的策略答案来源，不代替开发批准、计划评审同意或工具授权。策略跟随控制连接，接管时恢复新控制者的设置或默认值，不写入计划或会话持久数据。
- 复用原提问桥，让同 HOME、同项目的 VS Code 旁观窗口回答 ACP 问题，支持迟到和重连补发。旁观者不能取消整个问题、接管控制或批准工具权限；服务端唯一采纳，其他窗口撤卡，客户端只缓存本窗实际提交的答案，不冒充服务端确认。
- 同机、同 HOME 下按原 `plan_id` 交接和更新标准 plans 目录内的同一文件。文件路径由智能体读取取得 ID，不新增路径参数或 SHA 交接协议。执行归属优先使用 `session_id`；旧 planner 恢复、冷载入或写入不能接管 coder 正在执行的计划。
- 整改完成：模式通知只在已有进行中 prompt 时排队，发送失败只记日志；恢复提问诊断日志，记录启动配置失败的真实原因；补齐唯一模式通知、控制者接管策略重置及消费者错误分类回归，并删除重复检查。
- 更新架构、计划工具和规划技能说明，纳入已交付的调研及早期方案文档。早期方案是历史材料，不代表其中的数据库、专用模式 UI 或运行中切换建议已实施；当前配置指南维护在 Multica 仓库 `tomcat_config.md`。
- 版本同步至 CLI / bundled CLI `0.1.89`、扩展 `0.2.7`，对应此前各一次 patch 升级，本轮不再次递增。darwin-x64 完整 VSIX 已构建并核验包内 CLI 与 release 产物一致；产物保持忽略，不入库。用户已安装插件并重启共享服务，本机会话中已核验 CLI / serve 均为 `0.1.89`，桌面 daemon 加载 Multica 修复提交 `5e36721c1`。
- TOMC-4 通用缓存键修复：在 `PromptCacheKeyFamily::key_for` 保留不超过 64 个 UTF-8 字节的逻辑键，超长时对完整会话与请求家族取 SHA-256 小写 hex。现场 75 字节评审键、64/65 边界、Unicode、长会话后缀与家族隔离均有回归；只改派生缓存键，不改会话身份、历史、权限或 provider wire。源码已提交并随上述构建部署；用户现已报告本机验收通过，不将用户验收等同于所有自动门禁或跨平台验证通过。

- TOMC-5 help 增强完成并经用户验收：根帮助以中英各四行介绍产品能力、共享服务及 ACP 入口，ACP 长帮助解释 stdio 与五项参数的用途和交互策略边界。沿用中文 clap 元数据与英文目录，仅增加 `tomcat.acp.long`；扩展原有本地化和隔离 CLI 测试，并在现有架构章节补帮助入口说明，不改参数、默认值、运行逻辑、权限或依赖。

- 公司 Multica 源码适配包已按后续指令更新：保留 `06835b8f5` 原迁移基线，`tomcat.go` 与已提交 Multica `5e36721c1` 逐字节一致，仅补旧宿主必需的清理结果接线、测试和说明。31 个目标在全新原基线精确重放，更新来源、清单及 SHA-256；模型目录实现不变，不倒入新版 Pi、桌面、调度或全局停机协议。ZIP 为本机生成的交付产物，遵循 `.agents` 忽略规则，不纳入源码提交。

### 🔌 INTERFACE (接口变更)

- `tomcat acp` 新增 `--mode=chat/plan`、`--plan-review=ask/auto/skip`、`--question-policy=manual/recommended`、`--plan-start=ask/auto`。不指定 mode 时，新会话为 Chat，恢复会话保留原模式；`--plan-start=auto` 不等于保存即开工。
- ACP select 配置保留 `model`，新增 `agent_mode`、`plan_review`、`question_policy`、`plan_start`，返回实际值与完整选项；Agent 执行期间自行切换模式时发送完整 `config_option_update`。
- 共享服务 ACP 握手回显 `acpOptions`，启动模式设置失败向客户端返回固定 `session_config_failed`。Native wire 没有新增可反序列化的策略命令，权限、路径隔离和既有控制权守卫保留。
- Multica 产品 adapter 继续原样传入自定义参数，不新增产品参数解析器、数据库字段、审批协议或轮询器。
- 公司旧基线宿主语义回迁 `Session.CleanupRequired` 和 `cleanup_unconfirmed` failure reason：未交付清理结果、handoff 超时或后台退出未确认时，保留诊断、sidecar 与 worktree，不自动提交、删除或 fresh 重试；取消回报沿用既有 wire 字段。不是 Tomcat ACP/native 协议扩展。
- `key_for` 返回值新增 64 字节上界；短键兼容，超长键与诊断日志显示稳定摘要。无新配置、依赖或协议字段，不承诺上游缓存命中率。

- 本次 help 增强仅更新展示元数据及英文帮助键，`--mode`、`--plan-review`、`--question-policy`、`--plan-start` 与 `--skills-dir` 的解析、可选性、合法值和行为保持不变；帮助继续在运行时初始化前退出。

### 验证与提交范围

| 范围 | 结果 | 证据与边界 |
| :--- | :--- | :--- |
| TOMC-5 本地化、ACP 解析与语言偏好 | 3 + 2 + 1 + 1，共 7 项通过 | `1794206279950-68yej5` exit 0；已读原日志，并复核相关源码、测试和 Cargo 文件未在该批次后修改，复用有效结果 |
| TOMC-5 隔离 CLI 黑盒 | 4 项通过；含四份双语根/ACP 帮助、空 HOME、坏配置与错误退出检查 | `1794206563841-qosh4e` exit 0；实际输出 `.agents/acceptance/tomc-5-help-output.log`，四个全新 HOME 前后均空，八个坏配置场景清单/字节不变；输入未在批次后修改 |
| TOMC-5 格式与范围 | 四个 Rust 文件的定向 rustfmt 和空白检查通过 | `1794206592999-mtbc2u` exit 0；对照 `plan_tomc_5_tomcat_help_tomcat_acp_help_acp_h_d3da124f` 审阅六文件 diff；本轮仅补 status 并提交，不重复运行有效测试 |
| 缓存键红绿 | 修复前 6 passed / 4 failed；修复后 10 passed / 0 failed | `cargo test --offline --locked --lib cache_key`；`1794185535184-ijrk2l` / `1794185709612-28zcen`，含现场摘要参考值与 AgentLoop 真实构造入口 |
| Rust Clippy / doctest / 构建 | `--all-targets -- -D warnings` 通过；doc 检查退出 0（当前 0 个 doctest）；integration 前置本地 debug 构建通过 | `1794185854805-urpf98` 各阶段独立记录；构建不是部署或真实评审验收 |
| Rust 库测试 | 3500 passed / 1 failed / 7 ignored，阶段退出 101 | 同一批次；唯一失败为 `search_files_content_mode_defaults_to_three_context_lines`，缺 rg 时走不提供上下文的既有 fallback；搜索实现和该测试未改，所有缓存键测试通过 |
| Rust integration | 未执行（缺 cargo-nextest），阶段退出 1 | 同一批次；不安装、不复刻 runner，不将 skipped/未执行写成通过 |
| 格式与空白 | `cargo fmt --all --check`、两仓 `git diff --check` 通过 | `1794185854980-u8mk03` 的 Rust 部分通过，但后续 Go 检查工作目录写错导致整条命令退出 1；已用 `1794185883867-a3l3kb` 在正确目录完成 Go 格式检查 |
| Multica agent 包 | `bash scripts/test-go.sh --race --only agent` 通过，574.389s | `1794185854916-6izl5o` exit 0；含解析早退与三个反例、既有取消/强杀/恢复边界；受既有 CLI guard 保护，不调用用户 CLI |
| 版本清单与交付包 | CLI / bundled CLI `0.1.89`，扩展 `0.2.7`；`node scripts/release-version.mjs check` 通过 | 构建与包内 CLI 字节核验见 `1794192100353-ym3mbh`；本次清单检查 `1794208016795-q35t2e`，没有再次升版或重打包 |

上述缓存键/help 自动检查保留原始结果与限制；此前插件/GUI、跨仓 ACP、手工答题与推荐答案协作的历史证据仍在 `.agents/acceptance/multica-plan-collaboration/REPORT.md` 和 `.agents/acceptance/multica-tomcat-remediation/REPORT.md`。用户确认本机验收后，版本同步与 TOMC-5 help 已分开本地提交，Multica 修复已在 `5e36721c1` 提交；不声称 L3 全绿。本次按后续指令单独更新公司源码适配包，适配包与分支 status 一并纳入本次本地提交；不安装、重启、推送或发布。

### 公司源码适配包本轮验收

| 检查 | 结果 | 证据与边界 |
| :--- | :--- | :--- |
| 最终旧基线重放 | 31/31 哈希、两份 patch、完整 index、许可证、FILES 清单通过 | `1794209746670-a52vdu` exit 0；最终副本与测试副本逐目标字节一致 |
| adapter / 四包聚焦 race | 定向边界分批通过；首次 daemon 夹具失败已补验 | `1794208715072-2xk70x`、`1794208987410-usjwpc`、`1794209605754-2ie8io`；第一次整体 exit 1 的两处夹具失败及修正记在包内 VALIDATION，不隐去，不重复其他有效检查 |
| 真实目录保留 | in_place / worktree 两场景 race 通过 | `1794209975096-ypagsh` exit 0；真实 runTask、fixture 进程、错误分类与 defer，保留实际写入文件和 AGENTS.md，未确认时不自动提交/删除 |
| Go module 编译 | `go build -p 2 ./...` 通过 | `1794209018834-2jafsl` exit 0；覆盖 CLI/daemon/execenv/handler 消费者，不是安装 |
| 受控真实 ACP | 四子场景通过，含四策略与真实待答撤卡 | `1794209066858-umor5v` exit 0；release CLI `0.1.89`、私有 HOME、loopback/假 Key；未接触用户共享服务，不冒称真实模型独立 Review 链路 |
| 格式与交付范围 | 普通文件、应用后源码空白检查及 Go 快照 gofmt 通过 | raw unified patch 上下文会触发 Git 文本空白提示，原 exit 2 与正确校验方式已记录；Multica 活跃工作树保持干净 |

本次提交按用户要求不运行测试、构建或覆盖率检查；上表为此前适配包更新阶段的已有结果。旧门禁失败、缺 nextest、help 未进入既有 VSIX 等边界保留在上下文及下表。资料包来源和细节见 `tomcat/docs/architecture/tomcat-acp-host-integrations/multica-tomcat-acp-integration/VALIDATION.md`。

### ⚠️ BLOCKED (阻塞/风险)

| 项目 | 原因／影响 | 后续与边界 |
| :--- | :--- | :--- |
| 默认答题升级行为 | 未配置普通问题策略的 ACP 智能体会等待同项目 VS Code 人工回答 | 无人值守须显式 recommended；不将自动答题当作开发或工具授权 |
| 搜索环境与既有 fallback 缺口 | PATH 中缺少 rg，库测试的上下文行断言失败；`search.rs:573-577` 已明确不支持 fallback 上下文 | 未纳入本次两文件修复，不弱化断言或静默跳过；另行补齐环境/修复搜索边界后复验 |
| 集成测试工具 | 缺 cargo-nextest，本次 integration 未执行 | 由用户决定是否安装，再跑原 runner 的 integration 阶段；不声称全门禁通过 |
| help 部署边界 | TOMC-5 帮助增强、测试与架构说明纳入本轮独立提交；此前 `0.2.7` VSIX 构建早于该增强 | 不将源码提交当作新帮助已安装；如需交付该文案，再另行升版、构建与安装，不覆盖已交付的同版本包 |
| 公司源码包部署 | 旧基线源码包已更新并在隔离目录重放；公司二开分支、服务账号与迁移编号尚未代为验收 | 按包内 README/ACCEPTANCE 做语义合并和公司分支验证；不将本机源码包验收当作已部署 |
| 发布与服务边界 | 本机已核验的部署状态不代表公司二开环境或其它平台完成部署 | 本轮仅本地提交，不改真实配置/PATH、不操作运行服务、不推送或公开发布 |
| 验证边界 | 覆盖率未测量；Windows/Linux/arm64 原生旅程及远端 CI 未由本轮验证 | Cov% 留空值 `-`，提交信息省略覆盖率，不宣称跨平台或所有可能测试全绿；历史失败与环境限制保留在既有证据中 |
