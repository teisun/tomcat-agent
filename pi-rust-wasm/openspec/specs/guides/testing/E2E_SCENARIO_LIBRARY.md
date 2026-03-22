# E2E 用户操作模拟场景库

> 本文件是 [E2E_TEST_SPEC.md](E2E_TEST_SPEC.md) §2 的规范性附件，列出覆盖全部 P0 User Stories 的用户操作模拟用例清单。新增用例须同步更新本文件。

## 用例编号规则


| 前缀           | 含义                                             |
| ------------ | ---------------------------------------------- |
| E2E-CLI-NNN  | CLI 子进程 E2E 用例（`tests/cli_tests.rs`）           |
| E2E-WASM-NNN | Wasm 运行时 E2E 用例（`tests/wasmedge_e2e_tests.rs`） |


---

## 验收方式列（人工 / 自动）

各 Story 表格中的 **验收** 列取值：

| 取值 | 含义 |
| --- | --- |
| 自动 | 以 `cargo test`（`cli_tests` / `wasmedge_e2e_tests`）通过为准即可。 |
| 人工 | 建议在**真实终端、本机环境**下再执行等价操作，补验观感、确认流、路径与依赖；与 [INTEGRATION_MERGE_AND_ACCEPTANCE.md](../../../../agents/INTEGRATION_MERGE_AND_ACCEPTANCE.md) §4「人工验收」及跨平台（Windows/macOS/Linux）要求配合使用。 |

**说明**：标为「人工」的用例**通常已有**自动化测试；该标记表示交付前仍建议人工过一遍，避免仅依赖子进程 E2E 的断言盲区。

---

## Story 1 — 宿主初始化与基础配置（6 条）


| 编号          | 验收 | 用例名                                          | 用户意图                                | 操作序列                                | 必须断言                                                               |
| ----------- | -- | -------------------------------------------- | ----------------------------------- | ----------------------------------- | ------------------------------------------------------------------ |
| E2E-CLI-001 | 自动 | `test_user_first_time_setup_init_and_doctor` | 新用户首次安装，完成初始化并验证环境健康                | `pi init` → `pi doctor`             | init exit 0 + stdout 含"已生成配置文件"；doctor exit 0 + stdout 含"✓"或"配置合法" |
| E2E-CLI-002 | 自动 | `test_user_sets_config_value`                | 用户修改日志级别                            | `pi config set log.level warn`      | exit 0                                                             |
| E2E-CLI-003 | 自动 | `test_user_views_full_config`                | 用户查看当前全部配置                          | `pi config get`                     | exit 0；stdout 含配置段关键字                                              |
| E2E-CLI-004 | 自动 | `test_user_exports_config_to_file`           | 用户导出配置备份                            | `pi config export /tmp/pi_cfg.toml` | exit 0；文件存在                                                        |
| E2E-CLI-005 | 自动 | `test_user_imports_config_from_file`         | 用户从备份恢复配置                           | `pi config import /tmp/pi_cfg.toml` | exit 0；stdout 含"导入"                                                |
| E2E-CLI-006 | 人工 | `test_user_doctor_detects_environment`       | 用户运行 doctor 检测 WasmEdge/QuickJS 可用性 | `pi doctor`                         | exit 0；stdout 含环境检测项                                               |


---

## Story 2 — 4 原语安全管控（通过 `pi chat` 驱动）（6 条）

> 需要 `OPENAI_API_KEY`；无 key 时必须 `panic!`，不得跳过。
> **验收**：本 Story 与 §4 人工验收「对话模式、四原语与用户确认」对齐，**整组建议人工补验**（流式观感、多轮、确认提示）。


