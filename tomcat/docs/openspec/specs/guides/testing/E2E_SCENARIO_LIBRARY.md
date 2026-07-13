# E2E 用户操作模拟场景库

> 本文件是 [E2E_TEST_SPEC.md](E2E_TEST_SPEC.md) §2 的规范性附件，列出覆盖全部 P0 User Stories 的用户操作模拟用例清单。新增用例须同步更新本文件。

## 用例编号规则


| 前缀           | 含义                                                         |
| ------------ | ---------------------------------------------------------- |
| E2E-CLI-NNN  | CLI 子进程 E2E 用例（`tests/cli_tests.rs`）                       |
| E2E-QJS-NNN  | `rquickjs` 插件运行时与相关集成验证（如 `tests/quickjs_e2e_tests.rs`、`tests/long_lived_vm_tests.rs`） |
| E2E-VSCEXT-NNN | VSCode 扩展 / 宿主 UI E2E（如 `tomcat-vscode-ext/src/test/suite/*.test.ts`、安装版 harness、后续 webview UI driver） |


---

## 验收方式列（人工 / 自动）

各 Story 表格中的 **验收** 列取值：

| 取值 | 含义 |
| --- | --- |
| 自动 | 以 `cargo test`（如 `cli_tests` / `quickjs_e2e_tests` / `long_lived_vm_tests`）通过为准即可。 |
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
| E2E-CLI-009 | 自动 | `test_doctor_reports_all_checks`             | doctor 输出含全部检查项                     | `tomcat init` → `tomcat doctor`             | exit 0；stdout 含 配置合法/内嵌资源/rquickjs                   |
| E2E-CLI-010 | 自动 | `test_init_idempotent`                       | 连续两次 init，第二次以现有配置为基线继续运行 model-first 向导               | `tomcat init` × 2（同 `HOME`）           | 两次均 exit 0；第二次 stdout 含「已存在配置文件」或「已更新配置文件」 |
| E2E-CLI-017 | 自动 | `test_workspace_add_cwd_e2e`                 | `tomcat workspace add --cwd` 添加当前目录           | `tomcat init` → `cd` 至临时目录 → `tomcat workspace add --cwd` → `tomcat workspace list` | add exit 0；list 含该目录绝对路径 |

**补充（幂等 PATH）**：`test_init_path_export_idempotent_in_shell_profile` — 同一 `HOME` 下连续两次 `init`，`$HOME/.zshrc` 中仅一条 `export PATH=`。


---

## Story 2 — 4 原语安全管控（通过 `tomcat code` 驱动）（11 条）

> 需要 `DEEPSEEK_API_KEY`；无 key 时必须 `panic!`，不得跳过。
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
| E2E-CLI-016C | 自动（需 `DEEPSEEK_API_KEY`） | `test_user_background_bash_autofeed_real_llm_cli` | 用户让助手启动后台 bash、先做独立工作，然后只能依赖 `<background-task-finished ...>` 自动回灌继续推进 | `tomcat code` + 单轮 prompt：`bash(run_in_background=true)` 写 `BG_DONE`，立即写 `MARKER`，随后禁止 `task_output/task_list/task_stop` 与新 bash，只能等系统 follow-up | exit 0；stderr 含 `[bg] task ... queued for next turn`；`bg_done.txt` 与 `marker.txt` 真实落盘；stdout 含 `AUTOFEED_OK` |
| E2E-CLI-016D | 自动（需 `DEEPSEEK_API_KEY`） | `test_user_background_bash_blocking_waitslice_real_llm_cli` | 用户要求助手在后台 bash 后立即进入 `task_output(block=true)`，直到拿到真实新输出再收尾 | `tomcat code` + 单轮 prompt：后台 bash `sleep 2; echo TOKEN_WAITSLICE; printf BLOCKWAIT_DONE > file; sleep 30`，必须用 `task_output(block=true, timeout_ms=300)` + `task_stop` 完成，不准 `task_output(block=false)` | exit 0；stdout 含 `BLOCKWAIT_OK`；非 TTY stderr 不含 `waiting_for_output`；transcript 至少 1 次 `task_output(block=true, timeout_ms=300)`，并出现 `TOKEN_WAITSLICE` 与 `task_stop`；产物文件内容正确 |
| E2E-CLI-016E | 自动（需 `DEEPSEEK_API_KEY`） | `test_user_background_bash_multiple_timeout_slices_real_llm_cli` | 用户要求助手在同一后台任务上经历至少两次 timeout slice，再继续等到一次 `new_output` | `tomcat code` + 单轮 prompt：后台 bash `sleep 8; echo TOKEN_MULTI_TIMEOUT; printf MULTI_TIMEOUT_DONE > file; sleep 30`，必须重复 `task_output(block=true, timeout_ms=300)` 直到看到 token，再 `task_stop` | exit 0；stdout 含 `MULTI_TIMEOUT_OK`；非 TTY stderr 不含 `waiting_for_output`；transcript 中 `task_output` 至少 3 次、timeout 结果至少 2 次，最终出现 `TOKEN_MULTI_TIMEOUT` 与 `task_stop`；产物文件内容正确 |
| E2E-CLI-016F | 自动（需 `DEEPSEEK_API_KEY`） | `test_user_background_bash_midturn_followup_real_llm_cli` | 用户先起后台 bash，再执行一个耗时 foreground batch；只有在 foreground batch 结束后的后续请求里看见 `<background-task-finished ...>`，助手才允许继续验证结果 | `tomcat code` + 单轮 prompt：先 `bash(run_in_background=true)` 写 `BG_DONE`，再 foreground `bash` 睡眠并写 `FG_DONE`，禁止 `task_output/task_list/task_stop`；若 foreground batch 结束时仍未看到系统消息则必须回复 `MIDTURN_MISSED_FOLLOWUP` 并停止 | exit 0；stdout 含 `MIDTURN_FOLLOWUP_OK` 且不含 `MIDTURN_MISSED_FOLLOWUP`；两份文件真实存在且内容正确；transcript 中 `<background-task-finished ...>` 位于 `FG_BATCH_START` 之后、成功词之前；不出现 `task_output/task_list/task_stop` |
| E2E-CLI-016G | 自动（需 `DEEPSEEK_API_KEY`） | `test_user_background_bash_timeout_snapshot_stays_bounded_real_llm_cli` | 用户让助手观察一个几乎永不结束的后台 bash；在 EOF 处拿到 `timeout` 返回的 tail 快照后必须停止轮询，不能 busy loop | `tomcat code` + 单轮 prompt：后台 bash `printf HUNG_TIMEOUT_BOOT; sleep 60`；先用一次 `task_output(block=true)` 吃掉首个 `new_output`，再用第二次 `task_output(block=true)` 在 EOF 处命中 timeout 快照，随后禁止继续 poll / task_stop / task_list | exit 0；stdout 含 `HUNG_TIMEOUT_BOUNDED_OK`；非 TTY stderr 不含 `waiting_for_output`；transcript 中 `task_output` 次数有上界（<= 3）且至少 2 次；存在带 `HUNG_TIMEOUT_BOOT` 的 timeout 工具结果；`role=user` 不含 `waiting_for_output`；不出现 `task_stop/task_list` |
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

