# 连接器验收整改记录

> 本次按计划在源码仓库保留这一份验收更正；一般调研归档仍遵循本目录 README 的 wiki 约定。

## 范围与当前状态

源码基线：`1cc07c8b41991f13b33643d5692d8d849f1aadca`，分支 `feature/transcript-rich-render`。该 SHA 只标识 HEAD，不表示当前工作树干净；整改在未提交工作树上继续。本次不恢复旧 diff、不回退连接器、不提交或发版。`.agents` 和 Cursor commands 不在本次操作范围。

提交说明（2026-09-08）：上述“不提交”是整改执行阶段的范围约束。验收结束后，用户另行要求按 `commit-with-status` 提交代码；本次仅补分支 status 和本说明后做本地提交，不推送或发版。后文 HEAD、58 个路径及哈希均为验收结束时的历史快照，不随此次提交改写。

本次只修测试、测试脚本和说明；正式代码仅允许 guard 判断抽取和 Messenger 可等待关闭。**不改计划状态机、Acceptance 工具或自动催促。必验批次的最终结果与限制见末尾；运行时收口状态以活动 PlanFile 为准，不把局部绿当完成。**

```text
旧结果：部分成功 + 完整检查失败 -> 只报成功部分 -> 错误收口
本次：逐项保留结果 -> 修复后重跑 -> 必验项目未全部通过就不称验收成功
```

## 对上次结论的更正

撤销“所有失败都是与连接器无关的基线问题”的笼统判断。定向连接器测试、打包、截图成功，不能证明其他失败与改动无关，也不能替代完整集成及真实 VS Code 安装版检查。

旧计划 `~/.tomcat/plans/plan_tomcat_mcp_http_oauth_connectors_webview_65391cc4.plan.md:122-175` 中的成功回执及 completed 保留为历史，不改写它们。当前 Acceptance 只验证提交的成功记录，不替执行者核对是否漏报；本次只记录这个限制，不修改该机制。

## 历史失败索引

下列是已经读过的原日志，不是本轮重新运行的证明。日志路径仅供本机查证，不复制原始日志、截图或用户配置进仓库。

| 历史批次 | 实际结果 | 原始日志（相对 `~/.tomcat/agents/main/tool-results/`） |
|---|---|---|
| 扩展第一轮 | 15 失败、454 通过；11 项旧 Plan 检查、2 项恢复展示、2 项超时 | `bash-1788622940880-ezkypu.log:8-92,1051-1052` |
| 扩展后一轮 | 14 失败、455 通过；12 项旧检查、get_state 超时、ask_question 清理 ENOTEMPTY；两轮失败集合不同 | `bash-1788623821438-m4fxig.log:8-77,514-543,1015-1016` |
| Rust lib | 2776 通过、1 个 guard 失败、1 ignored | `bash-1788623847632-ub3aag.log:2820-2832` |
| 另一轮 Rust CLI | 111 通过、1 个真实模型后台通知失败；此前 lib 通过 | `bash-1788622941057-2j94o9.log:3304-3343` |
| checkpoint | 未汇总；pre-rollback 用例未完成，随后任务被停止，不能算通过 | `bash-1788626116024-6rapfb.log:2890-2905` |
| VSIX | 安装成功、窗口启动失败、未取得截图 | `bash-1788625920213-i7lhsd.log:183-228` |

历史调用的完整参数/后台 ID 不在以上文本日志中的，标为证据缺失，不按日志文件名补造 ID。重跑必须记录完整命令、实际选集、退出状态和真实任务 ID，不能以另一个子集成功覆盖原完整批次失败。

## 已查明与仍待查明

- 旧测试未同步：Plan 的 workspaceMode/agentMode/activePlan 与旧字段混用；Retry/Resume 旧断言要求删除失败章节，而现有产品保留它。依据：`tomcat-vscode-ext/src/serveClient/sessionRouter.ts:25-45`、`tomcat-vscode-ext/src/ui/webview/state.ts:2597-2624` 及相应集成测试。仅据此归因这些断言。
- guard 单测没有建立“标志未启用”的前提。依据：`tomcat/src/api/cli/tests/nested_guard_test.rs:286-307`。生产禁止嵌套修改的规则保留。
- checkpoint 独立环境有界复现和完整 14 项通过；本轮未重现历史卡点，不能把安全标志拒绝当成历史卡死根因。
- 本轮 serve 清理超时出现过主进程已退出但 stdio 未关闭；隔离 fixture 的默认 Playwright MCP 派生子进程持有日志通道。fixture 明确使用空 MCP 后，等待 close 再删目录，定向测试通过。不修改产品 MCP、不延长关闭时限。
- VS Code 环境 A 组显式 `ELECTRON_RUN_AS_NODE=1` 重现“把仓库当 Node 模块”；清理后的五种真实窗口场景通过。这支持本轮启动修法，不能反推没有环境记录的历史日志已经完全归因。
- 读本轮 PNG 发现系统截屏拍到了用户宿主窗口，不能作为测试窗口证据。改用每次测试独立 CDP 端口（浏览器调试接口）直接捕获该渲染器，保留 PNG、实际可访问性树和截屏时浏览器缓存日志；这些日志不宣称是全时段追踪。
- 后台通知离线 mock 的多轮失败最终定位到 `sseFinish` 多余 `}`，首个结束事件 JSON 无效。修正后两种任务完成/EOF 顺序都通过；真实 DeepSeek 单项也另行通过。两类证据不互相替代。
- Cargo target 分类由契约检查核对；nextest 实际列举的五个混合 binary 共 142 项精确分为 118 离线、17 live、7 手动。完整默认集成仍须最终批次执行。