| 编号          | 验收 | 用例名                                           | 用户意图                                | 操作序列                                                                                      | 必须断言                                                       |
| ----------- | -- | --------------------------------------------- | ----------------------------------- | ----------------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| E2E-CLI-011 | 人工 | `test_user_asks_pi_a_question`                | 用户向 pi 提问并收到回答                      | `pi chat` + stdin `你好，介绍一下你自己`，timeout 60s                                                | exit 0；stdout 非空；含 AI 回复文字                                 |
| E2E-CLI-012 | 人工 | `test_user_asks_pi_technical_question`        | 用户问技术问题，验证 LLM 回复质量                 | `pi chat` + stdin `什么是 Rust 的所有权系统`，timeout 60s                                           | exit 0；stdout 含"所有权"或"ownership"                           |
| E2E-CLI-013 | 人工 | `test_user_asks_pi_to_write_hello_world_bash` | 用户要求 pi 写一个可执行的 hello world bash 脚本 | `pi chat` + stdin `请帮我写一个 hello world 的 bash 脚本保存到 /tmp/hw.sh`，timeout 60s                | exit 0；stdout 含 bash 代码片段或操作确认；/tmp/hw.sh 存在或 stdout 含脚本内容 |
| E2E-CLI-014 | 人工 | `test_user_asks_pi_to_read_a_file`            | 用户要求 pi 读取指定文件内容                    | 预置 /tmp/test_read.txt → `pi chat` + stdin `请读取 /tmp/test_read.txt 的内容`，timeout 60s        | exit 0；stdout 含文件内容或读取确认                                   |
| E2E-CLI-015 | 人工 | `test_user_asks_pi_to_edit_a_file`            | 用户要求 pi 修改文件中的某行内容                  | 预置 /tmp/test_edit.txt → `pi chat` + stdin `请把 /tmp/test_edit.txt 第一行改成 hello`，timeout 60s | exit 0；修改后文件第一行为 hello                                     |
| E2E-CLI-016 | 人工 | `test_user_asks_pi_to_run_bash_command`       | 用户要求 pi 执行一条 bash 命令                | `pi chat` + stdin `请执行 echo hello_from_pi`，timeout 60s                                    | exit 0；stdout 含 hello_from_pi 或命令确认提示                      |


**已实现**：E2E-CLI-013 已实现于 `test_user_asks_pi_to_write_hello_world_bash`（工作区 workspace 下写 hello_e2e.txt）；E2E-CLI-016 已实现于 `test_user_asks_pi_to_run_bash_command`。014、015 待后续补充。

---

## Story 3 — WasmEdge + QuickJS 插件系统（6 条）

> **验收**：021–025 与 §4 人工验收「pi-mono 风格插件加载/卸载、错误隔离」对齐，建议在**本机真实插件路径**下补验；026 以自动化断言为主。


| 编号          | 验收 | 用例名                                                   | 用户意图              | 操作序列                                             | 必须断言                           |
| ----------- | -- | ----------------------------------------------------- | ----------------- | ------------------------------------------------ | ------------------------------ |
| E2E-CLI-021 | 人工 | `test_user_loads_plugin_and_lists`                    | 用户从路径加载插件并查看已加载列表 | `pi plugin load <plugin_dir>` → `pi plugin list` | load exit 0；list stdout 含插件 id |
| E2E-CLI-022 | 人工 | `test_user_views_plugin_info`                         | 用户查看插件详情（名称、版本）   | `pi plugin info <id>`                            | exit 0；stdout 含 name/version   |
| E2E-CLI-023 | 人工 | `test_user_disables_plugin`                           | 用户禁用插件            | `pi plugin disable <id>`                         | exit 0                         |
| E2E-CLI-024 | 人工 | `test_user_enables_plugin_after_disable`              | 用户重新启用被禁用的插件      | `pi plugin enable <id>`                          | exit 0                         |
| E2E-CLI-025 | 人工 | `test_user_unloads_plugin_removes_from_list`          | 用户卸载插件后从列表消失      | `pi plugin unload <id>` → `pi plugin list`       | unload exit 0；list 不含该 id      |
| E2E-CLI-026 | 自动 | `test_user_loads_nonexistent_plugin_path_shows_error` | 用户加载不存在路径时看到错误提示  | `pi plugin load /nonexistent/path`               | exit 0；stdout 含"不存在"或 error 信息 |


