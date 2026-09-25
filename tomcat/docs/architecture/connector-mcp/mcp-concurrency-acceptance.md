# MCP 并发与 Reload：验收入口及现场记录口径

## 证据边界

```text
真实 Serve（固定一个 Agent）
  会话 A ── tool_call / tool_run_code ─┐
                                     ├─ 同一 Workspace MCP 来源：缺省 16
  会话 B ── tool_call ────────────────┘
             ↓                                  ↓
  带 sessionId 的真实输出事件           服务端闸门、请求 ID、执行/取消计数
```

- 本文记录本轮新增验收入口，不替代 SDK 升级历史记录，也不修改旧人工验收手册。
- 两个真实 Serve 会话、两个 manager future、两个独立 Serve 进程是三种不同证据，不能互相冒充。
- 当前 Serve 的 Agent 由进程配置决定，`new_session` 不接受 Agent 选择。本轮验收不把系统提示词差异或多 Agent Serve 当作测试维度：共享机制只由同一有效来源、工作区与请求身份决定；真实双会话的客户端共享、默认额度、请求归属和停止隔离已经单列验证。
- 安装态采集的开发迭代可以使用当前 debug CLI；最终批次须使用计划指定的 release CLI，并保存实际路径和哈希。外网 DeepWiki 现场独立记录。

## 已完成的真实双会话定向检查

`tomcat/tests/serve_multi_session.rs` 在 `test-streamable-http-server` feature 下引入 `tomcat/tests/support/serve_mcp_multi_session.rs`。它启动真实 CLI、按会话内容路由的脚本 LLM 和现有 HTTP MCP 夹具，不注入假的 Serve 工具结果。

| 用例 | 实际约束 |
| --- | --- |
| 同名工具、延迟响应头 | A 被服务端闸门挡住；B 在放行 A 前完成；随后 interrupt A，不影响 B 的另一笔已发请求 |
| 不同工具、SSE | 同样证明重叠；close_session A 不使 B 失去连接 |
| JSON、默认额度 | A 的真实 `tool_run_code` 并行占用 15 个名额，B 使用第 16 个；配置没有 `maxConcurrentCalls` 字段 |
| 归属与资源 | A/B 外层 `toolCallId` 相同，MCP request ID 不同；结果事件只进入 B；只握手一次，无隐式重发，响应退出且 Serve/夹具结束 |

复现：

```sh
# 仓库根；首次会编译真实 CLI 和受控 HTTP fixture
cargo test --manifest-path tomcat/Cargo.toml --locked \
  --features test-streamable-http-server --test serve_multi_session \
  -- --nocapture --test-threads=1
cargo test --manifest-path tomcat/Cargo.toml --locked \
  --features test-streamable-http-server --lib api::chat::context::tests \
  -- --test-threads=1
```

本轮 `task:1790078983402-ebqn6k` 退出码 0：Serve 5/5（含原有两项），上下文 8/8。

这一链路还发现并修复了 scope 缓存问题：相同资源目录不意味着相同 MCP 配置边界。无显式项目根的默认会话，不得把自己的全局-only registry 交给有项目根的会话。`tomcat/src/api/chat/context.rs` 的缓存身份现在同时包含资源目录与显式项目根。**历史记录**：本轮采集时 Workspace 用例曾用逐服务批准；当前接口已改成 `trust_project`，不能把这份旧证据当成新项目信任流程的验收。

## 安装态采集设施

入口：`tomcat-vscode-ext/scripts/run-vscode-connectors-acceptance.ts`。

```text
构建 / 打包 / 安装 VSIX → 校验安装产物哈希 → 固定实际 CLI
   ├─ 独立自检 profile：Settings 注入 console.error → 检查器必须拒绝
   └─ 全新正常 profile：真实 Serve + HTTP/stdio 夹具
          → Reload 处理中 / 成功 / 失败 / 自动恢复
          → 每态 Settings PNG + ARIA + console
```