## Story 3 — 插件管理 CLI（6 条）

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

## Story 3b — PackageManager 统一安装（4 条）

> 主验收入口为 `tests/cli_tests.rs`。027–030 覆盖 shell CLI 的 install / packages / uninstall 黑盒路径；真实 TTY chooser/cancel/shadow warning 与 code/claw 会话内 `/install` live refresh 观感仍需按人工清单补验。

| 编号          | 验收 | 用例名                                                | 用户意图                                            | 操作序列                                                                 | 必须断言                                                                 |
| ----------- | -- | -------------------------------------------------- | ----------------------------------------------- | -------------------------------------------------------------------- | -------------------------------------------------------------------- |
| E2E-CLI-027 | 自动 | `test_user_installs_scope_package_and_lists_layered_packages` | 用户把 package 安装到当前项目，并确认三层 package 视图可见 | `tomcat install <package_dir> --scope-root <project>`（非交互默认 scope）→ `tomcat packages --scope-root <project>` | install exit 0；scope 层 `plugins/`、`skills/`、`packages/registry.json`、`plugins/registry.json` 均落盘；packages 输出含 `scope/agent/global` 三层与资源摘要 |
| E2E-CLI-028 | 自动 | `test_user_installs_bare_plugin_to_agent_layer`     | 用户把 bare plugin 安装到 agent 私有层                  | `tomcat install <plugin_dir> --visibility agent --scope-root <project>` → `tomcat packages --visibility agent --scope-root <project>` | install exit 0；agent 层 package/plugin registry 写入成功；packages 输出含 `[barePlugin]` 与 `plugin:<id>` |
| E2E-CLI-029 | 自动 | `test_user_installs_bare_skill_to_global_layer`     | 用户把 bare skill 安装到全局共享层                        | `tomcat install <skill_dir> --visibility global --scope-root <project>` → `tomcat packages --visibility global --scope-root <project>` | install exit 0；global 层 `skills/` 与 `packages/registry.json` 落盘；packages 输出含 `[bareSkill]` 与 `skill:<name>` |
| E2E-CLI-030 | 自动 | `test_user_uninstalls_scope_package_and_cleans_scope_layer` | 用户卸载 scope package 后，资源与账本被精准清理               | 先 `tomcat install <package_dir> --visibility scope --scope-root <project>` → 再 `tomcat uninstall <package_name> --visibility scope --scope-root <project>` → `tomcat packages --visibility scope --scope-root <project>` | uninstall exit 0；scope 层 plugin/skill 目录消失；`packages/registry.json` 与 `plugins/registry.json` 清空；packages 回到 `(none)` |
| E2E-CLI-031a | 自动 | `test_scope_function_override_wins_over_agent_and_global` | 用户在 scope / agent / global 三层各安装一个同 `point` 的 host-facing function 插件，当前项目应只看到 scope 赢家 | 预置三层 `plugins/<id>/plugin.json`，三者都声明同一 `functions[].point` → 构造/刷新当前 project scope → 读取 `FunctionRegistry` 或等价宿主调用视图 | `FunctionRegistry.functions_for_point(point)` 长度为 1；赢家来自 scope 层；agent/global 同点位 provider 不进入当前 scope 有效视图 |
| E2E-CLI-031b | 自动 | `test_lower_layer_function_reappears_after_scope_override_removed` | 用户移除高层覆盖插件后，低层同 `point` provider 在 refresh 后重新生效 | 先构造 global + scope 两层同 `point` 插件并确认 scope 获胜 → 删除 scope 层插件目录或卸载 scope package → refresh plugin catalog inventory | refresh 后 `FunctionRegistry.functions_for_point(point)` 仍长度为 1；赢家切回 global；不出现重复条目 |
| E2E-CLI-031c | 自动 | `test_web_search_backend_does_not_fallback_to_shadowed_provider` | 用户在高层覆盖 `web_search.backend` 后，即使赢家返回 `unsupported_backend`，宿主也不应自动回落到低层 provider | 预置高层和低层两个 `web_search.backend` 插件；高层返回 `unsupported_backend`，低层本可成功 → 触发宿主 `web_search` 调用 | 返回 `BackendFailure::Incompatible` 或等价清晰错误；低层 provider 不被调用；跨插件 fallback 不再发生 |


---

## Story 4 — rquickjs 插件运行时与兼容层（5 条）

> 主验收入口为 `tests/quickjs_e2e_tests.rs` 与 `tests/long_lived_vm_tests.rs`；对应架构入口见 [plugin-system-overview.md](../../../../architecture/plugin-system-overview.md)，运行时细节见 [plugin-system/runtime-and-sandbox.md](../../../../architecture/plugin-system/runtime-and-sandbox.md)。事件语义补充见 [plugin-system/events.md](../../../../architecture/plugin-system/events.md)。