---

## Story 4 — Node.js 兼容层（Wasm E2E）（5 条）


| 编号           | 验收 | 用例名                                              | 用户意图                                           | 操作序列                                                  | 必须断言                                                           |
| ------------ | -- | ------------------------------------------------ | ---------------------------------------------- | ----------------------------------------------------- | -------------------------------------------------------------- |
| E2E-WASM-001 | 自动 | `test_wasmedge_e2e_hello_world_inline`           | 插件 JS 可执行内联脚本                                  | WasmEngine + `run_script("print('Hello World');")`    | Ok；无崩溃                                                         |
| E2E-WASM-002 | 自动 | `test_wasmedge_e2e_hello_world_script_file`      | 插件 JS 可执行 .js 文件                               | `run_script_file(hello.js)`                           | Ok                                                             |
| E2E-WASM-003 | 自动 | `test_wasmedge_e2e_bridge_layer`                 | 插件 JS 可通过 pi 桥接层调用全部 4 原语                      | `run_script_file(bridge_test.js)`                     | host_call 次数 ≥4（readFile/writeFile/editFile/executeBash 各 1 次） |
| E2E-WASM-004 | 自动 | `test_wasmedge_e2e_require_path_modules_preopen` | `require('path')` 可用（WASI `./modules` preopen） | `run_script_file(require_path_test.js)`               | Ok；`path.join('a','b')` 不抛错                                    |
| E2E-WASM-005 | 自动 | `test_wasmedge_e2e_tps_transpile_run_script_poc` | SWC 转译后 pi-mono 风格 tps 插件可加载                   | `transpile_pi_plugin_for_quickjs` + `run_script_file` | Ok；不崩溃                                                         |


---

## Story 5 — 宿主工具注册（2 条）


| 编号           | 验收 | 用例名                                                 | 用户意图                                       | 操作序列                                               | 必须断言                                         |
| ------------ | -- | --------------------------------------------------- | ------------------------------------------ | -------------------------------------------------- | -------------------------------------------- |
| E2E-WASM-011 | 自动 | `test_wasmedge_e2e_tool_registration`               | 插件 JS 通过 registerTool 注册工具后宿主可感知 host_call | `run_script_file(tool_register_test.js)`           | host_call 中 method=registerTool 至少触发 1 次；无崩溃 |
| E2E-CLI-031  | 人工 | `test_user_tool_registered_by_plugin_can_be_called` | 插件注册的工具可被 chat 调用（需 OPENAI_API_KEY）        | load_plugin + `pi chat` + 触发工具的 prompt，timeout 60s | stdout 含工具执行结果或调用确认                          |


---

## Story 6 — 事件系统（Wasm E2E）（3 条）


| 编号           | 验收 | 用例名                                               | 用户意图                                           | 操作序列                                                                     | 必须断言                                                            |
| ------------ | -- | ------------------------------------------------- | ---------------------------------------------- | ------------------------------------------------------------------------ | --------------------------------------------------------------- |
| E2E-WASM-021 | 自动 | `test_wasmedge_e2e_event_dispatch`                | 宿主分发事件后插件 JS handler 被触发，ctx 全部方法均触发 host_call | `dispatch_event(event_dispatch_test.js, "test_event", ...)`              | host_call 次数 ≥8                                                 |
| E2E-WASM-022 | 自动 | `test_wasmedge_e2e_event_once_fires_exactly_once` | 事件 once handler 可通过 dispatch_event 触发          | `dispatch_event(event_once_test.js, "__e2e_once_event", ...)` 一次         | log host_call 计数 ≥1；注：MVP 无状态执行模型下「恰好 1 次」保证需 Story 8b（P1）实现后补充 |
| E2E-WASM-023 | 自动 | `test_wasmedge_e2e_event_on_multiple_handlers`    | 多个 on 监听同一事件均被触发                               | `run_script_file(event_multi_handler_test.js)`（pi.on 注册 h1/h2 + emit 一次） | log host_call 计数 ≥2（h1、h2 各触发一次）                                |


