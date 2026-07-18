#!/usr/bin/env bash
# 集成测试：默认路径不依赖外部 Wasm 运行时。
# 测试执行按资源需求分类：默认 integration 门禁走 cargo-nextest 4 并发，显式 real-llm 层单独触发。
# 非 TTY 下强制 EDITOR/PAGER 为无交互，避免子进程阻塞；说明见 docs/reports/integration_test_hang_remediation.md。
#
# 用法（在项目根）：
#   ./scripts/run-integration-tests.sh                      # 默认快门禁：clippy → lib → doctest → integration
#   ./scripts/run-integration-tests.sh all                  # 同上（保留兼容别名）
#   ./scripts/run-integration-tests.sh release              # 仅 cargo build --release
#   ./scripts/run-integration-tests.sh clippy               # 仅 cargo clippy --all-targets -- -D warnings
#   ./scripts/run-integration-tests.sh lib                  # 仅库内单元测试（cargo test --lib）
#   ./scripts/run-integration-tests.sh doctest              # 仅文档测试（cargo test --doc）
#   ./scripts/run-integration-tests.sh integration          # 默认 integration 门禁（nextest 4 并发 + serial 兜底空组）
#   ./scripts/run-integration-tests.sh integration-parallel # 默认 integration 组（nextest 4 并发，含原串行组已放开的 binary）
#   ./scripts/run-integration-tests.sh integration-serial   # 仅 serial 兜底组（默认空）
#   ./scripts/run-integration-tests.sh integration-real-llm # 真 LLM 显式层（nextest real-llm profile，max-threads=2）
#   ./scripts/run-integration-tests.sh integration-openai-responses-wire # 只跑 OpenAI Responses wire 真链路组（需当前 OpenAI target 对应 key）
#   ./scripts/run-integration-tests.sh gate-fast            # 快门禁：clippy → lib → doctest → integration
#   ./scripts/run-integration-tests.sh gate-full            # 全门禁：gate-fast → integration-real-llm
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

NEXTTEST_INSTALL_HINT="cargo install cargo-nextest --locked"

log_phase() {
  echo "=== [$(date '+%Y-%m-%d %H:%M:%S')] $* ==="
}

ensure_nextest() {
  if cargo nextest --version >/dev/null 2>&1; then
    return 0
  fi
  cat >&2 <<EOF
cargo-nextest 未安装，无法执行当前门禁。

请先安装：
  $NEXTTEST_INSTALL_HINT

或使用 binstall：
  cargo binstall cargo-nextest
EOF
  return 127
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
  log_phase "开始 lib: cargo test --lib"
  cargo test --lib
  local status=$?
  log_phase "结束 lib"
  return $status
}

run_doctest() {
  log_phase "开始 doctest: cargo test --doc"
  cargo test --doc
  local status=$?
  log_phase "结束 doctest"
  return $status
}

run_integration_parallel() {
  local args=()
  while IFS= read -r arg; do
    args+=("$arg")
  done < <(build_test_args "${TOMCAT_INTEGRATION_PARALLEL_TESTS[@]}")

  ensure_nextest
  log_phase "开始 integration-parallel（默认 integration 门禁，nextest 4 并发）"
  cargo nextest run --no-fail-fast "${args[@]}"
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
    log_phase "跳过 integration-serial：serial 兜底组当前为空"
    return 0
  fi

  ensure_nextest
  log_phase "开始 integration-serial（nextest serial 兜底组）"
  cargo nextest run --no-fail-fast "${args[@]}"
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
  ensure_nextest
  log_phase "开始 integration-real-llm（真 LLM 显式层；nextest real-llm profile，max-threads=2；需 ${openai_key_env} + DEEPSEEK_API_KEY + MIMO_API_KEY）"
  cargo nextest run --profile real-llm --no-fail-fast
  local status=$?
  log_phase "结束 integration-real-llm"
  return $status
}

run_gate_fast() {
  local fail=0
  run_clippy || fail=1
  run_lib || fail=1
  run_doctest || fail=1
  run_integration || fail=1
  return $fail
}

run_gate_full() {
  local fail=0
  run_gate_fast || fail=1
  run_integration_real_llm || fail=1
  return $fail
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
  doctest)
    run_doctest
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
  gate-fast)
    run_gate_fast
    ;;
  gate-full)
    run_gate_full
    ;;
  all)
    set +e
    FAIL=0
    run_gate_fast || FAIL=1
    set -e
    if [ $FAIL -ne 0 ]; then
      echo "=== 存在失败的测试，请查看上方输出 ===" >&2
      exit 1
    fi
    echo "=== 默认快门禁通过 ==="
    ;;
  -h|--help|help)
    sed -n '2,23p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
    ;;
  *)
    echo "用法: $0 [release|clippy|lib|doctest|integration|integration-parallel|integration-serial|integration-real-llm|integration-openai-responses-wire|gate-fast|gate-full|all|-h]" >&2
    echo "  默认与 all：clippy → lib → doctest → integration" >&2
    echo "  gate-full：gate-fast → integration-real-llm" >&2
    echo "  integration-real-llm 需当前 OpenAI target 对应 key + DEEPSEEK_API_KEY + MIMO_API_KEY；不进默认门禁，须显式触发" >&2
    exit 2
    ;;
esac
