# E2E 用户操作模拟场景库

> 本文件是 [E2E_TEST_SPEC.md](E2E_TEST_SPEC.md) §2 的规范性附件，列出覆盖全部 P0 User Stories 的用户操作模拟用例清单。新增用例须同步更新本文件。

## 用例编号规则


| 前缀           | 含义                                                         |
| ------------ | ---------------------------------------------------------- |
| E2E-CLI-NNN  | CLI 子进程 E2E 用例（`tests/cli_tests.rs`）                       |
| E2E-QJS-NNN  | rquickjs 插件运行时 E2E 用例（`tests/quickjs_e2e_tests.rs`）         |
| E2E-WASM-NNN | 历史 Wasm 兼容验收 / 可选验证（`tests/wasmedge_e2e_tests.rs`） |


---

## 验收方式列（人工 / 自动）

各 Story 表格中的 **验收** 列取值：

| 取值 | 含义 |
| --- | --- |
| 自动 | 以 `cargo test`（`cli_tests` / `quickjs_e2e_tests` / `wasmedge_e2e_tests`）通过为准即可。 |
| 人工 | 建议在**真实终端、本机环境**下再执行等价操作，补验观感、确认流、路径与依赖；与 [INTEGRATION_MERGE_AND_ACCEPTANCE.md](../agents/INTEGRATION_MERGE_AND_ACCEPTANCE.md) §4「人工验收」及跨平台（Windows/macOS/Linux）要求配合使用。 |

**说明**：标为「人工」的用例**通常已有**自动化测试；该标记表示交付前仍建议人工过一遍，避免仅依赖子进程 E2E 的断言盲区。

除非特别说明，本文对话入口统一写作 `tomcat code`；历史测试函数名中保留的 `chat` 仅为沿用既有命名，隐藏兼容别名 `tomcat chat -> tomcat code` 仍可用。

---

## Story 1 — 宿主初始化与基础配置（11 条）

> **变更（TASK-12 / TASK-16）**：原 E2E-CLI-004/005（`config export` / `import`）随子命令删除而作废；**E2E-CLI-004** 现为 `workspace add/list/remove`。**TASK-16**：`tomcat init` 为 `[1/3][2/3][3/3]`，PATH 自动写入 shell 配置；**E2E-CLI-001** 断言与 **E2E-CLI-005**（PATH 写入）、**E2E-CLI-017**（`workspace add --cwd`）、**E2E-CLI-010**（幂等提示）同步。

| 编号          | 验收 | 用例名                                          | 用户意图                                | 操作序列                                | 必须断言                                                               |
| ----------- | -- | -------------------------------------------- | ----------------------------------- | ----------------------------------- | ------------------------------------------------------------------ |
| E2E-CLI-001 | 自动 | `test_user_first_time_setup_init_and_doctor` | 新用户首次安装，完成初始化并验证环境健康                | `tomcat init` → `tomcat doctor`（`HOME`+`SHELL` 隔离）             | init exit 0 + stdout 含 `[1/3]` `[2/3]` `[3/3]` + `配置文件已写入` + `tomcat code` + `PATH`，且生成的 `tomcat.config.toml` 默认 `provider = "openai-responses"`、`default_model = "gpt-5.4"`、`api_key_env = "OPENAI_API_KEY"`；doctor exit 0 + stdout 含"配置合法"+"内嵌资源已就绪" |
| E2E-CLI-002 | 自动 | `test_user_sets_config_value`                | 用户修改日志级别                            | `tomcat config set log.level warn`      | exit 0                                                             |
| E2E-CLI-003 | 自动 | `test_user_views_full_config`                | 用户查看当前全部配置                          | `tomcat config get`                     | exit 0；stdout 含配置段关键字                                              |
| E2E-CLI-004 | 自动 | `test_workspace_add_list_remove_e2e`         | 用户授权工作区目录（add/list/remove，持久化 `tomcat.config.toml` `[workspace] workspace_roots`） | `tomcat init` → `tomcat workspace add <path>` → `tomcat workspace list` → `tomcat workspace remove`（`HOME` 隔离） | add/remove exit 0 且 stdout 含「已添加/已移除」；list 含路径；最终 list 提示无已授权工作区 |
| E2E-CLI-005 | 自动 | `test_init_auto_adds_path_to_shell_profile` | init 将 PATH 写入隔离 `HOME` 下 shell 配置 | `tomcat init`（`HOME`+`SHELL=/bin/zsh`） | `$HOME/.zshrc` 含 `# Added by tomcat init` 与 `export PATH=` |
| E2E-CLI-006 | 自动 | `test_user_doctor_detects_environment`       | 用户运行 doctor 检测 QuickJS/rquickjs 环境 | `tomcat doctor`                         | exit 0；stdout 含 rquickjs/配置/✓/内嵌资源/.env 检查项                       |
| E2E-CLI-007 | 自动 | `test_init_creates_env_file`                 | init 后配置文件包含 LLM 配置段并落到默认 provider 路径                | `tomcat init`                           | exit 0；config 文件存在且含 `[llm]`，并包含 `provider = "openai-responses"`、`default_model = "gpt-5.4"`、`api_key_env = "OPENAI_API_KEY"` |
| E2E-CLI-008 | 自动 | `test_init_creates_env_with_correct_permissions` | init 后 .env 权限为 0600（Unix）       | `tomcat init` → 检查 .env 权限              | .env 存在时 mode=0600                                                 |
| E2E-CLI-009 | 自动 | `test_doctor_reports_all_checks`             | doctor 输出含全部检查项                     | `tomcat init` → `tomcat doctor`             | exit 0；stdout 含 配置合法/内嵌资源/QuickJS wasm/rquickjs                   |
| E2E-CLI-010 | 自动 | `test_init_idempotent`                       | 连续两次 init，第二次以现有配置为基线继续运行 model-first 向导               | `tomcat init` × 2（同 `HOME`）           | 两次均 exit 0；第二次 stdout 含「已存在配置文件」或「已更新配置文件」 |
| E2E-CLI-017 | 自动 | `test_workspace_add_cwd_e2e`                 | `tomcat workspace add --cwd` 添加当前目录           | `tomcat init` → `cd` 至临时目录 → `tomcat workspace add --cwd` → `tomcat workspace list` | add exit 0；list 含该目录绝对路径 |

**补充（幂等 PATH）**：`test_init_path_export_idempotent_in_shell_profile` — 同一 `HOME` 下连续两次 `init`，`$HOME/.zshrc` 中仅一条 `export PATH=`。


---

## Story 2 — 4 原语安全管控（通过 `tomcat code` 驱动）（11 条）

> 需要 `OPENAI_API_KEY`；无 key 时必须 `panic!`，不得跳过。
> **验收**：本 Story 与 §4 人工验收「对话模式、四原语与用户确认」对齐，**整组建议人工补验**（流式观感、多轮、确认提示）。
> **T2-P0-004 follow-up**：E2E-CLI-018～022 补充 drag-deny / cwd runtime / 二进制错误 / Layer0 落盘路径场景；当前自动化由对应单测/集成测试锁定，真实终端交互仍建议人工 spot-check。