## 本轮运行记录

本节为分阶段记录，不是最终验收成功回执。工作目录缩写：E=`tomcat-vscode-ext`，R=`tomcat`，W=仓库根；日志统一在 `~/.tomcat/agents/main/tool-results/bash-<后台任务 ID>.log`。所有任务在上述 HEAD 加本轮未提交修改上运行，后续改到的面必须重新检查。

| 检查 / cwd | 完整命令 | 实际结果 / 后台任务 ID |
|---|---|---|
| guard / R | `cargo fmt --all && cargo test --lib api::cli::tests::nested_guard_test` | 通过；`1788839900832-uuouq4` |
| provider / E | `npx vitest run --maxWorkers 1 tests/webview_provider_flow.test.ts && npm run lint:extension` | 通过；`1788839944792-ulhkb2` |
| checkpoint / R | `cargo fmt --all && env -u TOMCAT_AGENT_ACTIVE cargo test --test checkpoint_cli_e2e -- --nocapture` | 14 通过；`1788842989399-uj7zmm` |
| Messenger / E | `env -u TOMCAT_AGENT_ACTIVE npx vitest run --maxWorkers 1 src/serveClient/tests tests/serve_fixture_lifecycle.test.ts tests/serve_disposal_integration.test.ts tests/serve_ask_question_integration.test.ts && npm run lint:extension` | 49 通过及类型检查通过；`1788843137128-qqvew8` |
| 脚本类型 / E | `npx tsc --ignoreConfig --noEmit --strict --types node --target ES2022 --module ESNext --moduleResolution Bundler --esModuleInterop --skipLibCheck scripts/buildArtifacts.ts scripts/vscodeLaunchEnv.ts scripts/run-vscode-verify-vsix.ts scripts/run-vscode-install-e2e.ts scripts/run-vscode-devhost.ts scripts/run-vscode-manual-acceptance.ts scripts/run-vscode-image-acceptance.ts` | 通过；`1788844059188-6sn7d5`；后续截图代码修改待最终重新检查 |
| 打包与选择 / E | `env -u TOMCAT_AGENT_ACTIVE npx vitest run --config vitest.integration.config.ts --maxWorkers 1 tests/package_vsix_smoke.test.ts tests/cli_autofeed.test.ts tests/integration_selection.test.ts` | 整体失败：13 通过（含 10 package smoke）、2 autofeed 失败；`1788844094868-utqcf3`，不得当整体成功 |
| 修复模拟通知 / E | `env -u TOMCAT_AGENT_ACTIVE npx vitest run --config vitest.integration.config.ts --maxWorkers 1 tests/cli_autofeed.test.ts tests/serve_fixture_lifecycle.test.ts` | 9 通过；`1788844903817-2rca7r` |
| 真实模型补充项 / R | `env -u TOMCAT_AGENT_ACTIVE cargo test --test cli_tests test_user_background_bash_autofeed_real_llm_cli -- --exact --nocapture --test-threads=1` | DeepSeek 1 通过、111 过滤、0 ignored，22.42s；`1788844924739-6tmb6t`；本次运行没有走连接失败 skip 分支 |
| VSIX 五场景 / E | `env -u TOMCAT_AGENT_ACTIVE npm run verify:vsix` | 9 测试通过，退出 0；`1788844664834-mirt3i`。**视觉证据不通过**：读图发现截错窗口，旧裁剪路径也失效。原图 `/var/folders/1t/xdx9bpn14bb365np42rb36rw0000gn/T/tomcat-vsix-verify-artifacts/run-dh1Odt/gui-SfNFY5/` 保留 |
| Electron A 对照 / E | `env ELECTRON_RUN_AS_NODE=1 '/Applications/Visual Studio Code.app/Contents/MacOS/Code' /Users/yankeben/workspace/tomcat-agent` | 预期故障：退出 1 / MODULE_NOT_FOUND；`1788845247718-aqxcl6`，不提交为 green-build |
| 启动包装 / E | `npm run lint:extension && npx tsc -p e2e-harness/tsconfig.json --noEmit && npx vitest run --config vitest.integration.config.ts --maxWorkers 1 tests/vscode_launch_env.test.ts` | 类型检查及 17 测试通过；`1788845247482-ika7vg` |

本轮未转绿前的原始失败还包括：`1788839802139-agszcy`（真实 CLI 继承安全标志）；`1788840609700-m7v8p3`（checkpoint 被同一标志拒绝）；`1788842685126-66uc9m` / `1788842776850-ur8qyf`（serve stdio 关闭）；`1788842893016-y5pe0y`（ask_question 清理）；`1788843585449-ink4u7`（同步内嵌 build 180s 超时）；`1788844295142-ehy7yq` / `1788844387893-ekvvcg` / `1788844615984-adbtjm` / `1788844682910-6fq5vq`（mock 通知旧帧/重试断言）。过宽 nextest 列举 `1788843350859-mt1yxa` 被主动停止，不算完成。

