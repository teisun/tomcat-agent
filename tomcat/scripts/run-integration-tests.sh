#!/usr/bin/env bash
# 集成测试：默认路径不依赖外部 Wasm 运行时。
# 测试执行按资源需求分类：单元测试默认并发；集成测试分为并发组与串行组。
# 非 TTY 下强制 EDITOR/PAGER 为无交互，避免子进程阻塞；说明见 docs/reports/integration_test_hang_remediation.md。
#
# 用法（在项目根）：
#   ./scripts/run-integration-tests.sh                      # 全量：release → clippy → lib → integration
#   ./scripts/run-integration-tests.sh all                  # 同上
#   ./scripts/run-integration-tests.sh release              # 仅 cargo build --release
#   ./scripts/run-integration-tests.sh clippy               # 仅 cargo clippy --all-targets -- -D warnings
#   ./scripts/run-integration-tests.sh lib                  # 仅库内单元测试（默认并发）
#   ./scripts/run-integration-tests.sh integration          # 并发组 + 串行组
#   ./scripts/run-integration-tests.sh integration-parallel # 仅可并发的 integration crate
#   ./scripts/run-integration-tests.sh integration-serial   # 仅必须串行的 integration crate
#   ./scripts/run-integration-tests.sh integration-real-llm # 真 LLM E2E（需当前 OpenAI target 对应 key + DEEPSEEK_API_KEY + MIMO_API_KEY）
#   ./scripts/run-integration-tests.sh integration-openai-responses-wire # 只跑 OpenAI Responses wire 真链路组（需当前 OpenAI target 对应 key）
#
# 未知子命令：打印用法并 exit 2。
set -e

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

. "$REPO_ROOT/scripts/test-groups.sh"

# 非 TTY（IDE/CI/管道）下若继承 EDITOR=vim 等，子进程会阻塞等输入，表现为测试「卡死」。
# 本脚本对由此启动的 cargo 子进程统一使用无交互编辑器与 pager。
export EDITOR=true
export VISUAL=true
export GIT_EDITOR=true
export PAGER=cat
export GIT_PAGER=cat

log_phase() {
  echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] $* ==="
}

build_test_args() {
  local test_name
  for test_name in "$@"; do
    printf '%s\n' "--test"
    printf '%s\n' "$test_name"
  done
}

openai_responses_target() {
  printf '%s' "${TOMCAT_E2E_OPENAI_TARGET:-gpt-5.4}"
}

openai_responses_key_env() {
  local target
  target="$(openai_responses_target)"
  case "$target" in
    gpt-5.2|gpt-5.4|gpt-5.5|gpt-5.6)
    printf '%s' "OPENAI_API_KEY"
    ;;
    *)
    printf '%s' "${TOMCAT_E2E_OPENAI_KEY_ENV:-OPENAI_GATEWAY_API_KEY}"
    ;;
  esac
}

missing_required_envs_for_real_llm() {
  local openai_key_env
  openai_key_env="$(openai_responses_key_env)"
  if [ -z "${!openai_key_env}" ]; then
    printf '%s\n' "$openai_key_env"
  fi
  if [ -z "$DEEPSEEK_API_KEY" ]; then
    printf '%s\n' "DEEPSEEK_API_KEY"
  fi
  if [ -z "$MIMO_API_KEY" ]; then
    printf '%s\n' "MIMO_API_KEY"
  fi
}

run_release() {
  log_phase "开始 release: cargo build --release"
  cargo build --release
  local status=$?
  log_phase "结束 release"
  return $status
}

run_clippy() {
  log_phase "开始 clippy: cargo clippy --all-targets -- -D warnings"
  cargo clippy --all-targets -- -D warnings
  local status=$?
  log_phase "结束 clippy"
  return $status
}

run_lib() {
  log_phase "开始 lib: cargo test --lib（默认并发；少数全局状态用例用 serial_test 串行）"
  cargo test --lib -- --nocapture
  local status=$?
  log_phase "结束 lib"
  return $status
}

run_integration_parallel() {
  local args=()
  while IFS= read -r arg; do
    args+=("$arg")
  done < <(build_test_args "${TOMCAT_INTEGRATION_PARALLEL_TESTS[@]}")

  log_phase "开始 integration-parallel（可并发 integration crate）"
  cargo test --no-fail-fast "${args[@]}" -- --nocapture
  local status=$?
  log_phase "结束 integration-parallel"
  return $status
}

