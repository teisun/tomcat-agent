#!/usr/bin/env bash
# 集成测试：WasmEdge 检测（非 Windows 可自动 install-wasmedge.sh -y）、source ~/.wasmedge/env。
# 使用 cargo test --test '*' 已含 cli_tests 与 wasmedge_e2e_tests，不再单独重复跑。
# 全量测试固定为串行：`-j 1` 串行各测试二进制，`--test-threads=1` 串行同二进制内用例（降低 Wasm/Tokio 并发死锁风险）。
# 非 TTY 下强制 EDITOR/PAGER 为无交互，避免子进程阻塞；说明见 docs/reports/integration_test_hang_remediation.md。
#
# 用法（在项目根）：
#   ./scripts/run-integration-tests.sh              # 全量：release → clippy → lib → integration
#   ./scripts/run-integration-tests.sh all          # 同上
#   ./scripts/run-integration-tests.sh release      # 仅 cargo build --release
#   ./scripts/run-integration-tests.sh clippy       # 仅 cargo clippy --all-targets -- -D warnings
#   ./scripts/run-integration-tests.sh lib        # 仅单元测试
#   ./scripts/run-integration-tests.sh integration  # 仅 tests/ 下全部 integration crate
#
# 未知子命令：打印用法并 exit 2。
set -e

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

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

SKIP_WASMEDGE=0
if [ -n "$OS" ] && [ "$OS" = "Windows_NT" ]; then
  echo "Windows：跳过 WasmEdge 安装与 wasmedge_e2e_tests，仅执行其余验收步骤。Wasm 验收请按文档安装 WasmEdge 后手动执行。" >&2
  SKIP_WASMEDGE=1
fi

if [ $SKIP_WASMEDGE -eq 0 ]; then
  if ! command -v wasmedge >/dev/null 2>&1 && [ ! -x "$HOME/.wasmedge/bin/wasmedge" ]; then
    echo "未检测到 WasmEdge，正在执行 ./scripts/install-wasmedge.sh -y ..."
    ./scripts/install-wasmedge.sh -y
  fi
  if [ -f "$HOME/.wasmedge/env" ]; then
    set +e
    . "$HOME/.wasmedge/env"
    set -e
  fi
fi

export RUST_LOG=pi_wasm=debug,info

CMD="${1:-all}"
case "$CMD" in
  release)
    log_phase "开始 release: cargo build --release"
    cargo build --release
    log_phase "结束 release"
    ;;
  clippy)
    log_phase "开始 clippy: cargo clippy --all-targets -- -D warnings"
    cargo clippy --all-targets -- -D warnings
    log_phase "结束 clippy"
    ;;
  lib)
    log_phase "开始 lib: cargo test --lib（-j 1，--test-threads=1）"
    cargo test -j 1 --lib -- --nocapture --test-threads=1
    log_phase "结束 lib"
    ;;
  integration)
    log_phase "开始 integration（tests/ 下全部 integration test crate）"
    if [ $SKIP_WASMEDGE -eq 1 ]; then
      INTEGRATION_TEST_ARGS=()
      for f in tests/*_tests.rs; do
        [ -f "$f" ] || continue
        base=$(basename "$f" .rs)
        if [ "$base" = "wasmedge_e2e_tests" ]; then
          continue
        fi
        INTEGRATION_TEST_ARGS+=(--test "$base")
      done
      cargo test -j 1 --no-fail-fast "${INTEGRATION_TEST_ARGS[@]}" -- --nocapture --test-threads=1
    else
      cargo test -j 1 --no-fail-fast --test '*' -- --nocapture --test-threads=1
    fi
    log_phase "结束 integration"
    ;;
  all)
    set +e
    FAIL=0
    log_phase "开始 release: cargo build --release"
    cargo build --release || FAIL=1
    log_phase "结束 release"
    log_phase "开始 clippy: cargo clippy --all-targets -- -D warnings"
    cargo clippy --all-targets -- -D warnings || FAIL=1
    log_phase "结束 clippy"
    log_phase "开始 lib: cargo test --lib（-j 1，--test-threads=1）"
    cargo test -j 1 --lib -- --nocapture --test-threads=1 || FAIL=1
    log_phase "结束 lib"
    log_phase "开始 integration（tests/ 下全部 integration test crate）"
    if [ $SKIP_WASMEDGE -eq 1 ]; then
      INTEGRATION_TEST_ARGS=()
      for f in tests/*_tests.rs; do
        [ -f "$f" ] || continue
        base=$(basename "$f" .rs)
        if [ "$base" = "wasmedge_e2e_tests" ]; then
          continue
        fi
        INTEGRATION_TEST_ARGS+=(--test "$base")
      done
      cargo test -j 1 --no-fail-fast "${INTEGRATION_TEST_ARGS[@]}" -- --nocapture --test-threads=1 || FAIL=1
    else
      cargo test -j 1 --no-fail-fast --test '*' -- --nocapture --test-threads=1 || FAIL=1
    fi
    log_phase "结束 integration"
    set -e
    if [ $FAIL -ne 0 ]; then
      echo "=== 存在失败的测试，请查看上方输出 ===" >&2
      exit 1
    fi
    echo "=== 全量集成测试通过 ==="
    ;;
  -h|--help|help)
    sed -n '2,14p' "$0" | sed 's/^# \{0,1\}//'
    exit 0
    ;;
  *)
    echo "用法: $0 [release|clippy|lib|integration|all|-h]" >&2
    echo "  默认与 all：release → clippy → lib → integration（含 cli + wasmedge_e2e）" >&2
    exit 2
    ;;
esac