| 编号          | 验收 | 用例名                                           | 用户意图                                | 操作序列                                                                                      | 必须断言                                                       |
| ----------- | -- | --------------------------------------------- | ----------------------------------- | ----------------------------------------------------------------------------------------- | ---------------------------------------------------------- |
| E2E-CLI-011 | 人工 | `test_user_asks_pi_a_question`                | 用户向助手提问并收到回答                      | `tomcat code` + stdin `你好，介绍一下你自己`，timeout 60s                                                | exit 0；stdout 非空；含 AI 回复文字                                 |
| E2E-CLI-012 | 人工 | `test_user_asks_pi_technical_question`        | 用户问技术问题，验证 LLM 回复质量                 | `tomcat code` + stdin `什么是 Rust 的所有权系统`，timeout 60s                                           | exit 0；stdout 含"所有权"或"ownership"                           |
| E2E-CLI-013 | 人工 | `test_user_asks_pi_to_write_hello_world_bash` | 用户要求助手写一个可执行的 hello world bash 脚本 | `tomcat code` + stdin `请帮我写一个 hello world 的 bash 脚本保存到 /tmp/hw.sh`，timeout 60s                | exit 0；stdout 含 bash 代码片段或操作确认；/tmp/hw.sh 存在或 stdout 含脚本内容 |
| E2E-CLI-014 | 人工 | `test_user_asks_pi_to_read_a_file`            | 用户要求助手读取指定文件内容                    | 预置 /tmp/test_read.txt → `tomcat code` + stdin `请读取 /tmp/test_read.txt 的内容`，timeout 60s        | exit 0；stdout 含文件内容或读取确认                                   |
| E2E-CLI-015 | 人工 | `test_user_asks_pi_to_edit_a_file`            | 用户要求助手修改文件中的某行内容                  | 预置 /tmp/test_edit.txt → `tomcat code` + stdin `请把 /tmp/test_edit.txt 第一行改成 hello`，timeout 60s | exit 0；修改后文件第一行为 hello                                     |
| E2E-CLI-016 | 人工 | `test_user_asks_pi_to_run_bash_command`       | 用户要求助手执行一条 bash 命令                | `tomcat code` + stdin `请执行 echo hello_from_pi`，timeout 60s                                    | exit 0；stdout 含 hello_from_pi 或命令确认提示                      |
| E2E-CLI-016C | 自动（需 `OPENAI_API_KEY`） | `test_user_background_bash_autofeed_real_llm_cli` | 用户让助手启动后台 bash、先做独立工作，然后只能依赖 `<background-task-finished ...>` 自动回灌继续推进 | `tomcat code` + 单轮 prompt：`bash(run_in_background=true)` 写 `BG_DONE`，立即写 `MARKER`，随后禁止 `task_output/task_list/task_stop` 与新 bash，只能等系统 follow-up | exit 0；stderr 含 `[bg] task ... queued for next turn`；`bg_done.txt` 与 `marker.txt` 真实落盘；stdout 含 `AUTOFEED_OK` |
| E2E-CLI-016D | 自动（需 `OPENAI_API_KEY`） | `test_user_background_bash_blocking_waitslice_real_llm_cli` | 用户要求助手在后台 bash 后立即进入 `task_output(block=true)`，直到拿到真实新输出再收尾 | `tomcat code` + 单轮 prompt：后台 bash `sleep 2; echo TOKEN_WAITSLICE; printf BLOCKWAIT_DONE > file; sleep 30`，必须用 `task_output(block=true, timeout_ms=300)` + `task_stop` 完成，不准 `task_output(block=false)` | exit 0；stdout 含 `BLOCKWAIT_OK`；非 TTY stderr 不含 `waiting_for_output`；transcript 至少 1 次 `task_output(block=true, timeout_ms=300)`，并出现 `TOKEN_WAITSLICE` 与 `task_stop`；产物文件内容正确 |
| E2E-CLI-016E | 自动（需 `OPENAI_API_KEY`） | `test_user_background_bash_multiple_timeout_slices_real_llm_cli` | 用户要求助手在同一后台任务上经历至少两次 timeout slice，再继续等到一次 `new_output` | `tomcat code` + 单轮 prompt：后台 bash `sleep 8; echo TOKEN_MULTI_TIMEOUT; printf MULTI_TIMEOUT_DONE > file; sleep 30`，必须重复 `task_output(block=true, timeout_ms=300)` 直到看到 token，再 `task_stop` | exit 0；stdout 含 `MULTI_TIMEOUT_OK`；非 TTY stderr 不含 `waiting_for_output`；transcript 中 `task_output` 至少 3 次、timeout 结果至少 2 次，最终出现 `TOKEN_MULTI_TIMEOUT` 与 `task_stop`；产物文件内容正确 |
| E2E-CLI-016F | 自动（需 `OPENAI_API_KEY`） | `test_user_background_bash_midturn_followup_real_llm_cli` | 用户先起后台 bash，再执行一个耗时 foreground batch；只有在 foreground batch 结束后的后续请求里看见 `<background-task-finished ...>`，助手才允许继续验证结果 | `tomcat code` + 单轮 prompt：先 `bash(run_in_background=true)` 写 `BG_DONE`，再 foreground `bash` 睡眠并写 `FG_DONE`，禁止 `task_output/task_list/task_stop`；若 foreground batch 结束时仍未看到系统消息则必须回复 `MIDTURN_MISSED_FOLLOWUP` 并停止 | exit 0；stdout 含 `MIDTURN_FOLLOWUP_OK` 且不含 `MIDTURN_MISSED_FOLLOWUP`；两份文件真实存在且内容正确；transcript 中 `<background-task-finished ...>` 位于 `FG_BATCH_START` 之后、成功词之前；不出现 `task_output/task_list/task_stop` |
| E2E-CLI-016G | 自动（需 `OPENAI_API_KEY`） | `test_user_background_bash_timeout_snapshot_stays_bounded_real_llm_cli` | 用户让助手观察一个几乎永不结束的后台 bash；在 EOF 处拿到 `timeout` 返回的 tail 快照后必须停止轮询，不能 busy loop | `tomcat code` + 单轮 prompt：后台 bash `printf HUNG_TIMEOUT_BOOT; sleep 60`；先用一次 `task_output(block=true)` 吃掉首个 `new_output`，再用第二次 `task_output(block=true)` 在 EOF 处命中 timeout 快照，随后禁止继续 poll / task_stop / task_list | exit 0；stdout 含 `HUNG_TIMEOUT_BOUNDED_OK`；非 TTY stderr 不含 `waiting_for_output`；transcript 中 `task_output` 次数有上界（<= 3）且至少 2 次；存在带 `HUNG_TIMEOUT_BOOT` 的 timeout 工具结果；`role=user` 不含 `waiting_for_output`；不出现 `task_stop/task_list` |
| E2E-CLI-018 | 自动 | `path_with_intent_silent_passthrough_contract` | 用户输入「路径 + 意图」时不触发本地路径授权命令 | `tomcat code` → 输入 `<abs-path> 看下里面有什么文件` | `parse_chat_command` 返回 `NotACommand`；自动化见 `tests/path_command_e2e.rs` |
| E2E-CLI-019 | 人工 | `manual_path_command_denied_shows_cancel_only` | 用户通过 `/path` 授权命中 deny 规则的路径时不能扩大授权 | 预置 `primitive.path_rules=[{path=<secret>, mode="deny"}]` → `tomcat code` → 输入 `/path <secret>` | 仅允许取消/不授权；不得展示永久允许、本次允许或工作区写授权选项；自动化回归见 `path_menu_with_deny_rule_hides_authorization_choices` |
| E2E-CLI-020 | 人工 | `manual_config_set_path_rules_runtime_effective` | 用户在同一会话内通过配置工具新增 deny 规则后立即生效 | `tomcat code` → 触发 `config_set primitive.path_rules` 追加 deny → 再请求 read/write 同一路径 | 后续工具调用被 deny 拦截，无需重启；自动化回归见 `runtime_deny_rule_overrides_existing_session_grant` / `config_set_array_path_rule_appends_with_json_value` |
| E2E-CLI-021 | 自动 | `read_binary_returns_structured_hint` | 用户要求读取二进制或非 UTF-8 文件时获得明确错误 | 构造未知扩展 + 含 NUL 字节文件 → `read` | 返回产品化错误，提示「binary / non-UTF-8 + 首字节 hex」，不把乱码注入上下文；自动化见 `tests/read_tool_tests.rs` |
| E2E-CLI-021a | 自动 | `read_text_offset_limit_window_with_line_numbers` | 用户用 `read` 工具按窗口查看文件并保留绝对行号 | 写 20 行文件 → `read{path,offset:15,limit:3,line_numbers:true}` | 返回 `    15\tL15\n    16\tL16\n    17\tL17\n` 形态；分页绝对行号；自动化见 `tests/read_tool_tests.rs` |
| E2E-CLI-021b | 自动 | `read_hashline_renders_two_char_hash_prefix` | 用户启用 `hashline` 获取行级 `xxh32` 短指纹用于 hashline 编辑 | 3 行小文件 → `read{path,line_numbers:true,hashline:true}` | 每行形如 `    1#XX:alpha`（6 + 1 + 2 + 1 + body 字节）；hashline 优先于 `line_numbers`；自动化见 `tests/read_tool_tests.rs` |
| E2E-CLI-021c | 自动 | `read_png_routes_to_image_and_can_build_input_image_part` | 用户读 PNG 时 `read` 自动走多模态路径并能注入 LLM | 拷贝 fixture PNG → `read` → `ChatMessageContentPart::image_b64(mime,&path)` | 返回 `ReadResult::Image{mime:image/png,filename,...}`；后续 helper 构造 `input_image` part 成功；自动化见 `tests/read_tool_tests.rs` |
| E2E-CLI-021d | 自动 | `read_pdf_routes_to_pdf_and_can_build_input_file_part` | 用户读 PDF 时 `read` 自动走多模态路径并能注入 LLM | 写 PDF fixture → `read` → `ChatMessageContentPart::file_b64(filename,mime,&path)` | 返回 `ReadResult::Pdf{mime:application/pdf,filename}`；后续 helper 构造 `input_file` part 成功；自动化见 `tests/read_tool_tests.rs` |
| E2E-CLI-021e | 自动 | `read_oversize_image_rejected_before_loading_bytes` | 用户读超大图片在 metadata 阶段被拒，避免 base64 膨胀 OOM | 构造 5 MiB PNG-magic blob → `read` | 返回 `AppError::Primitive`，错误信息含 `IMAGE_MAX_BYTES` / size 关键字；不读全文；自动化见 `tests/read_tool_tests.rs` |
| E2E-CLI-021f | 自动 | `read_large_window_is_cut_at_post_read_budget_with_resume_hint` | 用户读取超大文本窗口时，`read` 会在 128 KiB 后读预算处自动切页 | 写 40 行、每行 4096B 的文本 → `read{path,offset:1,limit:40}` | 返回内容在完整行边界停止（当前实现停在第 32 行），并附 `offset=33, limit=40` 续读提示；自动化见 `tests/read_tool_tests.rs` |
| E2E-CLI-021g | 自动 | `read_first_returned_line_over_budget_returns_recoverable_error` | 用户请求窗口的首个返回行本身过胖时，得到可恢复错误而不是把超大单行灌进上下文 | 写首行 > 128 KiB 的文本 → `read{path,offset:1,limit:2}` | 返回结构化错误，明确提示缩小 `offset/limit`；自动化见 `tests/read_tool_tests.rs` |
| E2E-CLI-022 | 自动 | `layer0_persist_file_readable` | 大工具结果落盘到 agent runtime trail，不污染 workspace definition | 构造超阈值 tool_result → Layer0 cleanup | 文件写入 `agent_trail_dir/tool-results/{session_id}`；不写入旧 `workspace/<session>/tool-results` 路径；preview 留在上下文 |
| E2E-CLI-023 | 自动 | `deny_path_command_menu_only_allows_cancel_contract` | 用户通过 `/path` 命中 deny 的路径后不会进入 LLM | 构造 deny `path_rules` → `/path <secret>` 路径授权菜单 | 菜单只允许取消；自动化见 `tests/path_command_e2e.rs` |
| E2E-CLI-026 | 自动 | `path_help_command_contract` | 用户可通过 `/help` 查看本地命令 | `tomcat code` → 输入 `/help` | 输出包含 `/path <绝对路径>` 与 `/help`；解析契约见 `parse_chat_command` 单测 |
| E2E-EXEC-024 | 自动 | `bash_assignment_rhs_denied_in_all_supported_positions` | Bash 中 `NAME=/path` 不绕过 deny | `stat -c %s p=/deny/foo`、`p=/deny/foo cmd`、`p=/deny/foo; cmd` | 每条都返回 Permission deny，错误包含 RHS 路径；自动化见 `tests/bash_assignment_deny.rs` |
| E2E-PROMPT-025-offline | 自动 | `system_prompt_names_three_directories_and_keeps_state_as_permission_list` | LLM prompt 明确当前目录是 `agent_workspace_dir`，且它不自动授权文件访问 | 构造三目录 `WorkspaceContext` + `WorkspaceState` | prompt 包含三目录用途/权限；`WorkspaceStateSection` 不重复 cwd / runtime trail 解释；自动化见 `tests/system_prompt_cwd_priority.rs` |
| E2E-CHAT-025-online | 人工 | `cwd_question_e2e`（待在线补验） | 真实 LLM 回答“当前目录”时看用户 cwd | 含 `OPENAI_API_KEY` 时在临时项目下运行 `tomcat code` 并询问当前目录 | 回复包含项目哨兵文件，不包含 `workspace-main` / `.pi_` |