| 编号          | 验收 | 用例名                                  | 用户意图                                   | 操作序列                                                                 | 必须断言                                                                 |
| ----------- | -- | ------------------------------------ | -------------------------------------- | -------------------------------------------------------------------- | -------------------------------------------------------------------- |
| E2E-QJS-001 | 自动 | `run_script_console`                 | 插件脚本可在 rquickjs 中使用 `console` / microtask / timer | `PluginEngine.create_instance()` → `run_script()` 执行 `console.log/error`、`Promise.resolve()`、`setTimeout()` | host binding 收到 `log/error/microtask/timer` 四类日志；脚本无崩溃 |
| E2E-QJS-002 | 自动 | `pi_readfile_llm`                    | 插件通过 `pi.readFile()` 与 `pi.complete()` 走宿主 bridge | `start_session_vm` → `dispatch_session_event(session_start)`，脚本内 `await pi.readFile()` + `await pi.complete()` | VM 保持 `Running/Idle`；`readFile` 返回 mock 内容；LLM 返回 mock `"hi"` |
| E2E-QJS-003 | 自动 | `shims_and_crypto_work_in_session_vm` | Tier-A 垫片与同步 crypto 在 session VM 内可用 | `start_session_vm` → `dispatch_session_event(session_start)`，脚本验证 `path/util/events/Buffer/crypto` | `sha256/hmac/randomBytes` 正常；VM 健康；`end_session` 后 RuntimeManager 为空 |
| E2E-QJS-004 | 自动 | `runaway_plugin_interrupted`         | 插件跑飞后被 interrupt budget / timeout 掐断并可重建 | `start_session_vm` → `dispatch_session_event(loop)` 触发死循环 → 再次 `start_session_vm` | 首个 VM 进入 `Error`；再次启动返回新 handle 且可恢复 `Running/Idle` |
| E2E-QJS-005 | 自动 | `panicking_plugin_isolated`          | 一个插件抛错后不连坐同 session 下其他插件         | 同 session 启动 crashy + healthy 两个插件 → 仅向 crashy 分发异常事件        | crashy 保持 `Running/Idle`；healthy 保持 `Running`；`end_session` 后 RuntimeManager 清空 |


---

## Story 5 — 宿主工具注册（2 条）


| 编号          | 验收 | 用例名                                                 | 用户意图                                       | 操作序列                                               | 必须断言                                         |
| ----------- | -- | --------------------------------------------------- | ------------------------------------------ | -------------------------------------------------- | -------------------------------------------- |
| E2E-QJS-011 | 自动 | `registered_tool_surfaces_to_tool_registry`         | 插件通过 `registerTool` 注册工具后宿主 registry 可见      | `load_plugin` → 读取 `ToolRegistry`                 | 注册成功；工具元数据可见；无崩溃 |
| E2E-CLI-031 | 人工 | `test_user_tool_registered_by_plugin_can_be_called` | 插件注册的工具可被对话模式调用（需 OPENAI_API_KEY）        | load_plugin + `tomcat code` + 触发工具的 prompt，timeout 60s | stdout 含工具执行结果或调用确认                          |


---

## Story 6 — 事件系统（4 条）


| 编号          | 验收 | 用例名                                               | 用户意图                                           | 操作序列                                                                     | 必须断言                                                            |
| ----------- | -- | ------------------------------------------------- | ---------------------------------------------- | ------------------------------------------------------------------------ | --------------------------------------------------------------- |
| E2E-QJS-021 | 自动 | `dispatch_events_once_returns_listener_id`        | 插件注册 `once` 监听后，宿主为其分配监听标识                     | `dispatch("events", "once", ...)`                                        | 返回 listener id；协议字段完整 |
| E2E-QJS-022 | 自动 | `dispatch_events_off_removes_listener`            | 插件取消监听后，宿主侧监听记录被移除                              | `dispatch("events", "off", ...)`                                         | 返回成功；监听条目被删除 |
| E2E-QJS-023 | 自动 | `shims_and_crypto_work_in_session_vm`             | session VM 中事件 shim 可用并与其他基础 shim 共存                | `start_session_vm` → `dispatch_session_event(session_start)`             | `events.EventEmitter` 可正常工作；VM 保持健康 |
| E2E-QJS-024 | 自动 | `dispatch_event_isolates_non_fatal_handler_errors` | 同一事件的某个 handler 抛错后，后续 handler 仍继续执行            | `PluginVmInstance.run_script()` 注册两个 `pi.on("demo")` handler，第一个抛错、第二个写标记，再触发 `__pi_dispatch_event(...)` | 后续 handler 仍执行；非 fatal 错误只影响当前 handler，不得中断同事件剩余 handler |


---

## Story 7 — LLM 统一接入（2 条）

> 需要 `DEEPSEEK_API_KEY`；无 key 时必须 `panic!`。
> **验收**：与 §4 人工验收「流式输出、对话模式观感」对齐，建议**人工补验**终端流式表现。


