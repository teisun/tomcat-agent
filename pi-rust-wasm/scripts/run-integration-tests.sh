#!/usr/bin/env bash
# 集成测试前检查 WasmEdge：未安装则自动执行 install-wasmedge.sh -y，再跑全量验收（含 wasmedge_e2e_tests）。
# 使用方式：在项目根执行 ./scripts/run-integration-tests.sh
set -e

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

# Windows 下跳过 Wasm 安装与 wasmedge 用例，仅跑不依赖 WasmEdge 的步骤
SKIP_WASMEDGE=0
if [ -n "$OS" ] && [ "$OS" = "Windows_NT" ]; then
  echo "Windows：跳过 WasmEdge 安装与 wasmedge_e2e_tests，仅执行其余验收步骤。Wasm 验收请按文档安装 WasmEdge 后手动执行。" >&2
  SKIP_WASMEDGE=1
fi

if [ $SKIP_WASMEDGE -eq 0 ]; then
  # 检查 WasmEdge 是否可用
  if ! command -v wasmedge >/dev/null 2>&1 && [ ! -x "$HOME/.wasmedge/bin/wasmedge" ]; then
    echo "未检测到 WasmEdge，正在执行 ./scripts/install-wasmedge.sh -y ..."
    ./scripts/install-wasmedge.sh -y
  fi
  # 使当前 shell 能加载 libwasmedge（cargo test --lib 等需要），已安装时也需 source
  if [ -f "$HOME/.wasmedge/env" ]; then
    set +e
    . "$HOME/.wasmedge/env"
    set -e
  fi
fi

export RUST_LOG=pi_wasm=debug,info
FAIL=0

echo "=== cargo build --release ==="
cargo build --release

# === 阶段 1：单元测试 ===
echo "=== [1/3] 单元测试 ==="
cargo test --lib -- --nocapture || FAIL=1

# === 阶段 2：集成测试（API 级） ===
echo "=== [2/3] 集成测试（API 级） ==="
cargo test --no-fail-fast \
  --test event_tests --test hostcall_tests --test llm_tests \
  --test plugin_tests --test primitives_tools_tests \
  --test robustness_tests --test session_tests \
  -- --nocapture || FAIL=1

# === 阶段 3：E2E 测试（pi CLI + Wasm 运行时） ===
echo "=== [3/3] E2E 测试（用户操作模拟） ==="
cargo test --no-fail-fast --test cli_tests -- --nocapture || FAIL=1
if [ $SKIP_WASMEDGE -eq 0 ]; then
  echo "=== cargo build（含 WasmEdge）==="
  cargo build
  cargo test --no-fail-fast --test wasmedge_e2e_tests -- --nocapture || FAIL=1
else
  echo "跳过 wasmedge 构建与测试（Windows）。"
fi

if [ $FAIL -ne 0 ]; then
  echo "=== 存在失败的测试，请查看上方输出 ==="
  exit 1
fi
echo "=== 全量集成测试通过 ==="