正常独立 CLI 用例由外层命令显式 `env -u TOMCAT_AGENT_ACTIVE` 启动；测试 fixture 和 GUI 环境 helper **不清除**该安全标志，guard 的 active 用例仍检查原拒绝规则。`ELECTRON_RUN_AS_NODE` 只在 GUI 作用域清理并还原；不改用户全局配置。

补充失败记录：定向 devhost 命令 `env -u TOMCAT_AGENT_ACTIVE TOMCAT_E2E_SCREENSHOT=1 TOMCAT_E2E_GREP='renders the transcript UI groups, tool rows, file chips, and progress' TOMCAT_E2E_CAPTURE_PROGRESS=1 TOMCAT_E2E_TRANSCRIPT_PROGRESS_DELAY_MS=1500 npm run test:e2e:webview-devhost`（E）使用了安装版的测试名称，实际 **0 passing**，虽退出 0 仍判为未验证，任务 `1788845295187-a2lfth`。已给两种常规 Mocha 入口加 `failZero`，随后使用源码确认的 devhost 名称重跑。辅助生成的 `.bak` 和 `true` 已移到本机临时目录 `/var/folders/1t/xdx9bpn14bb365np42rb36rw0000gn/T/tomcat-remediation-scratch-5NrRJd/`，未删除用户文件；两个 shell 脚本的可执行权限已恢复。

截图修复定向复跑（E）：`env -u TOMCAT_AGENT_ACTIVE TOMCAT_E2E_SCREENSHOT=1 TOMCAT_E2E_GREP='renders transcript action rows and context groups in the Tomcat webview' TOMCAT_E2E_CAPTURE_PROGRESS=1 TOMCAT_E2E_TRANSCRIPT_PROGRESS_DELAY_MS=1500 npm run test:e2e:webview-devhost`，任务 `1788845477109-2t5ure`，**1 passing、退出 0**。已读 `/var/folders/1t/xdx9bpn14bb365np42rb36rw0000gn/T/tomcat-vscode-runs/gui-LEu0hj/` 的 progress/collapsed PNG：确为 `[Extension Development Host]`，计划卡、工具结果和进度显示正常；窄侧栏会截断长命令/标题，本次不修改 UI 布局。匹配 console 记录没有 error；最初 AX 只取到外层 webview，已补取内层 frame，最终批次须核对实际应用标签。截图窗口问题已闭环，完整验收尚未完成。

## 首轮审查后的修正

- F02：初始化与两个长驻 checkpoint CLI 统一经过同一个环境构造入口，移除继承的 `TOMCAT__*` 配置覆盖，再显式写入本轮 mock 设置；安全标志不在移除范围。定向复跑会额外注入错误 work_dir/model，检查隔离仍成立。
- F03：工具后台 shell 使用自己的进程组，不能只随 CLI 组清理。增加 PID 文件的资源所有权：失败时先停止 CLI，再结束后台组，最后删除目录；加入 panic 与超时两条异常路径测试，正常 task_stop 和 transcript/checkpoint 断言不删除。
- F04/F05：缺 CLI、setup 恢复前后、瞬态恢复、慢握手均在实际断言点取 PNG/ARIA/console；manual/image 的独立图片目录进入本轮索引，不再错误标记“未生成图”。
- F01 执行条件说明：继续执行的最新用户消息明确要求真实 CLI 命令加 `env -u TOMCAT_AGENT_ACTIVE`、active guard 用例保留标志，已向运行时记录此执行授权相对旧计划文字的已知取舍。历史命令不改写，也不把它们伪称用户亲自开启的独立 shell；后续仅按该明确授权运行隔离测试，不修改活动 Agent 环境或产品安全规则。

审查修复定向结果：R 下 `cargo fmt --all && pollution=$(mktemp -d /tmp/tomcat-checkpoint-env-XXXXXX) && env -u TOMCAT_AGENT_ACTIVE TOMCAT__STORAGE__WORK_DIR="$pollution" TOMCAT__LLM__DEFAULT_MODEL=should-not-be-used cargo test --test checkpoint_cli_e2e -- --nocapture`，任务 `1788846315682-mtul9x`，15 通过；日志中的 panic 是 catch_unwind 验证路径，不是套件失败。E 下 `npm run lint:extension && npx tsc -p e2e-harness/tsconfig.json --noEmit && npx vitest run --config vitest.integration.config.ts --maxWorkers 1 tests/vscode_launch_env.test.ts`，任务 `1788846315757-ittehw`，类型检查及 18 测试通过。

第二轮审查补充：原 onboarding 测试读取的是被 suppress/自动选动作后的提示历史，不能证明用户看到了警告。verify 的缺 CLI/setup 两个场景现明确启用真实通知、清空自动动作；先等待窗口里出现警告文字并截图，再在同一个测试窗口点击 `Start Setup` 驱动恢复。生产提示实现不变。