| 编号          | 验收 | 用例名                                                | 用户意图                 | 操作序列                                 | 必须断言                             |
| ----------- | -- | -------------------------------------------------- | -------------------- | ------------------------------------ | -------------------------------- |
| E2E-CLI-041 | 人工 | `test_user_chats_with_llm_gets_streaming_response` | 用户与 LLM 对话，获得流式渲染回复  | `tomcat code` + stdin 一句话，timeout 60s    | exit 0；stdout 含 AI 回复；有对话 banner |
| E2E-CLI-042 | 人工 | `test_user_receives_nonempty_llm_response`         | 确认 LLM 回复内容非空（基础连通性） | `tomcat code` + stdin `说一个字`，timeout 30s | exit 0；stdout 非空                 |
| E2E-CLI-043 | 自动 | `test_user_toggles_thinking_display_modes` | 用户运行时通过 `/thinking` 切换 CLI thinking 显示档位（minimal/summary/full/toggle） | 构造 `CliTurnRenderer` + mock `thinking_delta` 流 → 顺序应用 `/thinking summary` / `minimal` / `full` / `toggle` → 各档位发出同样的 summary+raw delta | summary 模式：可见 summary、隐藏 raw、`[thinking]` 仅 1 次；minimal 模式：仅 `[thinking] ...` 占位；full 模式：summary+raw 同时可见；toggle 循环 `summary→full→minimal→summary`；自动化见 `src/api/chat/tests/cli_turn_renderer_test.rs` + `src/api/chat/commands/tests/cmd_thinking_test.rs` |
| E2E-CLI-044 | 自动 | `llm_error_renders_red_status_line` | Responses `response.failed` / 顶层 `error` 被映射为结构化 LLM 错误并在 CLI 明确显示 | 构造 `CliTurnRenderer` + `llm_error` payload，模拟正文后收到错误终局 | stderr 含红色 `[llm] <error message>`；不复用 `ExtensionError`；自动化见 `src/api/chat/tests/cli_turn_renderer_test.rs` + `src/core/agent_loop/tests/stream_handler_test.rs` |
| E2E-CLI-045 | 自动 | `llm_notice_renders_dim_non_error_hint` | Responses `incomplete/max_output_tokens` 走非错误轻提示而非红字报错 | 构造 `CliTurnRenderer` + `llm_notice{finishReason=max_output_tokens}` payload | stderr 含灰色轻提示、包含 `max_output_tokens`，且**不**出现红色错误样式；自动化见 `src/api/chat/tests/cli_turn_renderer_test.rs` + `src/core/agent_loop/tests/stream_handler_test.rs` |


---

## Story 8a — 异步 Hostcall 与 JS API 对齐（2 条）

> 主验收入口为 `src/ext/tests/instance_bridge_test.rs` 与 `tests/quickjs_e2e_tests.rs` 中的 async bridge 相关用例。

| 编号          | 验收 | 用例名                                        | 用户意图                                           | 操作序列                                                                 | 必须断言                                                                 |
| ----------- | -- | ------------------------------------------ | ---------------------------------------------- | -------------------------------------------------------------------- | -------------------------------------------------------------------- |
| E2E-QJS-041 | 自动 | `pi_readfile_llm`                          | 插件通过 `async/await` 调宿主 API 时，pending→poll→ready 链路可正常 resolve | `start_session_vm` → `dispatch_session_event(session_start)`，脚本内 `await pi.readFile()` / `await pi.complete()` | Promise resolve 成功；返回值契约与 ExtensionAPI 一致；VM 保持 `Running/Idle` |
| E2E-QJS-042 | 自动 | `async_poll_refreshes_budget_before_each_host_poll` | 长轮询 async Hostcall 在 pending→poll→ready 期间不会因 interrupt budget 耗尽被误杀 | `PluginVmInstance.run_script()` mock 一次 pending + 一次 ready，并覆盖 `__pi_budget_reset` 计数 | 每次 timer callback / host poll 前都会刷新预算；最终请求正常 resolve，且 poll fixture 至少走过一次 pending |

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

## Story 8c — Agent Server / UI Gateway（6 条）

> 主验收入口为 `tests/serve_stdio_e2e.rs`、`tests/serve_multi_session.rs`、`tests/serve_ask_question_tests.rs`、`tests/serve_robustness_tests.rs`、`tests/serve_schema_fixture.rs`。
> **验收**：以自动化为主；目标是把 `tomcat serve --stdio` 的协议、并发、审批回环、错误模型与 schema 漂移锁住，供 IDE / GUI 宿主稳定集成。

| 编号 | 验收 | 用例名 | 用户意图 | 操作序列 | 必须断言 |
| ---- | ---- | ------ | -------- | -------- | -------- |
| E2E-CLI-096 | 自动 | `serve_stdio_user_roundtrip_e2e` | IDE/GUI 父进程以 stdio 启动 `tomcat serve` 并完成一次基础问答回合 | spawn `tomcat serve --stdio` → `initialize` → `prompt` → EOF | `initialize` 回 `protocolVersion=1`；stdout 全为 NDJSON；`prompt` 至少产生 `agent_start/message_update/agent_end`；EOF 退出码为 0 |
| E2E-CLI-097 | 自动 | `serve_multi_session_concurrency_and_isolation` | 宿主同时驱动两个不同 `sessionId`，确认跨会话真并发且事件不串台 | `initialize` → `new_session` → 对 `s1/s2` 并发 `prompt` | 两个会话都收到各自 `agent_start/agent_end`；快会话可先于慢会话收敛；所有事件的 `sessionId` 与目标会话一致 |
| E2E-CLI-098 | 自动 | `serve_same_session_is_busy_until_turn_finishes` | 宿主在同一会话未收敛时再次 `prompt`，应拿到明确 `busy` 语义 | 单会话 `prompt`（慢响应）→ 立刻第二次 `prompt` 同 `sessionId` | 第二条命令收到 `response{success:false,error:"busy"}`；首轮仍正常收敛为 `agent_end` |
| E2E-CLI-099 | 自动 | `serve_ask_question_roundtrip_resumes_turn`；`serve_ask_question_cancel_roundtrip_does_not_hang` | 宿主收到 `ask_question` 控制请求后，能走回答 / 取消两条回环并让 turn 正常收口 | LLM 首轮返回 `ask_question` tool call → 宿主回 `control_response` 或 `control_cancel` → 继续下轮 LLM | `control_request{subtype=ask_question}` 含稳定 `requestId`；回答/取消都能触发后续 LLM 请求并最终收敛到 `agent_end`，不得卡死 |
| E2E-CLI-100 | 自动 | `serve_parse_error_does_not_break_following_initialize`；`serve_eof_exits_cleanly`；`serve_print_schema_matches_fixture` | 宿主面对坏输入、EOF 与 schema 导出时获得可恢复且可审计的行为 | 发送坏行 → 再 `initialize`；或直接 EOF；或执行 `serve --print-schema` | 坏行返回结构化 `parse_error` 且不打断后续初始化；EOF 干净退出无 panic；`serve.schema.json` / `serve.d.ts` 与 committed fixture 无漂移 |
| E2E-CLI-101 | 自动 | `serve_prompt_with_attachment_roundtrip` | 宿主通过 `prompt.params.attachments` 发送多模态输入时，serve 能把附件装配进真实回合而不是降级成纯文本 | `initialize` → `prompt{text, params.attachments}`（`image`/`file` 任选其一）→ 等待回合收口 | 首轮命令被接受；下行仍是纯 NDJSON；附件经 `ChatMessage::user_with_parts(...)` 进入 agent 回合，最终正常收敛到 `agent_end` |