---

## Story 7 — LLM 统一接入（2 条）

> 需要 `OPENAI_API_KEY`；无 key 时必须 `panic!`。
> **验收**：与 §4 人工验收「流式输出、对话模式观感」对齐，建议**人工补验**终端流式表现。


| 编号          | 验收 | 用例名                                                | 用户意图                 | 操作序列                                 | 必须断言                             |
| ----------- | -- | -------------------------------------------------- | -------------------- | ------------------------------------ | -------------------------------- |
| E2E-CLI-041 | 人工 | `test_user_chats_with_llm_gets_streaming_response` | 用户与 LLM 对话，获得流式渲染回复  | `pi chat` + stdin 一句话，timeout 60s    | exit 0；stdout 含 AI 回复；有对话 banner |
| E2E-CLI-042 | 人工 | `test_user_receives_nonempty_llm_response`         | 确认 LLM 回复内容非空（基础连通性） | `pi chat` + stdin `说一个字`，timeout 30s | exit 0；stdout 非空                 |


---

## Story 8 — CLI 对话与会话管理（11 条）

> **验收**：会话与审计子命令以自动化为主；058 涉及 chat 失败路径，可与 §4「对话模式」人工清单一并 spot-check。


| 编号          | 验收 | 用例名                                                     | 用户意图                      | 操作序列                                                       | 必须断言                            |
| ----------- | -- | ------------------------------------------------------- | ------------------------- | ---------------------------------------------------------- | ------------------------------- |
| E2E-CLI-051 | 自动 | `test_user_creates_new_session`                         | 用户创建一个新会话                 | `pi session new`                                           | exit 0；stdout 含"已创建会话"          |
| E2E-CLI-052 | 自动 | `test_user_lists_sessions`                              | 用户查看所有会话                  | `pi session list`                                          | exit 0                          |
| E2E-CLI-053 | 自动 | `test_user_switches_to_existing_session`                | 用户切换到已存在的会话               | `pi session new` → `pi session switch agent:default:main`  | exit 0                          |
| E2E-CLI-054 | 自动 | `test_user_switches_to_nonexistent_session_shows_error` | 用户切换到不存在会话时看到友好提示         | `pi session switch nonexistent-key`                        | exit 0；stdout 含"不存在"            |
| E2E-CLI-055 | 自动 | `test_user_deletes_session`                             | 用户删除刚创建的会话                | `pi session new` → `pi session delete agent:default:main`  | exit 0；stdout 含"已删除"            |
| E2E-CLI-056 | 自动 | `test_user_archives_session`                            | 用户归档会话                    | `pi session new` → `pi session archive agent:default:main` | exit 0；stdout 含"已归档"            |
| E2E-CLI-057 | 自动 | `test_user_searches_sessions_by_keyword`                | 用户按关键词搜索会话                | `pi session search default`                                | exit 0                          |
| E2E-CLI-058 | 人工 | `test_user_chat_without_api_key_fails_gracefully`       | 无 API key 时 chat 快速失败，不挂起 | `pi chat`（移除 OPENAI_API_KEY），timeout 5s                    | 进程 5s 内结束；stdout 或 stderr 含错误提示 |
| E2E-CLI-059 | 自动 | `test_user_views_audit_list`                            | 用户查看操作审计记录列表              | `pi audit list`                                            | exit 0                          |
| E2E-CLI-060 | 自动 | `test_user_exports_audit_to_file`                       | 用户导出审计记录到文件               | `pi audit export /tmp/audit.json`                          | exit 0；文件存在                     |
| E2E-CLI-061 | 自动 | `test_user_views_audit_show_invalid_id`                 | 用户查看不存在的审计条目时友好提示         | `pi audit show 9999999`                                    | exit 0；不 panic                  |


---

## Story 8b — 长生命周期 VM 与有状态插件（TASK-15 + TASK-05b/c Tier1–2，8 条）