真实通知补证（E）：`env -u TOMCAT_AGENT_ACTIVE npm run verify:vsix`，任务 `1788846870725-manbjf`，五场景分别 **4/1/3/1/1 passing**，合计 10，通过。证据根目录 `/var/folders/1t/xdx9bpn14bb365np42rb36rw0000gn/T/tomcat-vsix-verify-artifacts/run-jNwqCK/`；已读 `gui-U6Bvyh/onboarding-prompt.png`、`gui-dKQ6UV/setup-required-before-recovery.png` 和 `setup-recovered-stream.png`：真实警告、Start Setup 按钮、终端初始化与恢复回复都可见。全部配套 console error 列表为空，ARIA 包含实际警告和 Start Setup，正常场景包含内层 Transcript UI Showcase 标签。可选旧裁剪器仍尝试裁剪不适用的 startup PNG，已限定到 transcript 图片目录；全帧原图未受影响。VS Code 自身 stderr 仍有 `Unknown channel: agentHostClientByokLm` 和 Node deprecation 警告，保留但不归为本次扩展渲染错误。

## 最终批次：失败保留与后续复跑

| 检查 / cwd | 完整命令或入口 | 实际结果 / 任务 |
|---|---|---|
| 首轮 Rust / R | `cargo fmt --all -- --check && env -u TOMCAT_AGENT_ACTIVE bash scripts/pre-commit-check.sh gate-fast && env -u TOMCAT_AGENT_ACTIVE cargo build --release` | 整体退出 1，`1788847171949-ea58yo`。格式/Clippy 通过；库测试 2779 通过、1 ignored；doc 0 项；默认集成 373/377 通过、4 失败、25 skipped。注意误用了 pre-commit 包装（它不解析 gate-fast 参数，成功后还会运行覆盖率），因集成失败尚未进入覆盖率和 release；不把此命令记作验收通过 |
| 修复后四项 / R | `cargo fmt --all && env -u TOMCAT_AGENT_ACTIVE cargo nextest run --test cli_tests --test serve_schema_fixture --test serve_stdio_e2e --no-fail-fast -E 'test(=test_user_chat_skill_list_reload_use) \| test(=serve_print_schema_matches_fixture) \| test(=serve_stdio_user_roundtrip_e2e) \| test(=serve_stdout_only_emits_ndjson_frames)'` | 4 通过、120 过滤，`1788848342135-q8eoil` |
| 当前完整 Rust 复跑 / R | `env -u TOMCAT_AGENT_ACTIVE bash scripts/run-integration-tests.sh integration && cargo clippy --all-targets -- -D warnings && cargo fmt --all -- --check && env -u TOMCAT_AGENT_ACTIVE cargo build --release` | `1788848440992-iu7glp`，退出 0；默认集成 377/377 通过、25 skipped（不计通过），含本地 HTTP/OAuth feature；Clippy/格式通过，release 构建完成（12m55s）。库/文档测试的代码未再改，不重复其已经通过的完整批次 |
| 扩展完整入口 / E | `env -u TOMCAT_AGENT_ACTIVE npm run check:wire && env -u TOMCAT_AGENT_ACTIVE npm run gate:full` | `1788849094292-antcwq`，整体退出 1。check:wire、lint、35 文件/409 core tests、67 文件/572 GUI tests、build 及共享打包通过；集成 21 文件通过/4 文件失败，共 138/142 tests 通过，另有 1 unhandled rejection；安装/verify 尚未进入，不记整条成功 |

四个失败的修复边界：schema 快照只补上**已有协议**的 `FileDiffLine.skippedLines`（与真实生成文件逐字比较）；CLI skill fixture 删除只读 8192 字节就回包的服务器，复用完整请求读取及析构清理；Rust serve fixture 使用空 MCP，并在进程运行中持续收集 stderr，避免超时日志空白。不改正式协议，不增加等待时限。修复后两项 serve 都在约 7.7s 内通过，但不把这等同于全部历史超时已经归为同一根因。

扩展集成失败后的处理：
- 3 个 Plan 状态/事件测试原来只给一条普通文字，却等待整个执行计划结束；修正 JSON 帧后执行循环真正继续，mock 返回 unexpected request。现在固定流先发文本，测试查询 executing，再用公开 interrupt 收尾，并断言 pending/agent_idle，保留 build 事件、错误码和身份检查。不改执行、Acceptance 或催促逻辑。
- model 选择原来依赖内置 DeepSeek 条目和外部真实 key；测试现在在私有 models.toml 显式加入本地 endpoint 和 fixture 虚拟 key，保持选择/持久化断言。
- 首次定向 `1788849762897-qypezn` 为 3 通过/1 失败：错误地把 interrupt 后的状态仍预期 executing；已补中断前快照和中断后的 pending 两个断言。
- 最终定向（E）`env -u TOMCAT_AGENT_ACTIVE npx vitest run --config vitest.integration.config.ts --maxWorkers 1 tests/serve_set_plan_mode.test.ts tests/serve_get_state_planstate.test.ts tests/serve_plan_events.test.ts tests/serve_list_models.test.ts`，`1788849899908-uajhtn`，4/4 通过。
- 续跑不重复已通过的 lint/unit：会话临时编排文件 `/Users/yankeben/.tomcat/temp/remediation-final-gate.ts` 仅调用项目 `packageVsix(skipBuild:true)`（仍校验产物哈希）、`createHostE2eFixture`，随后顺序运行 `npm run test:integration`、`npm run test:e2e:vscode-install`、`npm run verify:vsix`，通过现有 PREBUILT 环境传递同一包并清理自有临时目录。命令 `env -u TOMCAT_AGENT_ACTIVE npx tsx /Users/yankeben/.tomcat/temp/remediation-final-gate.ts`（E），任务 `1788850121647-mrxp16`，**退出 1**：完整集成 25 文件/142 tests 全通过，安装版 17 passing、5 场景专属 pending、2 failing（问题重启及随后中断）；verify 未执行。不能提交为整条成功；不向仓库新增门禁协议或检查体系。

