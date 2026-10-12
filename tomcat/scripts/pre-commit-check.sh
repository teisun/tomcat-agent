#!/usr/bin/env bash
# 提交前检查：同一离线门禁 + 库覆盖率；联网测试须单独显式运行。
set -e
cd "$(dirname "$0")/.."

echo "=== 1. 离线门禁（与 scripts/test-groups.sh / nextest 分类一致）==="
./scripts/run-integration-tests.sh gate-fast
echo ""

echo "=== 2. 覆盖率（tomcat lib）==="
cargo tarpaulin --lib --packages tomcat --out stdout --no-fail-fast
echo ""
echo "=== 3. Commit message 模板 ==="
echo "请从上方 tarpaulin 输出中复制「tomcat」对应的覆盖率百分比，替换下方 [cov = xx.x%] 中的 xx.x 后使用。"
echo ""
cat << 'EOF'
feat(ext): 007/008 规范审查补漏：导出 invoke_host_func_with、文档与 Hostcall 集成测试

按宪法与 PLAN 补漏：ext/lib 导出 invoke_host_func_with；更新 src/ext/README.md（WasmEdge/Node/内存边界）；新增 tests/hostcall_tests.rs；instance_wasmedge host_call_impl 注释。

[cov = xx.x%]
EOF
echo ""