---

## Story 8d — VSCode Chat 扩展 Phase 2（8 条）

> 主验收入口为 `tomcat-vscode-ext/tests/*.test.ts`、`tomcat-vscode-ext/src/test/suite/host.e2e.test.ts`、`tomcat-vscode-ext/e2e-harness/src/test/installed.test.ts`，以及后续新增的 webview UI driver。
> **验收**：participant 与 webview 都要求走真实宿主用户路径；participant 需覆盖真实 chat UI，webview 需覆盖真实侧栏/DOM 交互与安装版 VSIX。允许保留较低层 API/harness 回归，但不得替代真实 UI 驱动验收。

| 编号 | 验收 | 用例名 | 用户意图 | 操作序列 | 必须断言 |
| ---- | ---- | ------ | -------- | -------- | -------- |
| E2E-VSCEXT-001 | 自动 | `test_participant_slash_plan_flow_via_real_chat_ui` | 用户在 VSCode 聊天框中输入 `@tomcat /plan`、`/plan build`、`/plan exit`，确认计划模式可驱动且状态可见 | 启动 VSCode Dev Host → 打开真实 chat UI → 新建聊天 → 依次输入 `@tomcat /plan`、`@tomcat /plan build`、`@tomcat /plan exit` | `set_plan_mode` 命令真实发出；事件流出现 `planState=planning/executing/chat` 或等价 UI 状态；`agent_end` 正常收口；不得把 `/plan` 当普通 prompt 文本喂给 LLM |
| E2E-VSCEXT-002 | 自动 | `test_participant_slash_model_picker_via_real_chat_ui` | 用户在 VSCode 聊天框中触发 `/model`，可看到模型列表、选择模型、取消无副作用 | 启动 VSCode Dev Host → 打开真实 chat UI → 输入 `@tomcat /model` → 真实选择一项或取消 | 先调用 `list_models`；选中时触发 `set_model` 且确认气泡显示当前模型；取消时无副作用；`get_state.model` 与 UI 标记一致 |
| E2E-VSCEXT-003 | 自动 | `test_webview_streams_thinking_tools_and_approval_cards` | 用户打开侧栏 webview，完成一轮真实消息发送，看到正文、thinking、工具卡与审批卡 | 启动 VSCode Dev Host / webview UI driver → 打开 `Tomcat` 侧栏 view → 输入消息 → 触发带 thinking/tool/ask_question 的回合 | webview 通过 `event` 通道渲染 `thinking_delta`、`tool_execution_*`、`ask_question`；审批回答后回合继续并收口；不回退到纯文本拼接 |
| E2E-VSCEXT-004 | 自动 | `test_webview_model_plan_and_multisession_controls` | 用户在 webview 顶部使用模型下拉、plan 开关和多会话 tab | 打开 webview → 切换模型 → 进入/退出 plan → 新建 2 个 tab 并切换 | `list_models` / `set_model`、`set_plan_mode`、`new_session` / `switch_session` / `close_session` 均被真实触发；tab 间 sessionId 不串台；plan 徽标随状态变化 |
| E2E-VSCEXT-005 | 自动 | `test_shared_scope_pool_and_single_owner_conflict_between_frontends` | 用户让 participant 与 webview 同时看到同一项目历史，但对同一 live 会话只能单前端驱动 | 启动 participant + webview → `list_sessions{scope:\"disk\"}` 枚举同一项目历史 → participant 激活会话 → webview 尝试驱动同一 session | 两端枚举到同一份历史，默认指向 `isCurrent`；第二前端只能只读/看到冲突提示；owner 释放后另一端可接管 |
| E2E-VSCEXT-006 | 自动 | `test_webview_diff_and_apply_reuses_vscode_editor_path` | 用户在 webview 工具卡中点击“看 diff / 应用编辑”，确认仍走 VSCode 原生编辑链路 | 打开 webview → 触发生成文件编辑的回合 → 点击 diff / apply | 宿主调用 `vscode.diff` + `WorkspaceEdit`；目标文件真实被修改；webview 不自建主编辑栈 |
| E2E-VSCEXT-007 | 自动 | `test_packaged_vsix_contains_gui_dist_and_installed_webview_loads` | 用户安装打包后的 VSIX 后，participant 与 webview 都能正常加载和使用 | 执行 VSIX 打包 → 安装版 harness 启动 VSCode → 打开 chat UI 与 webview | 包内含 `gui/dist`、不含 `gui/src`；安装版 participant 与 webview 都可加载；webview 无 CSP/资源加载错误 |
| E2E-VSCEXT-008 | 自动 | `test_webview_protocol_and_bridge_reuse_contract` | 宿主与 webview 的 typed `postMessage` 协议稳定，且 participant / webview 共用同一 `TomcatMessenger` 核心 | 运行扩展级集成 + 宿主 E2E；同时订阅 participant / webview | `messageId` / `state` / `event` 协议语义稳定；未知 id 安全丢弃；同一 `TomcatMessenger` 实例同时服务两个前端，核心行为不漂移 |

## Story 8e — VSCode webview transcript 仿 Chat 体验（11 条）

> 对应 [User_Stories.md](../../User_Stories.md) Story 8e；自动化入口 `tomcat-vscode-ext/e2e-harness/src/test/manual-acceptance.test.ts` / `installed.test.ts` + `gui/src/App.test.tsx` 单元测试。