浏览器补充检查记录：
- `1788847474936-6peizu`：managed browser 尚未准备，退出 1；按技能运行 `node /Users/yankeben/.tomcat/skills/verify/scripts/bootstrap.mjs`（`1788847687475-edw42t`，退出 0）。当前 mac13 不支持所锁定 Chromium，bootstrap 明确选择本机 Chrome。
- `1788847752957-a27ddk`：开发页 `/favicon.ico` 404，退出 1。开发 HTML 现引用已存在的 `public/favicon.svg`；不改变打包 webview 的 HTML/布局。
- `1788848441027-ieb3fb`：1440×900 的 desktop-fixed 截图和零错误 console 已生成，但随后 390×844 的导航 30s 超时，整条命令退出 1；不计整条成功。
- `1788848559375-a6et5f`：用 `curl --max-time 10 --fail` 分别检查 HTML、main.tsx、styles.css、favicon.svg，四项均 200（0.137/0.018/0.013/0.047s），退出 0。随后仅重跑未通过的窄窗口：`node /Users/yankeben/.tomcat/skills/verify/scripts/shot.mjs http://127.0.0.1:5199/ --out /Users/yankeben/workspace/tomcat-agent/.tomcat/shots/remediation --name narrow --viewport 390x844`，任务 `1788848788739-2zarp5`，退出 0。
- 已读 `.tomcat/shots/remediation/desktop-fixed.*` 与 `narrow.*` 的 PNG、ARIA、console：未接 VS Code 的开发页显示 Connecting 和禁用控件，无 console error。这只证明独立页面/资源加载及尺寸边界，**不替代前述真实 VS Code 通知与交互证据**。两个临时 Vite 服务均已停止。

运行时审查在两轮预算用尽后放行，并携带最后的提示可见性意见；不是声称第三轮 reviewer 零问题。该意见已由真实通知 PNG、ARIA 文字和手动点击恢复的定向结果处理，最终 verify 场景仍需通过。

### 安装版问题卡复现

- 定向命令（E）`env -u TOMCAT_AGENT_ACTIVE TOMCAT_E2E_GREP='renders ask_question answers|hydrates Disconnected|resets interrupted Tomcat' npm run test:e2e:vscode-install`，`1788850846242-m6d94k` 退出 1，1 passing/2 failing。新增的超时诊断明确显示 `connectionStatus=ready`、`busy=true`、`approvals=[]`。原始窗口日志保留在 `/var/folders/1t/xdx9bpn14bb365np42rb36rw0000gn/T/tomcat-vscode-runs/gui-0prWyO/`。
- `scripts/e2eHostFixture.ts` 的模拟问题原以 sessionId 生成 request/tool ID，同一会话的第二次提问撞上前次完成记录；同时恢复文件只保存 pendingApproval，历史缺少当前 assistant tool_call 和 `[pending]` 结果。新回归 `tests/host_fixture_questions.test.ts` 修前以“两个 toolCallId 相等”失败，命令 `npx vitest run --config vitest.integration.config.ts --maxWorkers 1 tests/host_fixture_questions.test.ts`，任务 `1788851091798-y6eogp`。测试仍覆盖重启后新 requestId/原 toolCallId、旧回复不能结束新请求、回答后 idle 及下一轮可接受。
- 修法只落在 fixture：每次工具调用使用独立 ID，写入真实形状的等待历史，完成时将等待结果标记 superseded 并追加正式结果。没有改产品恢复/传输层、guard、超时、Plan 或 Acceptance。

### 最终续跑已完成的部分

| 检查 / cwd | 完整命令 | 结果 / 真实后台 ID |
|---|---|---|
| 问题卡修复定向 / E | `npx vitest run --config vitest.integration.config.ts --maxWorkers 1 tests/host_fixture_questions.test.ts && npm run lint:extension && npx tsc -p e2e-harness/tsconfig.json --noEmit && env -u TOMCAT_AGENT_ACTIVE TOMCAT_E2E_GREP='renders ask_question answers\|hydrates Disconnected\|resets interrupted Tomcat' npm run test:e2e:vscode-install` | 退出 0；fixture 1 项、类型检查、安装版 3 项全部通过；`1788851170040-enoxzs` |
| 不带筛选的完整续跑 / E | `env -u TOMCAT_AGENT_ACTIVE -u TOMCAT_E2E_GREP TOMCAT_E2E_SCREENSHOT=1 npx tsx /Users/yankeben/.tomcat/temp/remediation-final-gate.ts` | 退出 0；集成 26 文件/143 tests，安装版 19 passing/5 场景专属 pending，verify 五场景 4+1+3+1+1 passing；`1788851268943-u6w98m`。5 个 pending 不是 pass，其缺 CLI/setup/瞬态/慢握手场景随后由 verify 专属窗口执行 |

