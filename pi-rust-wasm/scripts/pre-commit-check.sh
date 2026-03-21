#!/usr/bin/env bash
# 提交前检查：全量测试 + 覆盖率，并输出合规 commit message 模板（宪法 / commit-guard）
set -e
cd "$(dirname "$0")/.."

echo "=== 1. 全量测试（-j 1 与各目标内 --test-threads=1 串行）==="
cargo test -j 1 --all -- --test-threads=1
echo ""

echo "=== 2. 覆盖率（pi_wasm lib）==="
cargo tarpaulin --lib --packages pi_wasm --out stdout --no-fail-fast
echo ""
echo "=== 3. Commit message 模板 ==="
echo "请从上方 tarpaulin 输出中复制「pi_wasm」对应的覆盖率百分比，替换下方 [cov = xx.x%] 中的 xx.x 后使用。"
echo ""
cat << 'EOF'
feat(ext): 007/008 规范审查补漏：导出 invoke_host_func_with、文档与 Hostcall 集成测试

按宪法与 PLAN 补漏：ext/lib 导出 invoke_host_func_with；更新 docs/technical/02-wasm-runtime-and-plugin.md（WasmEdge/Node/内存边界）；新增 tests/hostcall_tests.rs；instance_wasmedge host_call_impl 注释。

[cov = xx.x%]
EOF
echo ""