| 编号 | 验收 | 用例名 | 用户意图 | 操作序列 | 必须断言 |
| --- | --- | --- | --- | --- | --- |
| E2E-VSCEXT-009 | 自动 | `test_user_webview_user_prompt_pill_and_assistant_no_card` | 用户发送消息后看到 VSCode Chat 风格布局 | 打开 webview → 发送 user prompt → 等待 assistant 回复 | DOM：`userPromptPill=true`（右对齐 pill、无 header）；`assistantNoCard=true`（无卡片边框/标签）；error/notice 保留左边框 |
| E2E-VSCEXT-010 | 自动 | `test_user_webview_thinking_tool_fold_groups_by_assistant_response` | 用户看到同 assistant message 的 tool 折叠为一组 | 触发含 thinking + 多 tool 的回合 | `assistantResponseGroups>=1`；折叠态 `toolRowFlat` 无 tool DOM（懒渲染）；展开后见 `toolRowFlat` 扁平行非大卡片；`groupFoldTitles` 含 LLM 摘要或回退标题 |
| E2E-VSCEXT-011 | 自动 | `test_user_webview_tool_row_read_filechip_open` | 用户点击 read 行 FileChip 打开文件 | 触发 read 工具回合 → 展开 tool 行 → 点击 FileChip | `fileChipOpen=true`；宿主打开对应文件 tab |
| E2E-VSCEXT-012 | 自动 | `test_user_webview_bash_tool_row_ran_command_expandable` | 用户看到 bash 折叠为 Ran \<cmd\> 且可展开输出 | 触发 bash 工具回合 | `toolRowFlat` 含 `Ran` 前缀；`toolRowExpandable=true`；无"打开终端"按钮 |
| E2E-VSCEXT-013 | 自动 | `test_user_webview_plan_executing_progress_row` | Plan 执行时底部显示 todo 进度 (N/M) | 进入 plan executing → 观察 live cluster 末尾 | `progressRow=true`；显示 `(current/total)` + 当前 todo title；PlanFileCard 同步 `planTodos` |
| E2E-VSCEXT-014 | 自动 | `test_user_webview_session_title_async_update` | 首条 user 后 session 标题从占位更新为 LLM 标题 | 新建 session → 发送首条 user 消息 | `sessionTitleUpdated=true`；SessionBar 标题从规则占位变为 LLM 生成标题 |
| E2E-VSCEXT-015 | 自动 | `test_user_webview_left_guide_line_and_ellipsis_above_group` | 折叠组视觉对齐 VSCode（竖线 + 阐述在折叠头上方） | 触发含阐述 + thinking + tool 的回合 | `leftGuideLine=true`；`ellipsisAboveGroupHeader=true`（阐述在折叠头上方可见） |
| E2E-VSCEXT-016 | 自动 | `test_user_webview_shimmer_while_summary_title_pending` | streaming 且 summaryTitle 未就绪时 header shimmer | 触发 thinking+tool 流式回合 | 折叠 header 含 shimmer class；summaryTitle 就绪后 shimmer 消失 |
| E2E-VSCEXT-017 | 自动 | `test_user_webview_switch_session_restores_plan_card_and_ctx` | 用户切到别的 session 再切回时，已有 active plan 的 transcript 状态能恢复，不需要重新等 live 事件 | webview 新建 session A → 触发带 `planPath` / `Ctx%` 的 transcript 场景 → 新建 session B → 切回 session A | DOM：`planCardCount===1`、`ctxLabel` 恢复、`planStateText` 与当前 `get_state.planState` 一致；Plan `Build` 控件仍可用；不得生成重复 plan 卡 |
| E2E-VSCEXT-018 | 自动 | `test_user_webview_reload_replays_plan_history_without_duplicate_card` | 用户 reload webview 后，plan 历史证据（review/verify）能从 `get_messages` 重放回来 | 触发含 custom `plan.review` / `plan.verify` / `plan.pending` 的回合 → 模拟 webview reload → 等待 bootstrap 完成 | DOM：`planNoticeReplayed=true`；review / verify notice 重新出现且各仅 1 次；`planCardCount===1`；plan footer 状态取 `get_state` 当前真相而不是旧历史 |
| E2E-VSCEXT-019 | 自动 | `test_user_webview_cross_owner_observes_plan_transitions_and_terminal_truth` | participant 持有会话时，webview 作为观察端仍能看到 plan enter/build/exit 与终态收敛 | participant 创建并持有 session → webview 切到该 session（只读）→ participant 触发 `plan.enter` / `plan.build` / `plan.exit` 生命周期 | 观察态 DOM：`hasConflict=true`，footer 依次显示 `Plan: planning` / `Plan: executing` / `null`；timeline 中同一路径始终单张 plan 卡；终态后 state 与 `get_state` 收敛，不残留 completed/pending 漂移 |
| E2E-VSCEXT-020 | 自动 | `test_user_webview_pick_context_routes_attachments_and_references` | 用户点击 `+` 选择图片、工作区文件与目录时，composer 能按类型分流为附件与上下文引用 | 打开 webview → 新建 session → 点击 `+` → 选择图片、工作区文件、工作区目录 | DOM：新增 1 个 `attachment-chip` 和 2 个 `composer-reference-chip`；标签分别显示图片文件名、代码文件名、目录名；state 中 pending attachment `kind=image` |
| E2E-VSCEXT-021 | 自动 | `test_user_webview_selection_reference_rehydrates_from_history` | 用户把编辑器选区加入 Tomcat 后，发送并 reload，历史 transcript 仍保留引用标签与行号 | 打开工作区文件并选中多行 → 执行 `Add Selection to Tomcat Chat` → webview 发送消息 → reload webview | composer chip 标题含 `<path>:<start>-<end>`；发送后的 user message `segments` 保留 selection reference；reload 后历史气泡仍显示相同文件名与行号标签 |
| E2E-VSCEXT-022 | 自动 | `test_user_webview_file_drop_reference_deduplicates_duplicate_paths` | 用户按住 Shift 拖入文件上下文时，composer 给出明确提示，并对重复文件引用去重 | 打开 webview → 观察 idle drag hint → drag over composer → 依次 drop 两个不同文件和一个重复文件 | idle 态提示“拖文件请按住 Shift”；drag over 时出现高亮与“松手加入上下文”；最终仅保留 2 个唯一 `composer-reference-chip`，重复路径不重复添加 |
| E2E-VSCEXT-023 | 自动 | `checkpoint_restore_layered_contract` | 用户在 webview transcript 中看到 checkpoint marker，并能走 Cancel / Don't revert / Revert 三态恢复 | 打开含 checkpoint marker 的 transcript → 点击 `Restore Checkpoint` → 依次验证 Cancel(Esc)、Don't revert、Revert | Cancel/Esc：不发 `restoreCheckpoint`、timeline 不变、composer 不回填；Don't revert：只截断 checkpoint 之后消息、文件不改；Revert：额外把工作区文件回滚到 checkpoint。当前先以分层自动化替代真实宿主 UI driver：`serve::tests::commands_test::serve_restore_checkpoint_reverts_files_and_reports_payload` 锁文件回滚与 payload，`gui/src/App.test.tsx` 锁三态交互，`tomcat-vscode-ext/tests/webview_provider_flow.test.ts` 锁宿主 refresh/history/checkpoints 重放。后续若补 webview UI driver，再把本条升级为真实宿主 E2E。 |
| E2E-VSCEXT-024 | 自动 | `checkpoint_refresh_keeps_latest_live_turn` | 用户刚结束一轮对话时，即使宿主只刷新 checkpoint、还没重新拉历史，最新 user/assistant 也不能从 transcript 消失 | bootstrap 旧 history + checkpoints → 发送新 prompt（local user 已确认）→ 流式收到 assistant reply → 触发 `turn_end`（内部会 `refreshCheckpoints`）→ 再手动触发一次 `listCheckpoints` intent | timeline 仍保留最新 prompt + reply；checkpoint 数据更新但 raw timeline 不出现物化 marker；`turn_end` 和 `listCheckpoints` 两条“单独刷新 checkpoints”路径都不会抖掉最新一轮。当前以分层自动化替代真实宿主 UI driver：`src/ui/webview/tests/state.test.ts` 锁 store 级重建回归，`src/ui/webview/tests/provider.test.ts` 锁 `refreshCheckpoints` 只更新 `session.checkpoints`，`tomcat-vscode-ext/tests/webview_provider_flow.test.ts` 锁真实 prompt + serve event 链路。 |
| E2E-VSCEXT-025 | 自动 | `checkpoint_count_matches_git_add_surface` | 用户在大仓库里修改被跟踪文件时仍应看到 checkpoint marker；若只改 `.gitignore` 忽略文件，则本就不应生成新 marker | 构造含大量 `target/`/`node_modules/`/自定义 ignored 目录的工作区 → 先改被跟踪文件，再单独改 ignored 文件 | 文件上限计数必须与 `git add -A` 口径一致：`.gitignore` 与 `DEFAULT_EXCLUDE_RULES` 不占上限；被跟踪文件改动可成功生成 checkpoint；只改 ignored 文件时复用最近一次 checkpoint、不新增 marker。当前以后端分层自动化替代真实宿主 UI driver：`src/core/checkpoint/tests/shadow_git_test.rs::record_ignores_default_excludes_and_gitignored_dirs_when_counting_limit` 锁“大量 ignored 文件不误占上限”，`ignored_only_changes_reuse_the_latest_checkpoint` 锁“ignored-only turn 不新建 checkpoint”。 |
| E2E-VSCEXT-026 | 自动 | `previous_turn_progress_settles_on_next_turn` | 用户发送新一轮后，上一轮的 thinking/loading/editing 必须立刻变成“过去态” | 构造“上一轮含 thinking + edit，新一轮 busy 但还没产出自己 tool/answer” 的 transcript / state 序列 | DOM 中上一轮不得残留 `thinking-streaming-indicator` / `tool-row-running-indicator`；edit 行文案应收敛为 `Edited`。当前先以分层自动化替代真实宿主 UI driver：`gui/src/components/TranscriptView.test.tsx` 锁 live cluster 的 streaming 归属，`src/ui/webview/tests/state.test.ts` + `provider.test.ts` 锁 `agent_idle` 收敛 running 工具卡，`gui/src/App.test.tsx` 锁整条 UI 链路。 |
| E2E-VSCEXT-027 | 自动 | `new_prompt_reveals_top_then_sticky` | 用户发送新提示词后，先看到它被滚到顶部；当当前轮超一屏时，再切到当前轮 sticky | 构造“历史一轮 + 新 user 与第一条 thinking 同帧到达 + 后续内容继续增长”的 transcript / state 序列 | 旧 sticky 先隐藏；最新 user 必须 reveal 到视口顶部；同帧 assistant/thinking 不得吞掉 reveal；当前轮超一屏后自动切回 follow-bottom，并显示当前轮 sticky。当前先以分层自动化替代真实宿主 UI driver：`gui/src/useAutoScroll.test.tsx` 锁 `latestUserMessageId` 触发、prepend/restore 护栏与超一屏切回 follow-bottom，`gui/src/App.test.tsx` 锁 reveal→sticky 整链路。 |
| E2E-VSCEXT-028 | 自动 | `reveal_survives_reset_and_reset_wins_on_collision` | reveal 触发不能被同帧的 `resetKey` 变化吞掉（0.1.12 真机不置顶的真因）；会话切换/重挂时 reset 又必须权威、不残留半吊子 reveal | ① 会话切换帧同时携带全新 latest user（`resetKey`+`latestUserMessageId`+`oldestItemKey` 同帧变化）；② 会话加载后紧接着在同一会话发送新提示词 | ①（collision）走加载语义：落底、无 reveal spacer、`following`，不残留 sticky；②（remount 后发送）同会话追加必定 reveal 到顶（`resetKey`/`oldestItemKey` 不变），reset 分支不会把触发 ref 洗成“已见过”。逻辑不变量用 `gui/src/useAutoScroll.test.tsx` 以 store props 驱动锁定（不 mock 触发）；纯布局/时序真因用**真实浏览器 smoke** 复验：`npm run dev` 起 GUI 独立跑，`window.postMessage` 灌“历史一轮→echo 新 user→流式”帧序列，CDP 读 `scrollTop`/`[data-message-kind="user"]` 的 `getBoundingClientRect().top` 判断是否 reveal 到顶（jsdom mock 布局对此类真因恒绿、测不出）。 |
| E2E-VSCEXT-029 | 自动 | `reveal_survives_viewport_growth_on_busy_flip` | reveal 到顶后，`busy` 翻转导致 composer 变高、stream 容器 `clientHeight` 变大时，最新 user 必须**保持置顶**，不得漂回视口下方（0.1.13 真机不置顶的真因） | 复刻真机时序：`busy=false` 的 echo 帧（reveal 到顶，spacer 按旧视口算）→ `busy=true` 翻转帧（`clientHeight` 变大、`contentKey` 不变）→ 流式 thinking/tool | `busy` 翻转后 `clientHeight` 变化触发 `ResizeObserver` 重算 spacer 并**重新固顶**（spacer 可增大）；即使钳底 `scroll` 先到，只要当前轮仍装得下（`latestTurnHeight <= clientHeight`）就重新固顶而非切 follow-bottom；只有真正超一屏才 follow-bottom。逻辑不变量用 `gui/src/useAutoScroll.test.tsx` 以**可变 `clientHeight`** 驱动锁定（`re-pins the reveal when the viewport grows...` / `re-pins on the resize-induced scroll clamp...` 两例）；真因用**真实浏览器 smoke** 复验：起**生产构建 `gui/dist`**（非 dev 版）静态服务，灌 `busy=false→true` 翻转帧序列，CDP 读 `[data-message-kind="user"]` 的 `getBoundingClientRect().top` 在翻转后是否仍为 0（jsdom mock `clientHeight` 恒定、对此真因恒绿、测不出）。 |