**已实现**：E2E-CLI-013 已实现于 `test_user_asks_pi_to_write_hello_world_bash`（`tomcat/workspace-temp/e2e_cli013_hello/` 下写 hello_e2e.txt，见 [INTEGRATION_TEST_SPEC §2.3](INTEGRATION_TEST_SPEC.md#23-仓库内-scratch-目录)）；E2E-CLI-016 已实现于 `test_user_asks_pi_to_run_bash_command`。E2E-CLI-016C～016G 已实现于 `tests/cli_tests.rs`，分别覆盖 finish-only auto-feed、`task_output(block=true)` wait-slice、多次 timeout slice、midturn follow-up auto-feed，以及 timeout tail snapshot 后停止轮询的 bounded 路径。其离线/Mock/子模块契约同时由 `src/core/agent_loop/tests/background_monitor_test.rs`、`src/core/agent_loop/tests/submodules_test.rs`、`src/api/chat/tests/suite_test.rs`、`src/core/prompts/tests/load_test.rs` 与 `src/api/chat/cli_turn_renderer.rs` 相关单测回归锁定。E2E-CLI-018～026、E2E-EXEC-024、E2E-PROMPT-025-offline 的核心契约已由 `path_command`、`permission::gate`、`core::tools::config_tool`、`tools::primitive`、`compaction`、`system_prompt` 自动化回归覆盖；其中真实终端菜单观感与 E2E-CHAT-025-online 仍按「人工」补验。014、015 待后续补充。E2E-CLI-021/021a/021b/021c/021d/021e 已实现于 `tests/read_tool_tests.rs`（PR-RA/RB/RF/RJ/RM 的 6 个集成用例，覆盖文本分页、二进制结构化提示、hashline、PNG/PDF 多模态路由、超限拒绝）。

---

## Story 3 — WasmEdge + QuickJS 插件系统（6 条）

> **验收**：021–025 与 §4 人工验收「插件加载/卸载、错误隔离」对齐，建议在**本机真实插件路径**下补验；026 以自动化断言为主。


| 编号          | 验收 | 用例名                                                   | 用户意图              | 操作序列                                             | 必须断言                           |
| ----------- | -- | ----------------------------------------------------- | ----------------- | ------------------------------------------------ | ------------------------------ |
| E2E-CLI-021 | 人工 | `test_user_loads_plugin_and_lists`                    | 用户从路径加载插件并查看已加载列表 | `tomcat plugin load <plugin_dir>` → `tomcat plugin list` | load exit 0；list stdout 含插件 id |
| E2E-CLI-022 | 人工 | `test_user_views_plugin_info`                         | 用户查看插件详情（名称、版本）   | `tomcat plugin info <id>`                            | exit 0；stdout 含 name/version   |
| E2E-CLI-023 | 人工 | `test_user_disables_plugin`                           | 用户禁用插件            | `tomcat plugin disable <id>`                         | exit 0                         |
| E2E-CLI-024 | 人工 | `test_user_enables_plugin_after_disable`              | 用户重新启用被禁用的插件      | `tomcat plugin enable <id>`                          | exit 0                         |
| E2E-CLI-025 | 人工 | `test_user_unloads_plugin_removes_from_list`          | 用户卸载插件后从列表消失      | `tomcat plugin unload <id>` → `tomcat plugin list`       | unload exit 0；list 不含该 id      |
| E2E-CLI-026 | 自动 | `test_user_loads_nonexistent_plugin_path_shows_error` | 用户加载不存在路径时看到错误提示  | `tomcat plugin load /nonexistent/path`               | exit 0；stdout 含"不存在"或 error 信息 |


---

## Story 4 — rquickjs 插件运行时与兼容层（QuickJS E2E）（5 条）

> 主验收入口为 `tests/quickjs_e2e_tests.rs`；对应架构实施点见 [plugin-system-overview_new.md](../../../../architecture/plugin-system-overview_new.md) 中 P2/P3/P4/P10。事件语义补充见 [plugin-system/events.md](../../../../architecture/plugin-system/events.md)。
> 历史 Wasm 兼容验收仍保留在 `tests/wasmedge_e2e_tests.rs` / `tests/js_api_alignment_tests.rs`，且只在 `./scripts/run-integration-tests.sh integration-wasm` 中显式执行。

| 编号          | 验收 | 用例名                                  | 用户意图                                   | 操作序列                                                                 | 必须断言                                                                 |
| ----------- | -- | ------------------------------------ | -------------------------------------- | -------------------------------------------------------------------- | -------------------------------------------------------------------- |
| E2E-QJS-001 | 自动 | `run_script_console`                 | 插件脚本可在 rquickjs 中使用 `console` / microtask / timer | `WasmEngine.create_instance()` → `run_script()` 执行 `console.log/error`、`Promise.resolve()`、`setTimeout()` | host binding 收到 `log/error/microtask/timer` 四类日志；脚本无崩溃 |
| E2E-QJS-002 | 自动 | `pi_readfile_llm`                    | 插件通过 `pi.readFile()` 与 `pi.complete()` 走宿主 bridge | `start_session_vm` → `dispatch_session_event(session_start)`，脚本内 `await pi.readFile()` + `await pi.complete()` | VM 保持 `Running/Idle`；`readFile` 返回 mock 内容；LLM 返回 mock `"hi"` |
| E2E-QJS-003 | 自动 | `shims_and_crypto_work_in_session_vm` | Tier-A 垫片与同步 crypto 在 session VM 内可用 | `start_session_vm` → `dispatch_session_event(session_start)`，脚本验证 `path/util/events/Buffer/crypto` | `sha256/hmac/randomBytes` 正常；VM 健康；`end_session` 后 RuntimeManager 为空 |
| E2E-QJS-004 | 自动 | `runaway_plugin_interrupted`         | 插件跑飞后被 interrupt budget / timeout 掐断并可重建 | `start_session_vm` → `dispatch_session_event(loop)` 触发死循环 → 再次 `start_session_vm` | 首个 VM 进入 `Error`；再次启动返回新 handle 且可恢复 `Running/Idle` |
| E2E-QJS-005 | 自动 | `panicking_plugin_isolated`          | 一个插件抛错后不连坐同 session 下其他插件         | 同 session 启动 crashy + healthy 两个插件 → 仅向 crashy 分发异常事件        | crashy 进入 `Error`；healthy 保持 `Running`；`end_session` 后 RuntimeManager 清空 |


---

## Story 5 — 宿主工具注册（2 条）


| 编号           | 验收 | 用例名                                                 | 用户意图                                       | 操作序列                                               | 必须断言                                         |
| ------------ | -- | --------------------------------------------------- | ------------------------------------------ | -------------------------------------------------- | -------------------------------------------- |
| E2E-WASM-011 | 自动 | `test_wasmedge_e2e_tool_registration`               | 插件 JS 通过 registerTool 注册工具后宿主可感知 host_call | `run_script_file(tool_register_test.js)`           | host_call 中 method=registerTool 至少触发 1 次；无崩溃 |
| E2E-CLI-031  | 人工 | `test_user_tool_registered_by_plugin_can_be_called` | 插件注册的工具可被对话模式调用（需 OPENAI_API_KEY）        | load_plugin + `tomcat code` + 触发工具的 prompt，timeout 60s | stdout 含工具执行结果或调用确认                          |


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
| E2E-CLI-041 | 人工 | `test_user_chats_with_llm_gets_streaming_response` | 用户与 LLM 对话，获得流式渲染回复  | `tomcat code` + stdin 一句话，timeout 60s    | exit 0；stdout 含 AI 回复；有对话 banner |
| E2E-CLI-042 | 人工 | `test_user_receives_nonempty_llm_response`         | 确认 LLM 回复内容非空（基础连通性） | `tomcat code` + stdin `说一个字`，timeout 30s | exit 0；stdout 非空                 |
| E2E-CLI-043 | 自动 | `test_user_toggles_thinking_display_modes` | 用户运行时通过 `/thinking` 切换 CLI thinking 显示档位（minimal/summary/full/toggle） | 构造 `CliTurnRenderer` + mock `thinking_delta` 流 → 顺序应用 `/thinking summary` / `minimal` / `full` / `toggle` → 各档位发出同样的 summary+raw delta | summary 模式：可见 summary、隐藏 raw、`[thinking]` 仅 1 次；minimal 模式：仅 `[thinking] ...` 占位；full 模式：summary+raw 同时可见；toggle 循环 `summary→full→minimal→summary`；自动化见 `src/api/chat/tests/cli_turn_renderer_test.rs` + `src/api/chat/commands/tests/cmd_thinking_test.rs` |
| E2E-CLI-044 | 自动 | `llm_error_renders_red_status_line` | Responses `response.failed` / 顶层 `error` 被映射为结构化 LLM 错误并在 CLI 明确显示 | 构造 `CliTurnRenderer` + `llm_error` payload，模拟正文后收到错误终局 | stderr 含红色 `[llm] <error message>`；不复用 `ExtensionError`；自动化见 `src/api/chat/tests/cli_turn_renderer_test.rs` + `src/core/agent_loop/tests/stream_handler_test.rs` |
| E2E-CLI-045 | 自动 | `llm_notice_renders_dim_non_error_hint` | Responses `incomplete/max_output_tokens` 走非错误轻提示而非红字报错 | 构造 `CliTurnRenderer` + `llm_notice{finishReason=max_output_tokens}` payload | stderr 含灰色轻提示、包含 `max_output_tokens`，且**不**出现红色错误样式；自动化见 `src/api/chat/tests/cli_turn_renderer_test.rs` + `src/core/agent_loop/tests/stream_handler_test.rs` |


---

## Story 8 — CLI 对话与会话管理（18 条）

> **验收**：会话与审计子命令以自动化为主；058 涉及 chat 失败路径，可与 §4「对话模式」人工清单一并 spot-check；062/063（Ctrl+C 软/硬中断）数据契约由 `src/core/agent_loop/tests/interrupt.rs`（中断路径，T2-P0-001 后由原单文件 `tests.rs` 拆分目录化）+ `src/api/chat/tests.rs::interrupt_persists_transcript_hard_ack`（T-017 partial 落盘）+ `src/api/cli/chat_cmd::check_double_tap` 单测锁定，终端观感由 §4 人工清单补验。


| 编号          | 验收 | 用例名                                                     | 用户意图                      | 操作序列                                                       | 必须断言                            |
| ----------- | -- | ------------------------------------------------------- | ------------------------- | ---------------------------------------------------------- | ------------------------------- |
| E2E-CLI-051 | 自动 | `test_user_creates_new_session`                         | 用户创建一个新会话                 | `tomcat session new`                                           | exit 0；stdout 含"已创建会话"          |
| E2E-CLI-052 | 自动 | `test_user_lists_sessions`                              | 用户查看所有会话                  | `tomcat session list`                                          | exit 0                          |
| E2E-CLI-053 | 自动 | `test_user_switches_to_existing_session`                | 用户切换到已存在的会话               | `tomcat session new`（记下 `session_id_a`）→ `tomcat session new` → `tomcat session switch <session_id_a>`  | exit 0                          |
| E2E-CLI-054 | 自动 | `test_user_switches_to_nonexistent_session_shows_error` | 用户切换到不存在会话时看到友好提示         | `tomcat session switch nonexistent-session-id`                        | exit 0；stdout 含"不存在"            |
| E2E-CLI-055 | 自动 | `test_user_deletes_session`                             | 用户删除刚创建的会话                | `tomcat session new` → `tomcat session delete agent:main:main`  | exit 0；stdout 含"已删除"            |
| E2E-CLI-056 | 自动 | `test_user_archives_session`                            | 用户归档会话                    | `tomcat session new` → `tomcat session archive agent:main:main` | exit 0；stdout 含"已归档"            |
| E2E-CLI-057 | 自动 | `test_user_searches_sessions_by_keyword`                | 用户按关键词搜索会话                | `tomcat session search default`                                | exit 0                          |
| E2E-CLI-058 | 人工 | `test_user_chat_without_api_key_fails_gracefully`       | 无 API key 时对话入口快速失败，不挂起 | `tomcat code`（移除 OPENAI_API_KEY），timeout 5s                    | 进程 5s 内结束；stdout 或 stderr 含错误提示 |
| E2E-CLI-059 | 自动 | `test_user_views_audit_list`                            | 用户查看操作审计记录列表              | `tomcat audit list`                                            | exit 0                          |
| E2E-CLI-060 | 自动 | `test_user_exports_audit_to_file`                       | 用户导出审计记录到文件               | `tomcat audit export /tmp/audit.json`                          | exit 0；文件存在                     |
| E2E-CLI-061 | 自动 | `test_user_views_audit_show_invalid_id`                 | 用户查看不存在的审计条目时友好提示         | `tomcat audit show 9999999`                                    | exit 0；不 panic                  |
| E2E-CLI-062 | 人工 | `test_user_interrupt_during_bash`                       | 用户在对话中触发 `execute_bash` 长命令后 Ctrl+C 软中断；partial assistant + 已完成 tool_result 落 transcript，`^C 已中断（partial 已保存）` 提示出现，prompt 返回，可继续输入 | `tomcat code` → stdin 触发 `execute_bash "sleep 30"` → 观察 tool_execution_start → `SIGINT` → 观察 prompt 回归 → `exit`（Ctrl+D） | 进程继续存活；transcript JSONL 末尾有 partial assistant（role=assistant、含 tool_calls） 及 / 或 tool_result（role=tool、tool_call_id 匹配）；无 panic；自动化层由 `run_interrupt_between_tools_retains_completed_tool_result` / `interrupt_persists_transcript_hard_ack` 锁死数据契约 |
| E2E-CLI-063 | 人工 | `test_user_double_ctrlc_exits`                          | 用户 2 秒内双击 Ctrl+C，对话进程以 exit code 130 终止；首击已 cancel 的 partial 仍完整落盘 | `tomcat code` → 发送任意触发 LLM 流式回复的 prompt → 第一次 `SIGINT` 后 1 秒内再发一次 `SIGINT` | 子进程 exit code == 130（128 + SIGINT）；transcript 含首击 cancel 的 partial assistant；双击阈值（2s）行为由单测 `check_double_tap` 四用例锁定（`api::cli::chat_cmd::tests`） |
| E2E-CLI-064 | 自动 | `test_chat_path_executes_web_search_tool_with_mock_server` | 用户在 chat 中触发 `web_search`，收到结构化联网搜索结果 | `run_chat_turn` + deterministic mock LLM 发 `web_search {"query":"reqwest rust","domain_filter":["docs.rs"]}`；Tavily mock server 返回 docs.rs 命中 | `final_text` 含收尾文本；tool result JSON 含 `backend=tavily` 与 `https://docs.rs/reqwest` |
| E2E-CLI-065 | 自动 | `live_example_fetch_smoke` | 用户抓取公开网页并获得正文或 `tool-results` 落盘路径 | 设 `PI_LIVE_WEB_FETCH=1` 后执行 `cargo test --test web_fetch_tool_tests live_example_fetch_smoke -- --nocapture` | `code == 200` 且 `result` 非空或 `persisted_output_path` 存在；离线 contract 另由 `submodules_test::tool_exec_web_fetch_routes_to_runtime` + `src/core/tools/web_fetch/tests/fetcher_test.rs` 锁定 |
| E2E-CLI-066 | 自动 | `test_chat_skill_discovery_disclosure_and_load_skill_roundtrip` | 用户在项目目录放入 skill 后，模型能在 prompt 看到目录卡片并按名装载正文 | 临时 workspace 写 `.tomcat/skills/commit/SKILL.md` → 构造 `ChatContext` + deterministic mock LLM 发 `load_skill {"name":"commit"}` → `run_chat_turn` | system prompt 含 `<available_skills>` 与 `<skill name="commit">Create a git commit</skill>`；`load_skill` tool result 含 `<skill name="commit"` 与正文；正文不含 frontmatter；`disable-model-invocation` skill 不出现在 prompt |
| E2E-CLI-067 | 自动 | `test_user_chat_skill_list_reload_use` | 用户在对话内列出 / 重载 / 显式使用 skill，且 user-only skill 可被点名 | `tomcat code` 或 inprocess chat loop：先 `/skill list`，再执行 `/skill reload`，最后 `/skill use secret "summarize request"` | `/skill list` 输出含 source / user-only 标签；`/skill reload` 成功回显；`/skill use` 向当前轮注入 `<skill>` 正文与 intent，且不因 `disable-model-invocation` 被拒 |
| E2E-CLI-068 | 自动 | `test_user_skill_cli_list_reload_e2e` | 用户通过外层 CLI 查看 / 重扫 skill 目录与诊断 | `tomcat skill list` → 新增或破坏 `.tomcat/skills/**/SKILL.md` → `tomcat skill reload` | 两个子命令都 exit 0；stdout 含 discovered skills / warnings / diagnostics；reload 后新增 skill 可见，坏文件仅产出 diagnostics、不阻断其余 skill |
| E2E-CLI-069 | 自动（env-gated） | `live_skill_load_roundtrip_with_real_llm` | 用户在真实模型链路里看到技能目录卡片，模型按名调用 `load_skill` 后再回答 | 设 `PI_LIVE_SKILL=1` 且配置有效 `OPENAI_API_KEY` 后执行 `cargo test --test skill_tool_tests live_skill_load_roundtrip_with_real_llm -- --nocapture` | 最终回答包含技能正文里的仅正文 token 与 `SKILL_LIVE_OK`；tool transcript 含 `load_skill` 结果；默认门禁不执行本场景 |

> 说明：`web_fetch` 当前只接受公网 `http/https` URL，默认拒绝 localhost / IP literal，并把更完整的 domain approval 留到 PR-WF-D；因此仓内默认的用户路径 smoke 仍保留为 env-gated live example，而不是伪造一个会绕过安全边界的本地 mock chat 用例。


---

## Story 8b — 长生命周期 VM 与有状态插件（TASK-15 + TASK-05b/c Tier1–2，8 条）

> Wasm 真实运行时 E2E 用例（`tests/wasmedge_e2e_tests.rs`）。须安装 WasmEdge 且以 `--features wasmedge` 编译；默认 `./scripts/run-integration-tests.sh all` 跳过本组（commit `f613708` 收窄），显式验收走 `./scripts/run-integration-tests.sh integration-wasm`。
> **验收**：031–035 以 `wasmedge_e2e_tests` 自动化为准；036–038 与插件兼容矩阵相关，建议**人工补验**本机 WasmEdge 与真实扩展抽样。


| 编号           | 验收 | 用例名                                                       | 用户意图                 | 操作序列                                                                                                                                       | 必须断言                                                                        |
| ------------ | -- | --------------------------------------------------------- | -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------- |
| E2E-WASM-031 | 自动 | `test_wasmedge_e2e_vm_actor_state_persists_across_events` | 插件全局变量跨事件保持          | start_session_vm → dispatch_session_event x2 → 检查 host_call 中的累加值                                                                          | 第二次事件的 host_call 反映累加状态；无崩溃                                                 |
| E2E-WASM-032 | 自动 | `test_wasmedge_e2e_handler_stays_registered`              | 已注册 handler 多次事件持续有效 | start_session_vm → dispatch_session_event("evt") x2                                                                                        | 每次 dispatch 均触发 handler（host_call 计数递增）                                     |
| E2E-WASM-033 | 自动 | `test_wasmedge_e2e_set_interval_runs_during_session`      | 会话期间周期性日志（定时器语义）稳定触发 | start_session_vm；fixture 用 `setTimeout` 链模拟周期（wasmedge_quickjs 对全局 `setInterval` 不稳定）；sleep ≥1.2s；断言 `VmActorState::Running`；`end_session` | 会话中 VM 仍为 Running；`pi.log` 侧可见多次 tick；`end_session` 后 RuntimeManager 为空、无悬挂 |
| E2E-WASM-034 | 自动 | `test_wasmedge_e2e_multi_session_isolation`               | 多会话上下文隔离             | start_session_vm(s1) + start_session_vm(s2) → 分别 dispatch → 验证各自 host_call                                                                 | s1 与 s2 的 host_call 各自独立、状态不串会话                                             |
| E2E-WASM-035 | 自动 | `test_wasmedge_e2e_session_end_no_hanging_threads`        | 关闭流程无悬挂线程            | start_session_vm → end_session → 检查 VmActorHandle 状态                                                                                       | end_session 后 RuntimeManager 为空；handle state 为 Stopped/Error                |
| E2E-WASM-036 | 人工 | `test_wasmedge_e2e_tps_tier1_agent_end_notify`            | tps Tier1：TS 长生命周期 + notify              | 临时插件 `main.ts`（fixture tps 源码）→ `start_session_vm` → `dispatch_session_event(agent_start)` → sleep → `dispatch_session_event(agent_end)`（含 assistant usage） | `with_ui_notify_counter` ≥1；`end_session` 后 RuntimeManager 为空                         |
| E2E-WASM-037 | 人工 | `test_wasmedge_e2e_tier2_compat_script`                   | TASK-05c Tier2：`registerCommand`+`__pi_invoke_command`、`registerTool`（schema 包装）、`ctx.ui` 等价 host、`executeBash`+args | `run_script_file(tier2_compat_test.js)` + stub host | 相关 host_call 次数 ≥7；脚本打印 done、无抛错 |
| E2E-WASM-038 | 人工 | `test_wasmedge_e2e_tier2_transpiled_export_default_plugin` | TASK-05c：社区风格 `export default function(pi)` TS 经 SWC 加载 + 命令注册与同步 invoke | 临时 `tier2_snippet.ts` → `run_script_file` + stub host | `registerCommand` host_call ≥1；脚本无抛错 |
| E2E-WASM-039 | 自动 | `test_wasmedge_e2e_tier3_diff_custom_ui` | TASK-05d Tier3：diff.ts 核心路径——registerCommand("diff") → exec("git") → ctx.ui.custom 渲染 Container/SelectList/Text | `run_script_file(tier3_diff_test.js)` + stub host（git status 返回固定 porcelain） | `executeBash` ≥1；`uiCustom` ≥1；脚本打印 done、无抛错 |
| E2E-WASM-040 | 自动 | `test_wasmedge_e2e_tier4_files_session_branch` | TASK-05d Tier4：files.ts 核心路径——registerCommand("files") → ctx.sessionManager.getBranch() → 空 session 降级 uiNotify | `run_script_file(tier4_files_test.js)` + stub host（getBranch 返回空数组） | `getBranch` ≥1；`uiNotify` ≥1；脚本打印 done、无抛错 |
| E2E-WASM-041 | 自动 | `test_wasmedge_e2e_tier3_diff_real_ts` | diff.ts 真实 TS 源码——长生命周期 VM + command_invoke → async handler 调用 pi.exec("git") → commandCompleted | `start_session_vm` + SWC 转译 diff.ts + `dispatch_session_event(command_invoke)` + mock PrimitiveExecutor | `commandCompleted` ≥1；handler 异步完成、无挂起 |
| E2E-WASM-042 | 自动 | `test_wasmedge_e2e_tier4_files_real_ts` | files.ts 真实 TS 源码——长生命周期 VM + command_invoke → async handler 调用 ctx.sessionManager.getBranch() → commandCompleted | `start_session_vm` + SWC 转译 files.ts + `dispatch_session_event(command_invoke)` + mock SessionManager | `commandCompleted` ≥1；handler 异步完成、无挂起 |


---

## Story 9 — AgentLoop 核心结构（TASK-14，3 条 + TASK-17 上下文管理 3 条）

> 需要 `OPENAI_API_KEY`；无 key 时必须 `panic!`（符合 INTEGRATION_TEST_SPEC §5.2）。
> **验收**：与 §4 人工验收「对话模式、多轮上下文、会话切换」对齐，建议**人工补验** AgentLoop 与终端交互体验。


| 编号          | 验收 | 用例名                                               | 用户意图                                         | 操作序列                                                                                      | 必须断言                                         |
| ----------- | -- | ------------------------------------------------- | -------------------------------------------- | ----------------------------------------------------------------------------------------- | -------------------------------------------- |
| E2E-CLI-081 | 人工 | `test_user_chat_non_interactive_with_prompt_flag` | 用户启动 `tomcat code` 并输入单句提问，AgentLoop 执行并输出 AI 回复 | `tomcat init` → `tomcat code`（stdin: `"Reply with exactly: pong\n"`，timeout 60s，含 OPENAI_API_KEY） | exit 0；stdout 非空（AI 已通过 AgentLoop::run() 回复） |
| E2E-CLI-082 | 人工 | `test_user_chat_resumes_last_session`             | 用户用 `--resume` 恢复上次会话，历史消息从 JSONL 加载         | `tomcat init` → `tomcat code`（stdin 第一轮）→ `tomcat code --resume`（stdin 第二轮，timeout 60s）               | exit 0；第二轮 stdout 非空；进程正常退出                  |
| E2E-CLI-089 | 人工 | `test_user_chat_model_switch_persists_across_resume` | 用户在对话内切换模型，并在重启/恢复后继续沿用该会话模型 | `tomcat init` → `tomcat code` 输入 `/model list`、`/model use deepseek-reasoner`、`/model current` → 退出 → `tomcat code --resume` → 再次输入 `/model current` | `/model current` 两次都显示 `deepseek-reasoner`；resume 后 prompt / banner 仍携带当前模型；`sessions.json` 中 `model_override` 已持久化 |
| E2E-CLI-083 | 人工 | `test_user_chat_multi_turn_context_retained`      | 用户进行两轮提问，第二轮 Agent 可引用第一轮答案（多轮上下文）           | `tomcat code`（stdin: 两行问答，第二问引用第一问答案，timeout 90s）                                             | exit 0；stdout 包含第二问回复且非空                     |
| E2E-CLI-084 | 自动 | `test_layer0_persist_and_readback`；`layer0_threshold_from_config`（`src/core/compaction/tests.rs`）；`test_compact_tool_results_*`（`tests/context_management_tests.rs`） | 超大 tool result：Layer 0 落盘 + preview / compactable 区内占位，不导致单次上下文爆炸 | 构造超大 `ToolResult` → `layer0_persist_large_results` 或 `compact_tool_results` / 集成写盘读回 | 落盘路径可读回或占位符替换；估算字符下降；阈值随 `[context]` 生效 |
| E2E-CLI-085 | 自动 | `test_context_overflow_triggers_compaction_and_retries` | Context overflow 触发 Compaction 后自动恢复 | Mock LLM 先返回 overflow 错误 → 触发压缩 → 重试成功 | 预算恢复；重试成功返回结果 |
| E2E-CLI-086 | 自动 | `test_session_reload_with_branch_summary_entries`；`test_session_reload_with_boundary` | Session JSONL 含 `type: branch_summary` 摘要行时重载正确 | 写入含 BranchSummaryEntry 的 JSONL → `init_context_state` → `build_context` | `CompactionSummary` 消息位置与顺序正确；boundary 场景无重复 |
| E2E-CLI-087 | 自动 | `preheat_warmup_active_vs_result_pending`、`preheat_restore_pending_result_keeps_non_idle_until_consumed`（`src/core/compaction/tests.rs`）；`test_session_reload_pending_preheat_restore`（`tests/context_management_tests.rs`） | 异步预热状态机：结果 pending / restore 与重载一致 | 构造 `Preheat` / 写 `is_boundary=false` 的 `branch_summary` 行后 `init_context_state` | preheat 非 idle 语义正确；重载后可 `poll_result` / `CachedCompleted` |
| E2E-CLI-088 | 自动 | `check_before_request`（`src/core/compaction/apply.rs`，由 `api/chat` 时机 ② 调用） | ratio ≥ 0.98 且预热未完成时允许同步等待后再发 LLM | 逻辑见 apply.rs；全链路行为由 `cli_tests` + chat 诊断日志观测 | 无独立历史用例名 `test_sync_wait_at_098`；以 apply 实现 + 集成为准 |
| E2E-CLI-089 | 自动 | `apply_boundary_replaces_covered_range` 等（`src/core/compaction/tests.rs`）；`check_after_reply_skips_below_085` / `check_after_reply_skips_when_no_preheat` | 预热完成后 Boundary / ratio 分档不误切换 | `CompactionResult` + `apply_boundary`；高 ratio 无 preheat 时 `check_after_reply` 不切换 | messages 含 `CompactionSummary`（`MessageKind`）；ratio 下降；idle preheat 不触发切换 |
| E2E-CLI-090 | 自动 | `test_session_reload_boundary_false_skipped` | Session 重载识别 is_boundary=false/true | 写含 is_boundary=false 的 BranchSummaryEntry → init_context_state | is_boundary=false 被跳过；is_boundary=true 生效 |
| E2E-CLI-091 | 自动 | `test_context_metrics_update_event_published` + `persist_context_observability_writes_sessions_json` | 上下文指标事件节奏与 `sessions.json` 可观测累计刷盘 | AgentLoop mock：`context_metrics_update` 顺序与字段；SessionManager：`persist_context_observability` 写入 `compactionCount` 等价字段 | stderr/事件含合法 metrics；store 中 `compaction_tokens_freed` 等与 `ContextState` 一致 |
| E2E-CLI-092 | 自动 | `check_after_reply_stale_apply_removes_branch_summary_and_keeps_preheat_idle`（`src/core/compaction/tests.rs`） | §5.7.5.1 列表与磁盘不一致：陈旧 `CompactionResult` 路径删 `branch_summary` 行、preheat 回 idle | `check_after_reply` + 不可匹配 `covered_end_id` 的 stale 场景（见架构 §5.7.5.1） | JSONL 对应行被移除；preheat 不再持有陈旧 completed |
| E2E-CLI-093 | 自动 | `read_entries_tail_skips_unknown_type_line`（`src/core/session/transcript/tests.rs`） | JSONL tail 中含无法反序列化到 `TranscriptEntry` 的行时不崩溃 | header + 合法 `message` + 合法 JSON 但 `type` 非 enum 成员 | `read_entries_tail` 返回仅含可解析条目；不 panic |
| E2E-CLI-094 | 自动 | `mid_turn_guard_rewrites_tail_and_transcript`（`src/core/agent_loop/tests/current_tail_guard_test.rs`） + `test_reasoning_loop_mid_turn_precheck_rewrites_before_second_llm`（`tests/context_management_tests.rs`） | current-tail aggregate guard 在下一次 LLM 请求前先减负，而不是等 overflow/空响应后补救 | 单测直打 `maybe_reduce_before_next_llm`；集成测试走 `AgentLoop::run()`，构造 many-medium `read` current tail 并观察第二次 LLM 请求 | 旧半区先 placeholder；超大结果走 persisted preview；transcript best-effort 回写；`post_usage_appended_chars` 同步下降；`reasoning_loop` 确认在第二次 LLM 前已完成减负 |
| E2E-CLI-095 | 自动 | `collapse_to_branch_summary_keeps_planning_snapshot`（`src/core/agent_loop/tests/current_tail_guard_test.rs`） | reduction 不够时把整份 working messages 折成一条 `branch_summary`，但 planning/runtime 状态不能丢 | 构造不可减负且超预算的 working set + Planning 模式 session todos → 运行 `maybe_reduce_before_next_llm` | 进入单条 `CompactionSummary`；摘要含 `Execution Keepalive`、当前步骤与 pending work；transcript 追加 `branch_summary(is_boundary=true)` |

> **TASK-17 备注**：E2E-CLI-084/085/086 上下文管理对用户透明（无新 CLI 命令），验收以 `tests/context_management_tests.rs` 为主、`src/core/compaction/tests.rs` 为 Layer0/L2 单测补充（见上表「用例名」列）。
> **TASK-20 备注**：E2E-CLI-087~090 异步预热与 Boundary/L3 语义：集成见 `context_management_tests.rs`，状态机与 `apply_boundary` 见 `src/core/compaction/tests.rs`；时机 ② `check_before_request` 见 `apply.rs` 与 `api/chat`。
> **TASK-21 备注**：§5.7 消息级 ID、锚点插入、`S::E`：`src/core/session/transcript/tests.rs` 与 `context_management_tests.rs` 中重载/边界用例对齐 JSONL 行序与 fold。**§5.7.5.1 陈旧 apply** 见 **E2E-CLI-092**；**read_entries_tail 跳过未知 type** 见 **E2E-CLI-093**。开发阶段不读盘兼容 `type: compaction`，见 [session-storage.md](../../../../docs/architecture/session-storage.md) transcript 说明。
> **上下文可观测性完善**：E2E-CLI-091 中 `test_context_metrics_update_event_published` 位于 `tests/agent_loop_tests.rs`，`persist_context_observability_writes_sessions_json` 位于 `src/core/session/manager/tests.rs`（lib 单测）。
> **阶段二 current-tail guard**：E2E-CLI-094/095 对应“发请求前减负”与“整份 collapse + keepalive”两条主路径；其中 094 现在同时覆盖 helper 级单测与 `AgentLoop::run()` 集成链路。本轮复用既有 integration crate，因此 `scripts/test-groups.sh` 仍无需额外分组变更。

---

## Story 10 — Plan 模式真 LLM 全路径（reviewer 子 Agent，3 条）

> 需要 `OPENAI_API_KEY`；无 key 时用例 fixture **必须** `panic!`（与 INTEGRATION_TEST_SPEC §5.2、reviewer.md §11 对齐）。默认模型来自 `TOMCAT_E2E_LLM_MODEL`，未设 → `gpt-5.4`。
> 这组用例与 [`plan_e2e_with_mock_llm_tests.rs`](../../../../tests/plan_e2e_with_mock_llm_tests.rs) **互补**：mock 组覆盖事件序与命令入口；这里保留两条窄 CLI smoke（planning-only / exec-only）与一条 inprocess full-chain 真 LLM 验收。
> 会话模型固定为 `session_key = agent:main:main`；每次真测 run 先创建 fresh `session_id`，CLI 的进程 A/B 共享同一个 `run_session_id`，inprocess 用另一条新的 `run_session_id`。`create_plan` 在 planning 阶段保持 `session_key/session_id == null`，只有 `/plan build` 进入 EXEC 时才绑定真实 session。`<plan>.plan.md.lock` 是 advisory 侧车文件，unlock 后可继续存在，不需要在真测前后批量清理。

| 编号 | 验收 | 用例名 | 用户意图 | 操作序列 | 必须断言 |
| ---- | ---- | ------ | -------- | -------- | -------- |
| E2E-PLAN-RL-001 | 自动（需 `OPENAI_API_KEY`） | `cli_planning_path_with_real_llm`；`cli_exec_resume_path_with_real_llm`（[`tests/plan_real_llm_cli_e2e.rs`](../../../../tests/plan_real_llm_cli_e2e.rs)） | 用两条窄 CLI smoke 分别覆盖真 `create_plan` 与真 `/plan build` | planning-only：fresh `run_session_id` → `tomcat code` + `/plan` + PLANNING_PROMPT → EOF；exec-only：fresh `run_session_id` + 预置 planning plan → `tomcat code --resume` + `/plan build {id}` + EXEC_PROMPT → EOF | planning-only：子进程 exit 0、磁盘 `frontmatter.state == Planning`、todos 非空、planning prompt 可见；exec-only：子进程 exit 0、seeded plan 已离开 `Planning`、`frontmatter.session_id` 绑定到该次 `run_session_id`、EXEC prompt 可见。full completion / artifact / transcript 顺序改由 `E2E-PLAN-RL-002` 负责 |
| E2E-PLAN-UI-003 | 自动 | `user_prompt_for_mode_formats_all_states`；`agent_prompt_for_mode_uses_agent_prefix_and_hides_plan_id`；`cli_planning_path_with_real_llm`；`cli_exec_resume_path_with_real_llm` | 用户在 Chat/Planning/Executing/Pending/Completed 间切换时，CLI prompt 能准确表达当前模式 | `/plan` → `/plan build <id/path>` → EXEC 推进 → completed / pending | user prompt 显示 `u[Chat]>` / `u[Plan:planning]>` / `u[Plan:executing]>` / `u[Plan:pending]>` / `u[Plan:completed]>`；agent prompt 显示 `agent.<id>[Plan:planning]>` / `agent.<id>[Plan:executing]>` / `agent.<id>[Plan:pending]>` / `agent.<id>[Plan:completed]>`；不再显示 `tomcat.<id>` 或 `[EXEC plan_id=...]` |
| E2E-PLAN-UI-004 | 自动 | `plan_build_enters_exec_and_binds_active_plan_path`；`full_plan_lifecycle_create_build_complete`；`cli_exec_resume_path_with_real_llm` | 用户执行 `/plan build <id/path>` 后无需再补一句，立即进入首个 EXEC 回合 | `/plan build <id/path>` → 观察 CLI 输出与后续工具调用 | `/plan build` 成功后自动出现 `u[Plan:executing]> start building <path>`；紧随其后进入标准 `run_chat_turn` / thinking / tool 调用链路；active plan path 与执行中的 plan 绑定一致 |
| E2E-PLAN-UI-005 | 自动 | `ask_question_result_carries_skipped_flag_for_skipped_question`；`ask_question_skipped_answer_rejects_option_ids`；`ask_question_ui_appends_custom_slot_via_panel_round_trip` | 用户在 PLAN 模式回答结构化问题时，`skip` / 自定义答案 / 单选语义稳定 | `ask_question` 面板 → 输入 `skip`、`c`、`c web UI`、非法输入重试 | `skip` 只跳当前题且结果返回 `skipped: true`；自定义答案返回 `option_ids:["__custom__"] + custom_text`；非法输入只重试当前题，不出现“答案数不一致”底层错误 |
| E2E-PLAN-RL-002 | 自动（需 `OPENAI_API_KEY`） | `inprocess_full_plan_path_with_real_llm`（[`tests/plan_real_llm_inprocess_tests.rs`](../../../../tests/plan_real_llm_inprocess_tests.rs)） | 进程内驱动 `ChatContext` + `run_chat_turn` 真起一次主 LLM + reviewer 子 LLM 跑全路径 | 先为 `agent:main:main` 建 fresh `run_session_id` → `dispatch_chat_command(/plan)` → `run_chat_turn(PLANNING_PROMPT)` → 从 `AgentRunOutcome.new_messages` 提取本次 `create_plan` 返回的 `plan_id/path` → `build_plan`（传真实 `run_session_id`）→ `run_chat_turn(EXEC_PROMPT)`（最多 3 轮）→ `finalize_completed_to_chat` | 磁盘 `frontmatter.state == Completed`；所有 todos `Completed`；`PlanRuntime::mode() == Chat` after finalize；transcript 至少一条 `plan.review` 自定义事件；EXEC/completed 盘 frontmatter 的 `session_id` 对应该次 `run_session_id` |

> **reviewer 子 Agent**：两条用例都默认走 [`prod_reviewer.rs`](../../../../src/api/chat/plan_runtime/prod_reviewer.rs) 真派发，主 LLM 每次 `create_plan` 都会消耗一段子 LLM token。transcript 中的 `plan.review` 事件包含 `reviewer_turns_used` / `reviewer_turns_limit` / `reviewer_stop_reason`，便于事后分析。

---

## 边界与健壮性场景（跨 Story）（7 条）


| 编号          | 验收 | 用例名                                    | 用户意图                       | 操作序列                    | 必须断言                                                    |
| ----------- | -- | -------------------------------------- | -------------------------- | ----------------------- | ------------------------------------------------------- |
| E2E-CLI-070 | 自动 | `test_resume_after_interrupt`          | 用户在一次中断后重启对话入口，已持久化 partial transcript 可继续 hydrate | `tomcat code` 中断一轮 → 重新执行 `tomcat code --resume` | 第二次启动不丢已落盘 partial；hydrate 后可继续输入；`/ckpt list` 可见 Interrupt ckpt |
| E2E-CLI-071 | 自动 | `test_user_views_full_help`            | 用户查看帮助，所有子命令可见             | `tomcat --help`             | exit 0；stdout 含 init/doctor/config/session/workspace/plugin/audit |
| E2E-CLI-072 | 自动 | `test_user_views_version`              | 用户查看版本号                    | `tomcat --version`          | exit 0；stdout 含版本号字符串                                   |
| E2E-CLI-073 | 自动 | `test_user_runs_unknown_command`       | 用户输入错误命令时看到帮助              | `tomcat nonexistent_cmd`    | exit 非 0；stderr 含"error"                                |
| E2E-CLI-074 | 自动 | `test_user_init_then_doctor_roundtrip` | 用户 init 后 doctor 通过，完整引导流程 | `tomcat init` → `tomcat doctor` | 两步 exit 0；doctor 含"配置合法"+"内嵌资源已就绪" |
| E2E-CLI-075 | 自动 | `test_slash_restore_recovers_after_bad_edit` | 用户通过 `/restore` 从坏编辑中恢复工作区并作废后续对话 | 先产生 TurnEnd ckpt → 人为改坏文件并追加新对话 → `/restore <ck>` | 文件内容回到 checkpoint；锚点之后 transcript 标记 `superseded`；追加 `Custom{checkpoint.restore}` |
| E2E-CLI-076 | 自动 | `test_hangup_during_run_leaves_interrupt_ckpt` | 用户在模型输出进行中挂断终端，重启后仍能看到 partial transcript 与中断锚点 | 启动 `tomcat code` 并让 mock LLM 流式回包 → 输出进行中发送 `SIGHUP`/关闭 stdin → 重启或检查 `/ckpt list` | 当次会话以 `Interrupted` 收尾；transcript 已落盘 partial assistant；`/ckpt list` 含 `Interrupt` checkpoint；空闲 EOF 对照场景不写此类 ckpt |
| E2E-CLI-077 | 自动 | `init_context_state_heals_single_dangling_tool_call_and_appends_marker`；`init_context_state_heals_only_missing_last_tool_result`；`init_context_state_heals_all_missing_tail_tool_results`；`build_responses_input_translates_hydrate_recovered_interrupted_tool_result`；`chat_request_serializes_hydrate_recovered_tool_round_for_openai_wire` | 用户恢复一个尾部遗留半截 tool_call 的坏 session 时，系统会自愈后继续对话 | 构造 transcript 尾部为 `assistant.tool_calls` 缺 1..N 条 tool result → 触发 hydrate / resume | hydrate 会按最后一个 tool_call block 的原顺序补齐所有缺失的 `[interrupted]` tool result；若尾部被非 `tool` role 打断则拒绝猜测；OpenAI Completions 与 Responses 两条 provider wire 均可消费修复后的消息链 |
| E2E-CLI-078 | 自动 | `test_mid_turn_llm_failure_preserves_persisted_progress` | 用户遇到 mid-turn LLM 流失败后，已完成进度不应整轮蒸发 | mock LLM：先产出 `assistant + tool_calls` / 已完成 `tool_result`，随后返回致命流错误 → 读取 transcript | transcript 保留本轮 `user` + 已完成 `assistant/tool_result`；未形成完整 message 边界的半截 delta 不写入；CLI 错误文案含明确 stage |
| E2E-CLI-079 | 自动 | `test_failed_turn_keeps_progress_and_next_user_starts_new_turn` | 用户上一轮失败后直接再发一条消息，系统应在保留旧进度的基础上继续，而不是清空上一轮 | mock LLM：第一轮在 mid-turn 失败；第二轮发送新的 user | transcript 中允许相邻两条 `user`；第一轮已落盘进度仍存在；第二轮正常继续，不依赖 `/retry` |
| E2E-CLI-080 | 自动 | `test_background_followup_drain_uses_same_immediate_append_path` | 后台 follow-up / synthetic user 应与普通 user 走同一条即时落盘路径 | 触发 background completion 生成 follow-up → 在 drain 时检查 transcript | follow-up 仅在 drain 时追加一次，enqueue 阶段不写盘；落盘后的 row 带 `msg_id`，不会在 turn 末重复 append |


---

## 跨平台（无独立 E2E 编号）

与 [INTEGRATION_MERGE_AND_ACCEPTANCE.md](../agents/INTEGRATION_MERGE_AND_ACCEPTANCE.md) §4 **人工验收第 8 条**一致：在 Windows / macOS / Linux 条件具备时，各至少执行一次 `cargo build` + `cargo test`（或 CI matrix）。**不占用**上表编号；发布前在 checklist 中单独勾选。