> Wasm 真实运行时 E2E 用例（`tests/wasmedge_e2e_tests.rs`）。须安装 WasmEdge。
> **验收**：031–035 以 `wasmedge_e2e_tests` 自动化为准；036–038 与 pi-mono 兼容矩阵相关，建议**人工补验**本机 WasmEdge 与真实扩展抽样。


| 编号           | 验收 | 用例名                                                       | 用户意图                 | 操作序列                                                                                                                                       | 必须断言                                                                        |
| ------------ | -- | --------------------------------------------------------- | -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------- |
| E2E-WASM-031 | 自动 | `test_wasmedge_e2e_vm_actor_state_persists_across_events` | 插件全局变量跨事件保持          | start_session_vm → dispatch_session_event x2 → 检查 host_call 中的累加值                                                                          | 第二次事件的 host_call 反映累加状态；无崩溃                                                 |
| E2E-WASM-032 | 自动 | `test_wasmedge_e2e_handler_stays_registered`              | 已注册 handler 多次事件持续有效 | start_session_vm → dispatch_session_event("evt") x2                                                                                        | 每次 dispatch 均触发 handler（host_call 计数递增）                                     |
| E2E-WASM-033 | 自动 | `test_wasmedge_e2e_set_interval_runs_during_session`      | 会话期间周期性日志（定时器语义）稳定触发 | start_session_vm；fixture 用 `setTimeout` 链模拟周期（wasmedge_quickjs 对全局 `setInterval` 不稳定）；sleep ≥1.2s；断言 `VmActorState::Running`；`end_session` | 会话中 VM 仍为 Running；`pi.log` 侧可见多次 tick；`end_session` 后 RuntimeManager 为空、无悬挂 |
| E2E-WASM-034 | 自动 | `test_wasmedge_e2e_multi_session_isolation`               | 多会话上下文隔离             | start_session_vm(s1) + start_session_vm(s2) → 分别 dispatch → 验证各自 host_call                                                                 | s1 与 s2 的 host_call 各自独立、状态不串会话                                             |
| E2E-WASM-035 | 自动 | `test_wasmedge_e2e_session_end_no_hanging_threads`        | 关闭流程无悬挂线程            | start_session_vm → end_session → 检查 VmActorHandle 状态                                                                                       | end_session 后 RuntimeManager 为空；handle state 为 Stopped/Error                |
| E2E-WASM-036 | 人工 | `test_wasmedge_e2e_tps_tier1_agent_end_notify`            | pi-mono tps Tier1：零修改 TS 长生命周期 + notify | 临时插件 `main.ts`（fixture tps 源码）→ `start_session_vm` → `dispatch_session_event(agent_start)` → sleep → `dispatch_session_event(agent_end)`（含 assistant usage） | `with_ui_notify_counter` ≥1；`end_session` 后 RuntimeManager 为空                         |
| E2E-WASM-037 | 人工 | `test_wasmedge_e2e_tier2_compat_script`                   | TASK-05c Tier2：`registerCommand`+`__pi_invoke_command`、`registerTool`（schema 包装）、`ctx.ui` 等价 host、`executeBash`+args | `run_script_file(tier2_compat_test.js)` + stub host | 相关 host_call 次数 ≥7；脚本打印 done、无抛错 |
| E2E-WASM-038 | 人工 | `test_wasmedge_e2e_tier2_transpiled_export_default_plugin` | TASK-05c：社区风格 `export default function(pi)` TS 经 SWC 加载 + 命令注册与同步 invoke | 临时 `tier2_snippet.ts` → `run_script_file` + stub host | `registerCommand` host_call ≥1；脚本无抛错 |
| E2E-WASM-039 | 自动 | `test_wasmedge_e2e_tier3_diff_custom_ui` | TASK-05d Tier3：diff.ts 核心路径——registerCommand("diff") → exec("git") → ctx.ui.custom 渲染 Container/SelectList/Text | `run_script_file(tier3_diff_test.js)` + stub host（git status 返回固定 porcelain） | `executeBash` ≥1；`uiCustom` ≥1；脚本打印 done、无抛错 |
| E2E-WASM-040 | 自动 | `test_wasmedge_e2e_tier4_files_session_branch` | TASK-05d Tier4：files.ts 核心路径——registerCommand("files") → ctx.sessionManager.getBranch() → 空 session 降级 uiNotify | `run_script_file(tier4_files_test.js)` + stub host（getBranch 返回空数组） | `getBranch` ≥1；`uiNotify` ≥1；脚本打印 done、无抛错 |