该续跑经哈希校验复用当前构建，共享包为 `/var/folders/1t/xdx9bpn14bb365np42rb36rw0000gn/T/tomcat-remediation-final-vsix-FLwp4E/tomcat-final.vsix`（217 文件，扩展 0.1.61，测试用 bundled CLI 0.1.47；任务结束后自有包目录已清理）。不冒充生产 CLI 分发包，也不声称原来的失败 gate:full 命令变成退出 0；它已经通过的 lint/core/gui/build 与本轮剩余阶段合起来才覆盖完整门禁。

本轮视觉证据根目录 `/var/folders/1t/xdx9bpn14bb365np42rb36rw0000gn/T/tomcat-vsix-verify-artifacts/run-PbCVC6/`：已读 `gui-KxChmT/onboarding-prompt.png`（缺 CLI）、`gui-hIvtFS/setup-required-before-recovery.png` / `setup-recovered-stream.png`（实际通知、点击 setup 后终端初始化和回复）、`gui-msvqai/tomcat-vsix-visual-{progress,collapsed}.png`（计划卡和进度）、`gui-VrOJcl/transient-startup-recovered.png` / `gui-sNIRr8/slow-handshake-ready.png`（Ready to chat）。11 份 console JSON 均未记录 error/exception；仍有 Electron iframe/feature warnings。ARIA 包含实际警告、Start Setup、恢复回复和内层 Transcript UI Showcase。**这是截图时刻缓存日志，不是全程浏览器追踪。** 安装版最后的 error-boundary 测试刻意制造崩溃且断言收到错误，不能把那条预期错误当未处理故障，也不称整段 stderr 为空。

已观察的显示限制仍保留：窄侧栏长标题/提示被截断，collapsed 图底栏模型/Ctx 文字拥挤；setup 成功回复图仍有系统信息通知遮住部分输入区。本计划不修改 UI 布局或通知产品行为，不宣称这些画面像素级零问题。旧可选裁剪器还报告未生成的 expanded/file-chip/tool-icons 图片；本轮验收依据是已读取的全帧 PNG 和实际断言，不用缺失裁剪图凑证据。

Git 核对：`tomcat/scripts/pre-commit-check.sh` 并未误删标题；第二行 shell 注释被有意改为“同一离线门禁 + 库覆盖率”，对应改后的执行命令，可执行模式仍为 100755。根 `.gitignore` 仅新增 `tomcat-vscode-ext/.build-artifacts.json`，这是本地构建指纹记录，不是隐藏源码或失败日志。`git diff --exit-code -- .agents .cursor/commands release-versions.json` 退出 0；未读删 `.agents` 内容。

### 其他窗口入口的失败与修复

- devhost 全选：`env -u TOMCAT_AGENT_ACTIVE -u TOMCAT_E2E_GREP TOMCAT_E2E_SCREENSHOT=1 npm run test:e2e:webview-devhost`（E），`1788851662350-i7temo`，退出 1，32 passing/4 场景 pending/1 failing：rejected recovery 用例只等到错误正文就点击，未等待历史恢复后的 Retry 按钮。测试现等 `agent_idle`，重载历史并明确等按钮出现，再单次点击，不改超时或产品逻辑。定向 `env -u TOMCAT_AGENT_ACTIVE TOMCAT_E2E_SCREENSHOT=1 TOMCAT_E2E_GREP='shows a rejected recovery reason inline' npm run test:e2e:webview-devhost`，`1788852027991-yi1oil`，退出 0、1 passing；不以此替代最后全选复证。
- manual 全选：`env -u TOMCAT_AGENT_ACTIVE -u TOMCAT_E2E_GREP TOMCAT_ACCEPT_SKIP_BUILD=1 npx tsx scripts/run-vscode-manual-acceptance.ts`（E），`1788851915452-6gtlos`，退出 1，0 passing/2 failing（并非零选集，两个用例实际运行并失败）。浏览器原始错误是 `unknown_command: list_checkpoints`，初始化虽已连接但无法加载会话；专用 `scripts/manual-acceptance/fake-serve.js` 没有该已有命令。现为无 checkpoint 的模拟会话返回空列表，补能力声明和定向契约检查；同步其旧 mode/agentMode 字段，并给 manual/image Mocha 入口也加 failZero。没有修改产品命令或初始化流程。

## 固定最终检查

- Rust：格式、`gate-fast`（包含完整默认集成及 fake HTTP/OAuth）、`release` 构建。
- 扩展：先 `check:wire`，再 `gate:full`；核对 lint、core/gui 单测、完整集成、安装及 verify:vsix 的实际执行情况。
- 改过但完整批次不覆盖的入口：devhost、image、manual 的真实窗口检查；读取本轮 PNG、ARIA、console。启动失败不得复用旧图。
- 工作区：`git diff --check`、版本只读检查；不自行改版本。
- 真实模型后台通知为计划预先声明的补充项；缺条件、未跑或仍失败单列，不能称真实模型问题已修复。固定模拟模型通知测试仍是必验。