---

## Story 8b — 长生命周期 VM 与有状态插件（5 条）

> 主验收入口为 `tests/quickjs_e2e_tests.rs`、`tests/long_lived_vm_tests.rs` 与 `src/ext/plugin/tests/suite_test.rs`。
> **验收**：以自动化为主；若涉及真实插件样例，可再按需做人工 spot-check。


| 编号          | 验收 | 用例名                                                       | 用户意图                 | 操作序列                                                                                                                                       | 必须断言                                                                        |
| ----------- | -- | --------------------------------------------------------- | -------------------- | ------------------------------------------------------------------------------------------------------------------------------------------ | --------------------------------------------------------------------------- |
| E2E-QJS-031 | 自动 | `test_multi_session_isolation_in_runtime_manager`         | 多会话上下文隔离             | 为两个不同 session 插入同一 plugin id 的 runtime handle                                                                                             | 两个 session 状态互不影响                                             |
| E2E-QJS-032 | 自动 | `test_session_cleanup_removes_all_handles_for_session`    | 会话结束后本 session 的 VM 全量清理 | 插入多个 runtime handle → `remove_session(sess-1)`                                                                                              | 仅目标 session 被清理；其余 session 保留 |
| E2E-QJS-033 | 自动 | `runaway_plugin_interrupted`                              | 跑飞插件被中断预算/超时掐断后可恢复     | `start_session_vm` → 触发死循环事件 → 再次 `start_session_vm`                                                                                     | 首个 VM 进入 `Error`；后续能重建恢复 |
| E2E-QJS-034 | 自动 | `panicking_plugin_isolated`                               | 单个插件抛错不连坐其他插件        | 同 session 启动 crashy + healthy 插件 → 仅向 crashy 分发异常事件                                                                                       | crashy 保持 `Running/Idle`；healthy 保持运行 |
| E2E-QJS-035 | 自动 | `start_session_vm_opportunistically_reaps_expired_runtime` | idle VM 按机会式语义回收     | 配置较短 `idle_ttl_ms` → 等待过期 → 再次 `start_session_vm`                                                                                           | 过期 runtime 在新活动进入时被顺手回收 |


