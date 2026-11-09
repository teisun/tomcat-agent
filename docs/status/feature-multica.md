| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tomcat | 2026-11-09 19:58 +0800 | DONE（已本地合入 develop，未推送） | feature/multica | - |

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

### 合并前审查与补验（进行中）

- 范围仍为本地 `feature/multica → develop`，原 16 个提交；不推送、不发布、不安装或重启日常服务，测试只使用自有 HOME/profile/service。尚未执行合并。
- 已修证实的问题：CLI 只从环境取运行目录/agent 身份，不让无关坏配置阻断已保存的模型目录；计划旧包装识别不再删除缩进代码中的标题；适配包 fresh 结果不丢失清理不确定性，TMPDIR 的既有执行锁保留至 daemon 退出。扩展关闭夹具显式注入测试入口并屏蔽真实安装；发布测试的 macOS runner 断言与当前矩阵同步，外层预算覆盖原有两次 10 秒子进程，不改发布断言或生产工作流。
- Rust 红绿：`1794212706261-ixxa6q` 先实测两项失败（各 101，外层成功复现）；`1794213108327-2lhpiz` 的格式、正文回归 1 项和 CLI 模型目录 4 项通过。后续审查又复现了无关非 UTF-8 环境触发的 CLI panic（`1794214594295-irsdc3` 子程序 101；外层诊断匹配写错而 exit 1），已改为按需解码身份并补不泄露错误值的回归，须以最终门禁覆盖这个最新输入，不能沿用旧 4 项作为全部最新证明。
- 扩展快门禁 `1794212759103-mnau8z` exit 0：核心 947、GUI 904 项及两侧类型检查通过。首次 integration `1794213292733-aig0sw` exit 1：282 passed / 5 failed / 1 skipped；其中旧 runner 断言、两个发布夹具超时、SDK 类型检查超时、schema 导出超时均保留原记录。发布修正后的 18 项由 `1794214135805-qsx8h6` 通过，最后的正常发布分支由 `1794214299928-mn92dg` 单项通过；未改 SDK/隔离断言，分别由 `1794214430255-6yjgnr`、`1794214478219-eraizu` 单项补验通过。按唯一用例合计 287 passed / 1 skipped，不把原组合任务改写为成功。
- 编译环境：首次 Rust 快门禁在 Clippy 阶段主动停止，未产生门禁通过结论；检查发现两套 IDE 的 rust-analyzer Cargo 也占用同一 artifact lock，不停止他人进程。当前改按原 runner 阶段补验并仅限制测试构建并发；schema 的 240 秒超时在锁释放后原断言 11.6 秒通过，SDK 原检查 4.1 秒通过。
- 扩展 integration 全文件组合补跑 `1794217546704-2769yy` exit 0：50 files、287 passed / 1 skipped，373.56 秒；其中发布脚本完整 19 项通过，覆盖此前五项失败，不再仅靠单项合并计算。原始失败记录仍保留。
- 适配包：旧包两类清理回归先失败，修复后聚焦 race `1794213108417-yj1ze9` exit 0；全新旧基线重放 `1794213272064-evn22h` exit 0，31/31 目标、18 条完整 index、许可证及清单一致，并与测试副本逐目标字节相同。锁只保证 daemon 存活期，不新增跨重启保留承诺；旧 ZIP 不包含补漏。
- 唯一联真：用户明确改用 DeepSeek，复用既有 DeepSeek helper（模型、主调用及 reviewer 均不把 Key 发往旧网关），不增加 provider 选择开关。`1794214135868-uev9uo` exit 0，精准选中 `inprocess_full_plan_path_with_real_llm`，1 passed / 0 skipped，执行 108.504 秒；不声称完整 real-llm/gate-full。旧 IDATATLAS 缺凭据阻塞由该用户决策和实测替代。
- Rust 全离线阶段、真实窗口/原生发现及最终合并复核仍进行中；最终结果记录后才在 develop 合并和更新看板。证据在 `.agents/acceptance/merge-multica-20261109/` 与上述任务日志。
- 最新聚焦批次 `1794217007722-u8drfr` exit 0：完整正文/旧包装 12 项、模型目录 4 项通过，包含无关非 UTF-8 环境与身份安全失败。复核还发现边界 `trim()` 会去掉首段代码缩进，已改为只去除首尾空白行，并通过真实保存/读取回归保留首段缩进及行末空格。此前离线批次 `1794215380243-mgadh7`、`1794215963530-yqg4ma` 为切换到该最终输入而主动停止，不算完整通过；采样时库测试仍有进展，未证实死锁。
- 旁观真实窗口 `1794216447461-a8lph4` exit 0，1/1 通过：真实鼠标选择非推荐项、宿主得到唯一非推荐答案、撤卡且任务控制继续禁用。前次 `1794215693368-ahnyvr` 的原生安装/失败分支先通过，后续旁观断言失败；原因是待答时产品切换为“可答题但不可控制”的另一条目录文案。现分别精确断言两种英文目录文案，未改产品或削弱权限断言；新桌面/窄屏 PNG 已读，四份 ARIA/console 已读，renderer errors 均为空。独立诊断中“冷 HOME 完全无目录”假设不成立（仅有内置 `.tomcat/temp`），已修正文档范围，不把此诊断误称产品失败。

### 最新门禁补漏（2026-11-09）

