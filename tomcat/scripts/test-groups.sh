#!/usr/bin/env bash
# 集成测试二进制分类唯一来源。
# 默认进入串行组；确认无进程级全局状态或重子进程依赖后再移入并发组。

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
  openai_responses_integration_tests
  context_management_tests
  plan_runtime_integration_tests
  plan_e2e_with_mock_llm_tests
  robustness_tests
  read_tool_tests
  resume_hydration_tests
  skill_tool_tests
)

TOMCAT_INTEGRATION_SERIAL_TESTS=(
  cli_tests
  checkpoint_cli_e2e
  openai_files_integration_tests
  resume_hydration_cli_e2e
  quickjs_e2e_tests
  long_lived_vm_tests
  hostcall_tests
  primitives_tools_tests
  tool_catalog_doc
)

# 真 LLM E2E（需 OPENAI_API_KEY；缺 key 时 fixture panic）。
# 这几个 target 串行运行：fixture 都会碰真实 `~/.tomcat` 盘目录，并发会互踩。
# 其中 `plan_real_llm_cli_e2e` 现在只保留 planning-only / exec-only 两条窄 CLI smoke，
# full completion / artifact / transcript 顺序 / EOF settlement 等重断言交给更快的
# inprocess/runtime 层；CLI 只保留 resume/build wiring、EXEC prompt 与 session 绑定。
# run-integration-tests.sh 显式跳过本组，用户/CI 需要时按需 `cargo test --test plan_real_llm_*` 单独触发。
TOMCAT_INTEGRATION_REAL_LLM_TESTS=(
  current_tail_guard_real_llm_tests
  plan_real_llm_inprocess_tests
  plan_real_llm_cli_e2e
  reasoning_continuity_real_llm_tests
)