---

## Story 9 — AgentLoop 核心结构（TASK-14，3 条 + TASK-17 上下文管理 3 条）

> 需要 `DEEPSEEK_API_KEY`；无 key 时必须 `panic!`（符合 INTEGRATION_TEST_SPEC §5.2）。
> **验收**：与 §4 人工验收「对话模式、多轮上下文、会话切换」对齐，建议**人工补验** AgentLoop 与终端交互体验。


| 编号          | 验收 | 用例名                                               | 用户意图                                         | 操作序列                                                                                      | 必须断言                                         |
| ----------- | -- | ------------------------------------------------- | -------------------------------------------- | ----------------------------------------------------------------------------------------- | -------------------------------------------- |
| E2E-CLI-081 | 人工 | `test_user_chat_non_interactive_with_prompt_flag` | 用户启动 `tomcat code` 并输入单句提问，AgentLoop 执行并输出 AI 回复 | `tomcat init` → `tomcat code`（stdin: `"Reply with exactly: pong\n"`，timeout 60s，含 DEEPSEEK_API_KEY） | exit 0；stdout 非空（AI 已通过 AgentLoop::run() 回复） |
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
> **TASK-21 备注**：§5.7 消息级 ID、锚点插入、`S::E`：`src/core/session/transcript/tests.rs` 与 `context_management_tests.rs` 中重载/边界用例对齐 JSONL 行序与 fold。**§5.7.5.1 陈旧 apply** 见 **E2E-CLI-092**；**read_entries_tail 跳过未知 type** 见 **E2E-CLI-093**。开发阶段不读盘兼容 `type: compaction`，见 [session-storage.md](../../../../architecture/session-storage.md) transcript 说明。
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