每条最终记录附工作目录、完整命令/过滤、代码版本及工作树差异标识、后台任务 ID、退出码、执行/失败/跳过/未运行数、构建或 VSIX 标识及日志/截图位置。被停止、空匹配、缺依赖和缺证据都不是通过。

## 最终补齐：devhost / manual / image（2026-09-08）

所有真实 `tomcat` / VS Code 入口均显式移除 `TOMCAT_AGENT_ACTIVE`；下面的完整入口同时移除 `TOMCAT_E2E_GREP`，没有用定向绿替代完整入口。原始日志统一为 `~/.tomcat/agents/main/tool-results/bash-<任务ID>.log`。

### 追加失败记录与针对性修复

- `1788852107289-idt0x6`：fake-serve 回归 0 通过/1 失败，期望 `workspaceMode=project` 实际 `code`；修复 fixture 模式归一化，不改产品协议。
- `1788852206223-itlfog`：`npx vitest run --config vitest.integration.config.ts --maxWorkers 1 tests/manual_acceptance_fake_serve.test.ts && npm run lint:extension && npx tsc -p e2e-harness/tsconfig.json --noEmit && env -u TOMCAT_AGENT_ACTIVE -u TOMCAT_E2E_GREP npx tsx scripts/run-vscode-manual-acceptance.ts`（E）。fixture 1 项与类型检查通过；manual 1 passing/1 failing，初始化缺命令已消失，但历史工具检查仍等待旧 `search_files` 标题。当前 UI 将上下文工具放入默认折叠组，并显示 `Searched files`。只修 manual 测试：先展开历史/当前组，按现有 `tool-row-toggle` 与可见标题操作；思考顺序在工具尚未聚组时核对实际 HTML。历史加载、展开、自动滚动/脱离/回到底部、成功工具默认折叠、错误工具展开及长内容内部滚动断言均保留。失败窗口 `/var/folders/1t/xdx9bpn14bb365np42rb36rw0000gn/T/tomcat-manual-artifacts-7hzZl6/` 保留。

### 完整入口结果

| cwd | 完整命令 | 真实结果 / 任务ID |
|---|---|---|
| E | `env -u TOMCAT_AGENT_ACTIVE -u TOMCAT_E2E_GREP TOMCAT_E2E_SCREENSHOT=1 npm run test:e2e:webview-devhost` | 退出 0，33 passing / 0 failing / 4 场景专属 pending；`1788852478445-29djq7`。pending 仍不计通过，启动专属场景见上文 verify 五窗口。 |
| E | `npm run lint:extension && npx tsc -p e2e-harness/tsconfig.json --noEmit && env -u TOMCAT_AGENT_ACTIVE -u TOMCAT_E2E_GREP TOMCAT_ACCEPT_SKIP_BUILD=1 npx tsx scripts/run-vscode-manual-acceptance.ts` | 退出 0；类型检查通过，manual 2 passing / 0 failing / 0 pending（`captures real-host webview acceptance artifacts`、`renders completed plan with code review and green build rows`）；`1788852722406-euy6uq`。 |
| E | `env -u TOMCAT_AGENT_ACTIVE -u TOMCAT_E2E_GREP TOMCAT_ACCEPT_SKIP_BUILD=1 TOMCAT_IMAGE_ACCEPT_ARTIFACTS_DIR=/Users/yankeben/.tomcat/temp/remediation-image npm run accept:image` | 退出 0，1 passing / 0 failing / 0 pending，实际执行 `captures real-host image attachment artifacts`；`1788852325707-a9ralt`。交接文字把该任务误记为 manual 重跑，现按原始 command 更正。 |

manual/image 的 skip-build 仍通过内容哈希验证；本轮 manual 改动只在 harness 测试文件，harness 每次另行编译，不影响此前已通过的产品构建/安装/verify/image。前文 Rust 377、扩展 core 409/gui 572/集成 143、安装版 19、verify 五场景 10 的对应阶段仍有效，未把原失败的 `gate:full` 日志改成成功，也未反复运行已覆盖阶段。

### 真实窗口取证及限制