- `SettingsFrameDriver` 依据 Settings 的标题、DOM、入口脚本和 CDP execution context 定位，而不是查询聊天 webview 的错误列表。
- PNG 从该 Settings webview 的可见区域采集；ARIA 来自其实际 frame；console 记录声明 capture-time 缓冲及连接后的采集范围，不冒充全过程追踪。
- HTTP 夹具能挡住 initialize、改变工具目录和连续返回 503；stdio 夹具通过退出闸门真实结束一次进程，并挡住第二次 initialize。
- 控制 MCP、用户数据、HOME 和 Workspace 属于独立测试实例；不使用真实 provider key，不覆盖用户已安装 CLI/扩展。
- 结果保留在 runner 打印的目录，默认 `.agents/shots/connectors-installed-*`。可通过 `TOMCAT_CONNECTORS_ACCEPT_ARTIFACTS_DIR` 指定本项目配置的资源目录。
- `artifact-hashes.json` 核对 CLI、HTTP fixture、VSIX、安装的 extension.js 以及 GUI JS/CSS 与工作区产物；`mcp-fixture-process.json` 记录受控服务退出。
- `.aria.txt` 是带 frame 身份的 AX 树和 DOM 摘要，不是冒充 ARIA 的 HTML。故意错误自检与正常案例的证据目录完全分开。
- 采集失败、图片未读取、只有 DOM 测试通过，都不算视觉验收通过。

最终命令（Rust 构建与扩展 gen:wire 串行）：

```sh
# 仓库根
cargo build --manifest-path tomcat/Cargo.toml --release --bin tomcat
cargo build --manifest-path tomcat/Cargo.toml --features test-streamable-http-server \
  --bin test_streamable_http_server

# tomcat-vscode-ext 目录
TOMCAT_CONNECTORS_ACCEPT_BINARY="$(cd ../tomcat && pwd)/target/release/tomcat" \
  npm run accept:connectors
```

自定义 Cargo 输出目录时显式设置 `TOMCAT_CONNECTORS_ACCEPT_BINARY` 和 `TOMCAT_CONNECTORS_ACCEPT_MCP_BINARY`，不要猜测路径。改动构建输入后不能强行跳过构建；产物检查器会拒绝过期指纹。

**开发定向轮次已通过（debug CLI，不冒充最终 release）：** `task:1790083893951-ilq5t2` 退出码 0：Host 34/34，检查器自检 1/1，正常安装态 2/2。证据目录 `.agents/shots/connectors-installed-Z1pQ2m/`，`acceptance-result.json` 两阶段均 passed；13 份正常 Settings console 均无错误，AX 根为 Tomcat Settings，frame/context 身份与 console 匹配。

已实际读取桌面 Reload 处理中、成功、新目录、失败以及自动恢复 PNG，和 382×752 的失败弹窗/卡片 PNG：长错误与配置路径折行，按钮完整，窄窗口导航改为顶部排列，卡片不再挤成逐字换行。原生 Enter 已验证 Done 和 Reload；后续收紧了自动恢复后的目录就绪等待，并加入键盘 Remove，留给最终批次验证。

迭代中区分了几类问题：过长临时 profile 的 Unix socket 路径已缩短；macOS Keychain 初始化停顿通过测试实例的 in-memory/mock secret storage 避开，不改用户密钥设置；CDP Enter 缺少字符事件已修正；重复打开 Settings 的旧页面等待模型/密钥发现才切换，现由 Host 同步发布路由并有单测。HTTP 503 后三次停止是正确终态，不改成无界重试来迎合测试。受控 HTTP 服务 PID 51276 的退出信号 SIGTERM 已记录；不据此宣称其他历史进程的归属或清理情况。

Rust release 已构建成功；最终 release 安装态与真实 DeepWiki 双会话现场均已通过，详见下节。本轮可执行范围的缺口已经复测；系统提示词差异不是客户端复用、额度或请求隔离的输入，因此不另设角色专项门槛。完整 Rust 门禁仍保留两项基线失败，不宣称全部全绿。

## 最终 diff 审查（当前可执行范围已完成）