- Rust 整轮 `1794218193291-qbjeu9` exit 1：Clippy 和 doc 命令通过，integration 并发组 396 passed / 27 skipped、串行组 92 passed / 3 skipped；库测试 3501 passed / 1 failed / 7 ignored。唯一失败是会话释放后仍报 `session_in_use`，不以单跑及该组 9 项绿色覆盖整轮红灯。
- 对照 develop，会话 guard、manager 及原测试未变；受控暂停 pre-exec 的回归 `1794219711149-aau2g3` exit 101，稳定证明被动继承的 CLOEXEC 描述符会拖延旧 guard 的解锁。现由取得锁的进程在 guard 结束时明确解锁；创建进程 ID 防止异进程副本反向解开原 owner，真正独立 pin 的另一 owner 仍阻止删除。原有锁、metadata 复核、权限和删除顺序保留，不增加重试或放宽断言。
- 修复聚焦 `1794220230882-6drm88` exit 0：占用/删除组 10 项及模拟异进程身份的真实文件锁检查 1 项通过，包含实际 fork→pre-exec 的双向屏障，子进程在断言前放行并回收。修复后的完整 Rust 快门禁 `1794220654037-iivdyy` 仍进行中。
- 已恢复扩展剩余组合 `1794214536501-mk1gpg` exit 0：安装态 24 passing / 6 pending；VSIX 五个专项分别 4、1、3、1、1 passing；开发态 Retry/Resume 1 passing。明确复用先前 production assets，不冒称整套从零构建；原 integration 首次五项失败及后续全文件补验记录保留。
- 锁补漏前的旁观安装态和 native 专项 `1794218193350-6zag5r` exit 0：安装态旁观恢复 1 项、真实 CLI 自动安装与安装失败各 1 项通过，VS Code 1.106.0。相关桌面/窄屏 PNG、ARIA、console 已读取；native capture-time console 各有一条测试 harness 的 marketplace 404，未归为产品 renderer 回归，也不报告全窗口零错误。
- 正文修复后、锁补漏前 CLI 的旧宿主受控 ACP `1794219120749-mgstd6` exit 0：四策略、人工等待取消、目录 ID/default、registered backend 执行/取消/恢复四子场景通过；自有 HOME、loopback HTTP、假 Key，无外部模型请求。不是公司分支部署或付费模型验收。
- 同一个 DeepSeek 全流程在正文最终修复后由 `1794218390965-qpkvmd` 再次通过（1 passed / 0 skipped，107.516 秒）；本次锁修复后的同场景 `1794220654107-ap7icv` 尚进行中。只增加同一场景的回归执行次数，不新增联真场景，不宣称完整 real-llm。当前仍未合并 develop。

### 过度设计清理与本地合并（2026-11-09 19:58）

- `0ee32b5f` 清理上两次补漏：会话锁删去 owner PID 与 unsafe fork 测试，改用共享打开描述的 `try_clone` 小测试（去掉解锁后确认先红）；`serve models` 身份改为两次直接读取环境变量；正文裁剪改为三行，仅保留首行缩进语义；真实计划用例恢复 IDATATLAS，DeepSeek 仅为本轮本地验证。`d05c6ec8` 提交旁观重载探测容忍旧页面断连；`fecc3aac` 将“失败用例修复后只独立重跑”写入测试规范。
- 只复验改动与失败用例：fmt、范围 Clippy（lib、`cli_tests`、`plan_real_llm_inprocess_tests`）通过；`usage_guard` 10 项、`create_plan` 1 项、`serve_models_*` CLI 4 项、旁观 VS Code 场景 1 passing。未重跑整套门禁或联真用例：`qbjeu9` 的唯一失败已修复并单独复验；`iivdyy`、`ap7icv` 结果未读取，不作证据。
- 已本地 no-ff 合入 develop，合并树与本分支相同；未推送、发布、安装或重启服务。

### ⚠️ BLOCKED (阻塞/风险)

| 项目 | 原因／影响 | 后续与边界 |
| :--- | :--- | :--- |
| 默认答题升级行为 | 未配置普通问题策略的 ACP 智能体会等待同项目 VS Code 人工回答 | 无人值守须显式 recommended；不将自动答题当作开发或工具授权 |
| 历史搜索环境与 fallback 缺口 | 旧门禁缺 rg，fallback 不支持上下文；本轮已将可用的私有 rg 仅加入测试 PATH | 不修改搜索实现或弱化断言；本轮全库结果仍待补齐，以本节后续结果为准 |
| 历史集成工具缺失 | 旧门禁缺 cargo-nextest；本轮私有 0.9.151 已实际运行聚焦 CLI 和唯一联真用例 | 原 runner 全离线 integration 尚待完成，不用工具可用或聚焦通过代替全阶段通过 |
| help 部署边界 | TOMC-5 帮助增强、测试与架构说明纳入本轮独立提交；此前 `0.2.7` VSIX 构建早于该增强 | 不将源码提交当作新帮助已安装；如需交付该文案，再另行升版、构建与安装，不覆盖已交付的同版本包 |
| 公司源码包部署 | 旧基线源码包已更新并在隔离目录重放；公司二开分支、服务账号与迁移编号尚未代为验收 | 按包内 README/ACCEPTANCE 做语义合并和公司分支验证；不将本机源码包验收当作已部署 |
| 发布与服务边界 | 本机已核验的部署状态不代表公司二开环境或其它平台完成部署 | 本轮仅本地提交，不改真实配置/PATH、不操作运行服务、不推送或公开发布 |
| 验证边界 | 覆盖率未测量；Windows/Linux/arm64 原生旅程及远端 CI 未由本轮验证 | Cov% 留空值 `-`，提交信息省略覆盖率，不宣称跨平台或所有可能测试全绿；历史失败与环境限制保留在既有证据中 |