- manual 根目录 `/var/folders/1t/xdx9bpn14bb365np42rb36rw0000gn/T/tomcat-manual-artifacts-N3miET/`。`screenshots/` 有 24 PNG / 24 ARIA / 24 console；逐份解析结构和 console，0 error/exception，包含历史 `Searched files`、展开思考与工具、模型/effort、图片预览。目视核对历史/滚动/思考/工具/宽窄 composer、模型能力错误与重启恢复关键 PNG。报告保留原有 composer 测量限制：控件行数和单行排版不符合旧像素预期；工具长 JSON 在窄侧栏显得拥挤。第二个 completed-plan 用例有真实断言日志，不捏造其不存在的专属 PNG。
- image 根目录 `/Users/yankeben/.tomcat/temp/remediation-image/run-UZdHBm/`。23 组 PNG/ARIA/console 已读取：单图/11 图窄栏、占位加载、键盘焦点、重启草稿、深/浅/高对比主题、复制状态、缩放、历史恢复、SVG 第 11 张、PDF、行内图及降级提示可见。ARIA 有 `Image Preview`、`Copied`、`This attachment is no longer available`、PDF 警告及降级文案。13/14/15 三份 console 各含同一条越界图片请求的 403，来自测试明确拒绝加载工作区外图片的预期负例；其他 20 份未记录 error/exception，缺失附件警告也属主动移除字节的用例。**不宣称整段 console 全空或将原始预期错误删去。** 截图是通过测试窗口 CDP 捕获；报告旧 limitations 仍写 macOS screencapture，和本轮实现不符，以实际捕获代码及全帧证据为准。系统 Save As 对话框仍未自动验收；320px 使用原有测试宽度 shim；源图全量内存数字是算术估算而非对照实测。未更改这些产品行为或放宽限制。
- devhost 根目录 `/var/folders/1t/xdx9bpn14bb365np42rb36rw0000gn/T/tomcat-vscode-runs/gui-innpKH/`。22 PNG，其中 18 组具有 ARIA/console；另 4 张 workbench-find 图无配套三件套，不作完整取证计数。18 份 console 均未记录 error/exception；ARIA 含重试/恢复、引用、分组工具、plan、模型控件等结构。关键截图已目视检查；长标题省略与侧栏拥挤仍如前述，不称像素级零问题。
- 上述 CDP console 为截图时刻缓存，而非从窗口启动起全程追踪。用于展示 PNG 的本地图片浏览页若请求 favicon/误拼文件产生 404，不属于被测 webview；不把目录页或解码失败的浏览图当验收图。

### 最后静态核对与工作区标识

- `1788852901977-6kqhd8`（E）：`env -u TOMCAT_AGENT_ACTIVE npm run check:wire && node ../scripts/release-version.mjs check && git diff --check && git diff --exit-code -- ../.agents ../.cursor/commands ../release-versions.json && npx tsc --ignoreConfig --noEmit --strict --types node --target ES2022 --module ESNext --moduleResolution Bundler --esModuleInterop --skipLibCheck scripts/buildArtifacts.ts scripts/vscodeLaunchEnv.ts scripts/run-vscode-verify-vsix.ts scripts/run-vscode-install-e2e.ts scripts/run-vscode-devhost.ts scripts/run-vscode-manual-acceptance.ts scripts/run-vscode-image-acceptance.ts`，退出 0；wire 最新，CLI 0.1.47/扩展 0.1.61/bundled CLI 0.1.47。
- `1788853125783-akwyty`（E）：用 TypeScript AST 读取四个 Mocha 入口的 options，逐个断言 `failZero=true`，执行空 suite 并断言回调 failures=1；四项通过。日志中的四次 `0 passing` 是**预期拒绝空集的负例**，不是 GUI 验收通过数。随后 `npx tsx -e 'import {assertBuildArtifacts} from "./scripts/buildArtifacts";assertBuildArtifacts(process.cwd());console.log("Current build inputs/outputs hashes verified")'` 通过。
- 工作树 47 tracked 修改 + 11 untracked 源码/测试/报告，共 58 个路径，无删除文件、无意外文件模式变化。已核对全量路径/stat 及主要 diff；未暂存/提交/回退其他改动。候选变更中没有 `.DS_Store`、VSIX、log、tmp-*、release-v* 或 test-stuff；`.agents/`、Cursor commands 和版本清单未改。既有忽略规则下的测试副产物没有入库，两个既有 scratch 目录在 Git 跟踪和未跟踪候选文件列表中均无条目。
- HEAD 保持 `1cc07c8b41991f13b33643d5692d8d849f1aadca`；对排序后的全部变更路径及字节（排除本账本）计算 SHA256：`fabe312fca556e9e8ec2adca5b4d3781012039a207f58cf633098034963920f6`。构建记录 `.build-artifacts.json` SHA256：`5e163f71fbe748a966a60482229ab92bff33f42f53fa130e184a010ab487b367`。这是工作树/已构建产物标识，不冒充新的提交 SHA。
- `1788853061377-ukb7r7`（R）在最后代码改动后重新跑 verify skill 截图：`node /Users/yankeben/.tomcat/skills/verify/scripts/shot.mjs http://127.0.0.1:5199/ --out /Users/yankeben/workspace/tomcat-agent/.tomcat/shots/remediation --name final-desktop --viewport 1440x900`，接着同命令 `--name final-narrow --viewport 390x844`，随后分别 `curl --max-time 10 --fail --silent --output /dev/null` 检查 `/src/main.tsx`、`/src/styles.css`、`/favicon.svg`。整链退出 0，资源均 200。两份 ARIA 均为未接 VS Code 的 Connecting/disabled 控件，两份 console 无 error/exception；PNG 仅证明独立加载/宽窄边界，不替代真实 host 交互。

以上是全部必验批次及保留的范围外/画面限制。运行时 review/Acceptance 回执由计划工具产生，禁止手改状态或用账本文字替代回执。