- 复核依赖锁定、两层额度、请求/连接所有权、进度窗口、连接事件与恢复预算、撤权/凭据提交边界、Host 回执及目录身份。`tomcat/src/core/connector/mcp/call.rs:272` 使用 SDK 进度续期且无总上限；`tomcat/src/core/connector/mcp/transport.rs:339` 对齐 HTTP 额度并关闭 session 重发；`tomcat/src/core/connector/mcp/manager.rs:872-959` 仅凭明确连接事实恢复，不以普通调用错误断开。
- 核实 stdio 的本地取消不依赖服务器回答：rmcp 3.4.0 `service.rs:1477-1495` 在取消通知发送完成（包括发送失败）后移除对应 pending 并完成本地响应；Tomcat 的发送适配器限制取消投递等待，再让响应 future 正常退出、移除进度观察者。不能据此宣称远端业务一定已停止。
- 确认一个 P1：工具目录读取失败/返回旧身份后，Host 的去重标记未释放，会导致同一连接一直显示 Loading tools。修正为既有轮询重读，持续旧身份按普通 5 秒节奏，不热循环、不 Reload、不重发工具；新增临时失败、旧目录与持续旧目录三项回归。
- `task:1790085256513-6xx9gu` 退出 0：Host 37/37（含上述三项新回归）及扩展类型检查通过。`task:1790085299981-i5915r` 的首次 Clippy 报本轮 6 项规范错误，随后主动停止尚未完成的后续批次（exit=-1，不计完整门禁）。已分组调用计时参数、等价简化 Option 判断并把测试模块移至末尾；`task:1790086029274-qulbyn` 的 Clippy 子阶段已通过，完整批次结果见下节，最终退出 1，并非全绿。
- 完整 lib 阶段发现旧 scope 共享测试把第二个会话仅写入 cwd，实际 `new_current_session(cwd)` 的 project_root 为 None（`tomcat/src/core/session/manager/session_impl.rs:838-849`），与第一会话的显式项目绑定不同。没有放宽生产隔离：两项同 scope 夹具改用 `new_current_session_with_project_root`；另外断言同 cwd 的未绑定会话不共享容器，并把另一用例从“工具名相同”加强为 registry 指针相同。后续该模块 33/33 通过，不把首轮失败抹去。
- E2E 增补了共用响应式外壳的 Models 窄窗口采集；首次 TS2339 是测试 API 的局部类型漏写 models，补齐后 `task:1790087261852-ius4gm` 退出 0（E2E 编译与 diff 空白检查）。隔离 DeepWiki 探针的 Node 语法检查已通过，后续现场通过结果见文末。
- 进程核查修正：PID 13911 是本会话工具 shell 的父进程（当前 Serve），不是可清理泄漏，未终止；PID 11904 为已存在约 9 天的孤儿 HTTP 测试服务，无法证明属于本轮，未擅自终止。两个本轮旧截图服务任务已停止。
- 已有真实双会话共享连接证据继续有效；系统提示词只影响模型选择工具和参数，不是共享客户端、默认额度、请求归属或停止隔离的键，因此不再要求额外角色组合。未扩大 Serve 为多 Agent，也未混入工作区其他未提交文档。最终命令和当前失败分类在本节后续补记。

## L3 实际结果与失败分类

`task:1790086029274-qulbyn` 最终退出 1，按项目脚本的 clippy / lib / doctest / integration / integration-real-llm 逐批执行，失败不会掩盖后续检查：

| 阶段 | 本轮结果 |
| --- | --- |
| Clippy | 通过（修复最初 6 项后） |
| lib | 2914 通过，2 失败，2 ignored；耗时 1463.63 秒 |
| doctest | 退出 0，当前 0 个文档测试，不冒充额外覆盖 |
| integration parallel | 357 通过，2 失败，26 按分类留给 serial |
| integration serial | 26/26 通过 |
| integration-real-llm | 29/29 通过，真实调用，不是缺凭据跳过 |
| release CLI / HTTP fixture | 两个构建均退出 0 |

四个失败分别处置：
1. 本轮 scope 身份修复暴露旧夹具的错误前提：已修正并增强两项同 scope 测试，见下述 33/33 定向补验。
2. 本轮新增回执/目录 schema 漏更新 Rust golden：`task:1790090030113-6neabo` 退出 0，使用已构建 CLI 的 `serve --print-schema` 官方生成器、隔离 HOME 更新 `tomcat/tests/fixtures/serve/serve.{schema.json,d.ts}`；diff 仅增加 MCP 相关 112/24 行，golden 复测 1/1 通过。
3. **基线失败，未改动：** `completion_flow_test.rs:58` 要求 planner 含 `human-readable “验收” section`，但 HEAD 的 `planner.txt` 已不含该串；两文件与 HEAD 相同，已用 Git 对象核验，并非 MCP 改动引入。
4. **环境触发的既有测试缺陷，未改动：** `integration_gate_config_tests.rs:168-177` 只清 OPENAI/LITELLM 两个 key；默认 target 已是 idatatlas，当前环境存在该 key，因此“缺密钥”用例误跑真请求，得到 1 而非预期 2。测试与脚本均与 HEAD 相同；只核实变量存在性，没有输出密钥。真正 real-llm 批次 29/29 已通过，不能用它掩盖此 preflight 测试失败。