---

## Story 9 — AgentLoop 核心结构（TASK-14，3 条）

> 需要 `OPENAI_API_KEY`；无 key 时必须 `panic!`（符合 INTEGRATION_TEST_SPEC §5.2）。
> **验收**：与 §4 人工验收「对话模式、多轮上下文、会话切换」对齐，建议**人工补验** AgentLoop 与终端交互体验。


| 编号          | 验收 | 用例名                                               | 用户意图                                         | 操作序列                                                                                      | 必须断言                                         |
| ----------- | -- | ------------------------------------------------- | -------------------------------------------- | ----------------------------------------------------------------------------------------- | -------------------------------------------- |
| E2E-CLI-081 | 人工 | `test_user_chat_non_interactive_with_prompt_flag` | 用户启动 `pi chat` 并输入单句提问，AgentLoop 执行并输出 AI 回复 | `pi init` → `pi chat`（stdin: `"Reply with exactly: pong\n"`，timeout 60s，含 OPENAI_API_KEY） | exit 0；stdout 非空（AI 已通过 AgentLoop::run() 回复） |
| E2E-CLI-082 | 人工 | `test_user_chat_resumes_last_session`             | 用户用 `--resume` 恢复上次会话，历史消息从 JSONL 加载         | `pi init` → `pi chat`（stdin 第一轮）→ `pi chat --resume`（stdin 第二轮，timeout 60s）               | exit 0；第二轮 stdout 非空；进程正常退出                  |
| E2E-CLI-083 | 人工 | `test_user_chat_multi_turn_context_retained`      | 用户进行两轮提问，第二轮 Agent 可引用第一轮答案（多轮上下文）           | `pi chat`（stdin: 两行问答，第二问引用第一问答案，timeout 90s）                                             | exit 0；stdout 包含第二问回复且非空                     |


---

## 边界与健壮性场景（跨 Story）（4 条）


| 编号          | 验收 | 用例名                                    | 用户意图                       | 操作序列                    | 必须断言                                                    |
| ----------- | -- | -------------------------------------- | -------------------------- | ----------------------- | ------------------------------------------------------- |
| E2E-CLI-071 | 自动 | `test_user_views_full_help`            | 用户查看帮助，所有子命令可见             | `pi --help`             | exit 0；stdout 含 init/doctor/config/session/plugin/audit |
| E2E-CLI-072 | 自动 | `test_user_views_version`              | 用户查看版本号                    | `pi --version`          | exit 0；stdout 含版本号字符串                                   |
| E2E-CLI-073 | 自动 | `test_user_runs_unknown_command`       | 用户输入错误命令时看到帮助              | `pi nonexistent_cmd`    | exit 非 0；stderr 含"error"                                |
| E2E-CLI-074 | 自动 | `test_user_init_then_doctor_roundtrip` | 用户 init 后 doctor 通过，完整引导流程 | `pi init` → `pi doctor` | 两步 exit 0；doctor 含"✓"                                   |


---

## 跨平台（无独立 E2E 编号）

与 [INTEGRATION_MERGE_AND_ACCEPTANCE.md](../../../../agents/INTEGRATION_MERGE_AND_ACCEPTANCE.md) §4 **人工验收第 8 条**一致：在 Windows / macOS / Linux 条件具备时，各至少执行一次 `cargo build` + `cargo test`（或 CI matrix）。**不占用**上表编号；发布前在 checklist 中单独勾选。


