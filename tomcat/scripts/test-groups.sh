#!/usr/bin/env bash
# 集成测试二进制分类唯一来源。
# 默认进入串行组；确认无进程级全局状态、真实 Wasm 运行时或重子进程依赖后再移入并发组。

TOMCAT_INTEGRATION_PARALLEL_TESTS=(
  audit_tests
  event_tests
  agent_loop_tests
  bash_assignment_deny
  system_prompt_cwd_priority
  path_command_e2e
  cwd_lazy_prompt_e2e
  search_files_tests
  session_tests
  plugin_tests
  llm_tests
  openai_responses_integration_tests
  context_management_tests
  robustness_tests
  read_tool_tests
)

TOMCAT_INTEGRATION_SERIAL_TESTS=(
  cli_tests
  wasmedge_e2e_tests
  long_lived_vm_tests
  js_api_alignment_tests
  hostcall_tests
  primitives_tools_tests
  tool_catalog_doc
)

TOMCAT_WASMEDGE_TESTS=(
  wasmedge_e2e_tests
  long_lived_vm_tests
  js_api_alignment_tests
  hostcall_tests
  primitives_tools_tests
)