定向补验：`task:1790090536362-udkhki` 退出 0：Clippy 通过、runtime_split_test owner 模块 33/33、serve_schema_fixture 1/1、diff 检查通过。本轮暴露的 scope 夹具与 golden 遗漏已关闭；不重跑其余仍有效的 24 分钟 lib 全集，两项基线失败仍单列。

## 扩展门禁与最终安装态迭代

`task:1790090828756-y77agw` 完整执行扩展 gate:full / check:wire / release accept:connectors，最终退出 1：core 478/478、GUI 601/601、integration 150/150，常规安装态 19 passing/5 场景条件 pending、verify:vsix 各场景及 wire 校验均通过。VSIX 旧裁图脚本提示 5 张 full-frame 不存在，未把这些裁图报成视觉证据；真实 Settings 安装态 1 passing/1 failing，另一个 pending 的检查器自测已在独立 profile 1/1 通过。

失败证据 `.agents/shots/connectors-installed-AgxMaA/normal/reload-failure-state.json:654-704`：stdio 已 Connected/attempt=2，但 Host selectedConnector 仍指向之前 HTTP 来源，目录为空。确认 UI `openDetail` 在 connecting 时没有发送选择意图，Host 无法跟踪新详情；与前述“读目录暂时失败”的问题不同。已改成打开未覆盖的来源就通知 Host，Host 原有 guard 仍将真正目录 RPC 延后至 Connected；新增 GUI/Host 两端回归验证切换来源和恢复后自动读目录，无额外 Reload。

`task:1790091689334-mivacq` 退出 0：Host 38/38、Settings GUI 39/39、扩展与 GUI lint、重新构建/打包/安装的 release 自检 1/1 与正常 2/2、diff 检查通过。正常 profile 的 1 pending 是已单独跑过的检查器自测，不算漏测。证据 `.agents/shots/connectors-installed-tH0SUm/`，15 组正常 Settings PNG/ARIA/console 已读取：桌面 Reload 处理中/成功/失败、关闭重开、自动恢复到新目录、预算耗尽/显式 Reload、键盘 Remove，以及 382×752 的失败详情/卡片和 Models 页面；没有观察到空白、遮挡或按钮裁切，窄窗长错误和路径正常折行。AX/DOM 的禁用及 busy 状态匹配，15 份捕获范围内 errors=[]，frame/context 身份一致（不声称全过程监控）。`task:1790092019682-mye5yd`、`task:1790092099365-i7ki75` 退出 0，补核安装/工作区 JS/CSS 与 release CLI 哈希、frame/context 身份及后端计数；HTTP 活动响应/通知流归零、initialize=5，stdio initialize=6/运行退出1/启动失败3，受控 HTTP 进程 PID45691 已 SIGTERM 退出。其他已通过且未受这处选择逻辑修复影响的门禁不重复跑。

## DeepWiki 现场复测（已通过，同 Agent 双会话）

首轮 `task:1790090536461-aw4eqj` 退出 1，证据 `.agents/shots/deepwiki-field-20260922-232216/`：release CLI SHA256 `eb3c55b7e875922e2064c2e64672f806e4f2ef93da79d092062c13315cc98a44`，真实 DeepWiki 2.14.3 已成功初始化并列出 3 个工具。但现场目录的问答工具已改名为 `ask_wiki_question`，官方网页与旧计划记录仍写 `ask_question`，硬编码探针因此在本地返回 unknown tool、没有发出 tools/call。不是传输超时，Serve 正常退出 0；该轮不计现场通过。探针已改为从当前 Serve 目录取 rawName/modelName，并核对实际 MCP tools/list 的参数 schema，后续按实时名称调用。


