#!/usr/bin/env bash
# 集成测试二进制分类唯一来源。
# 默认进入并发组；只有 3x 连跑证明确实互踩或压垮机器时，才退回串行兜底组。

TOMCAT_INTEGRATION_PARALLEL_TESTS=(
  audit_tests
  event_tests
  agent_loop_tests
  bash_assignment_deny
  system_prompt_cwd_priority
  path_command_e2e
  cwd_lazy_prompt_e2e
  search_files_tests
  web_search_tool_tests
  web_fetch_tool_tests
  checkpoint_integration_tests
  chat_git_preflight_tests
  session_tests
  session_concurrency_tests
  plugin_tests
  llm_tests
  llm_gateway_toggle_tests
  context_management_tests
  plan_runtime_integration_tests
  plan_e2e_with_mock_llm_tests
  robustness_tests
  read_tool_tests
  resume_hydration_tests
  skill_tool_tests
  transcript_summary_integration_tests
  integration_gate_config_tests
  cli_tests
  checkpoint_cli_e2e
  resume_hydration_cli_e2e
  quickjs_e2e_tests
  long_lived_vm_tests
  hostcall_tests
  primitives_tools_tests
  tool_catalog_doc
  serve_multi_session
  serve_ask_question_tests
  serve_schema_fixture
  serve_robustness_tests
  serve_stdio_e2e
)

TOMCAT_INTEGRATION_SERIAL_TESTS=(
)

# 真 LLM E2E（需当前 OpenAI target 对应 key；部分 target 还需 DEEPSEEK_API_KEY / MIMO_API_KEY）。
# 默认快门禁不跑本组；显式 real-llm 层才运行。
# 这几个 target 现在会在各自测试进程内切到临时 HOME，把 `~/.tomcat/*` 隔离到
# 私有 tempdir；剩余并发约束只来自 provider API 限流，因此 nextest profile 把本组
# 限到 max-threads=2，而不再用 -j1 串行到底。
# 其中 `plan_real_llm_cli_e2e` 现在只保留 planning-only / exec-only 两条窄 CLI smoke，
# full completion / artifact / transcript 顺序 / EOF settlement 等重断言交给更快的
# inprocess/runtime 层；CLI 只保留 resume/build wiring、EXEC prompt 与 session 绑定。
# run-integration-tests.sh 默认跳过本组，用户/CI 需要时按需
# `./scripts/run-integration-tests.sh integration-real-llm` 显式触发。
TOMCAT_INTEGRATION_REAL_LLM_TESTS=(
  current_tail_guard_real_llm_tests
  openai_files_integration_tests
  openai_responses_integration_tests
  plan_real_llm_inprocess_tests
  plan_real_llm_cli_e2e
  reasoning_continuity_real_llm_tests
)

TOMCAT_INTEGRATION_REAL_LLM_CLI_TESTS=(
  test_user_background_bash_autofeed_real_llm_cli
  test_user_background_bash_blocking_waitslice_real_llm_cli
  test_user_background_bash_multiple_timeout_slices_real_llm_cli
  test_user_background_bash_midturn_followup_real_llm_cli
  test_user_background_bash_timeout_snapshot_stays_bounded_real_llm_cli
)

# OpenAI Responses wire 真链路子组：只收口到最终走 `api=openai-responses` 的验收入口。
# 用途：集中管理 LiteLLM / OpenAI Responses 线上的 live 验收，不混入 DeepSeek / Mimo 分支。
# 注意：
# - `reasoning_continuity_real_llm_tests` 只跑 OpenAI Responses 那条 case；
# - `openai_files_integration_tests` 仍需 `--ignored`，且仅在 `PI_LIVE_OPENAI_FILES=1`
#   时真正打外网，因为 Files 能力可能未在网关侧开启。
TOMCAT_INTEGRATION_OPENAI_RESPONSES_WIRE_COMMANDS=(
  "cargo test -j 1 --test openai_responses_integration_tests -- --nocapture --test-threads=1"
  "cargo test -j 1 --test reasoning_continuity_real_llm_tests openai_responses_roundtrip_replays_reasoning_items -- --nocapture --test-threads=1"
  "cargo test -j 1 --test openai_files_integration_tests -- --ignored --nocapture --test-threads=1"
)