run_integration_serial() {
  local args=()
  while IFS= read -r arg; do
    args+=("$arg")
  done < <(build_test_args "${TOMCAT_INTEGRATION_SERIAL_TESTS[@]}")

  if [ "${#args[@]}" -eq 0 ]; then
    log_phase "跳过 integration-serial：当前平台无可执行串行组"
    return 0
  fi

  log_phase "开始 integration-serial（必须串行 integration crate）"
  cargo test -j 1 --no-fail-fast "${args[@]}" -- --nocapture --test-threads=1
  local status=$?
  log_phase "结束 integration-serial"
  return $status
}

run_integration() {
  local fail=0
  run_integration_parallel || fail=1
  run_integration_serial || fail=1
  return $fail
}

run_integration_real_llm() {
  local openai_key_env
  openai_key_env="$(openai_responses_key_env)"
  local missing=()
  local env_name
  while IFS= read -r env_name; do
    if [ -n "$env_name" ]; then
      missing+=("$env_name")
    fi
  done < <(missing_required_envs_for_real_llm)
  if [ "${#missing[@]}" -ne 0 ]; then
    echo "跳过 integration-real-llm：未设置 ${missing[*]}（当前 OpenAI target=$(openai_responses_target)）" >&2
    return 0
  fi
  local args=()
  while IFS= read -r arg; do
    args+=("$arg")
  done < <(build_test_args "${TOMCAT_INTEGRATION_REAL_LLM_TESTS[@]}")
  log_phase "开始 integration-real-llm（真 LLM E2E；串行，需 ${openai_key_env} + DEEPSEEK_API_KEY + MIMO_API_KEY）"
  cargo test -j 1 --no-fail-fast "${args[@]}" -- --nocapture --test-threads=1
  local status=$?
  log_phase "结束 integration-real-llm"
  return $status
}

run_integration_openai_responses_wire() {
  if [ "${#TOMCAT_INTEGRATION_OPENAI_RESPONSES_WIRE_COMMANDS[@]}" -eq 0 ]; then
    log_phase "跳过 integration-openai-responses-wire：当前未配置命令"
    return 0
  fi
  local openai_key_env
  openai_key_env="$(openai_responses_key_env)"
  if [ -z "${!openai_key_env}" ]; then
    log_phase "跳过 integration-openai-responses-wire：当前 OpenAI target=$(openai_responses_target) 未设置 ${openai_key_env}"
    return 0
  fi

  log_phase "开始 integration-openai-responses-wire（仅 OpenAI Responses wire 真链路组）"
  local fail=0
  local cmd
  set +e
  for cmd in "${TOMCAT_INTEGRATION_OPENAI_RESPONSES_WIRE_COMMANDS[@]}"; do
    log_phase "执行: $cmd"
    eval "$cmd"
    local status=$?
    if [ $status -ne 0 ]; then
      fail=1
    fi
  done
  set -e
  log_phase "结束 integration-openai-responses-wire"
  return $fail
}

export RUST_LOG=tomcat=debug,info

CMD="${1:-all}"
case "$CMD" in
  release)
    run_release
    ;;
  clippy)
    run_clippy
    ;;
  lib)
    run_lib
    ;;
  integration)
    run_integration
    ;;
  integration-parallel)
    run_integration_parallel
    ;;
  integration-serial)
    run_integration_serial
    ;;
  integration-real-llm)
    run_integration_real_llm
    ;;
  integration-openai-responses-wire)
    run_integration_openai_responses_wire
    ;;
  all)
    set +e
    FAIL=0
    run_release || FAIL=1
    run_clippy || FAIL=1
    run_lib || FAIL=1
    run_integration || FAIL=1
    set -e
    if [ $FAIL -ne 0 ]; then
      echo "=== 存在失败的测试，请查看上方输出 ===" >&2
      exit 1
    fi
    echo "=== 全量测试通过 ==="
    ;;
  -h|--help|help)
    sed -n '2,18p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
    ;;
  *)
    echo "用法: $0 [release|clippy|lib|integration|integration-parallel|integration-serial|integration-real-llm|integration-openai-responses-wire|all|-h]" >&2
    echo "  默认与 all：release → clippy → lib → integration-parallel → integration-serial" >&2
    echo "  integration-real-llm 需当前 OpenAI target 对应 key + DEEPSEEK_API_KEY + MIMO_API_KEY；不进 all，须显式触发" >&2
    exit 2
    ;;
esac