当前轮次 `task:1790091080063-qbe4zw` 退出 0，证据 `.agents/shots/deepwiki-field-20260922-233120/field-result.json`、`requests.json`、`frames.jsonl`。使用相同 release 哈希、隔离配置与脚本 LLM（不消耗真实模型密钥），MCP 响应来自真实公开 DeepWiki；观察代理不捏造或延迟响应。下表为相对启动时刻的实际上游发送/响应结束时间（毫秒）：

| 组合（均为同一 Agent 的 A/B 会话） | A 发送 → 结束 | B 发送 → 结束 | 结果 |
| --- | --- | --- | --- |
| 问答 + 问答 | 7788 → 20481（requestId=3） | 9659 → 21716（requestId=4） | B 在 A 未结束时已发出；各得自己的答案 |
| 问答 + wiki 目录 | 21939 → 34704（requestId=5） | 22026 → 25373（requestId=6） | 请求重叠且 B 先结束；没有按返回顺序错配 |

四次外层 toolCallId 特意使用相同值 `same-external-id`，仍按 sessionId 和 MCP requestId 正确归属。两组结束前仅 1 次 initialize、同一 configKey/generation；DeepWiki 本轮未下发 Session ID（sessionHash=null），因此只声明共享 Tomcat 来源/客户端，不虚构服务端协议会话 ID。`same`/`different` 是同名/不同名工具，不是不同 Agent。

随后真实 Serve Reload 在 23ms 返回 accepted，实际观察 connecting → connected、新 generation=2 与匹配目录；总 initialize=2，tools/call 仍为 4，没有重放。Serve 正常退出 0。本轮没有制造公网断网，也没有用约 13 秒的现场响应代替虚拟时钟的 120 秒续期证明；系统提示词差异不构成额外的 MCP 基础设施验收变量。

### 手动复测步骤

1. 从独立终端/隔离 VS Code profile 启动匹配的构建，核对 runner 留下的版本与哈希。当前正在运行的旧 Serve 不会因源码更新而自动换成新实现。
2. 在隔离 profile 的主配置 `tomcat.config.toml` 中确认 `[connector.mcp] call_timeout_ms` 为默认 120000（或显式 120000）、`max_concurrent_calls` 为默认 16（或显式 16）。它们不属于 `mcp.json`，且每个来源分别拥有 16 个名额。不要为测试静默改写用户已有的显式覆盖。
3. 打开同一工作区的两个会话。两会话必须解析到同一有效来源（相同 scope/configKey），不要用两个 Serve 或两套独立客户端替代共享证明；系统提示词差异不是本轮额外变量。
4. 第一组：A/B 分别对同一公开仓库调用当前目录公布的问答工具（现场为 `ask_wiki_question`；旧名 `ask_question` 不能硬编码），使用不同且容易辨认的问题。第二组：A 调用该问答工具，B 调用 `read_wiki_structure`。按实时 schema 核对参数；本轮两者都有 `repoName`，问答另有 `question`。
5. 在 A 仍等待时发起 B，保留各自完整结果和 sessionId/外层 toolCallId；记录本地实际发送与结束时间。仅有两段 UI 动画不能证明请求已发送。
6. 再点击 Reload，确认先显示接收/恢复状态、终态后目录才可用。不要把 accepted 当作恢复成功。
7. 记录共享客户端/协议会话诊断；HTTP 并发可以有多个 TCP socket，不把 socket 数等同于 MCP 客户端数。缺少可用诊断时写“未验证”，不靠推测填表。

现场记录至少包含：

| 项目 | 待记录 |
| --- | --- |
| 构建 | CLI/扩展版本、SHA256、实际 CLI 路径 |
| 来源 | scope/configKey、显式/缺省额度、进度等待窗口 |
| 会话 | A/B 的 sessionId、相同 workspace 与 configKey |
| 请求 | 工具名、外层调用 ID、本地发送/结束时间、两份结果 |
| 连接 | generation/attempt 或可用的同等诊断；无法核实项明确留空 |
| 结论 | 本地阻塞、远端串行/限流、网络错误分别报告 |

小于 30 秒的现场成功不替代受控 35 秒长流测试；目录可见不等于现场成功。本轮已完成脚本 LLM 驱动的真实 DeepWiki 现场请求和 Serve Reload；未把它冒称为真实模型自主选工具的证明。系统提示词差异不改变 MCP 基础设施身份，故不另设角色组合验收；两项 L3 基线失败如上保留，不宣称完整门禁全绿。
