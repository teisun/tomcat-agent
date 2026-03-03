#!/usr/bin/env bash
# 汇总各分支 status 碎片，覆盖生成 INTEGRATION.md。建议在 develop 分支上执行。
set -e

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || true)
if [ "$CURRENT_BRANCH" != "develop" ]; then
  echo "警告：当前分支为 $CURRENT_BRANCH，建议在 develop 上执行本脚本。" >&2
fi

# 分支与 status 文件名对应（与 status/README.md 一致）
declare -a BRANCHES=( "feature/infra:status/feature-infra.md"
                      "feature/session-cli:status/feature-session-cli.md"
                      "feature/llm:status/feature-llm.md"
                      "feature/wasm-plugin:status/feature-wasm-plugin.md"
                      "feature/primitives-tools:status/feature-primitives-tools.md"
                      "feature/chat:status/feature-chat.md" )

OUTPUT="# 项目集成与进度看板
"
PARTICIPATED=""

for entry in "${BRANCHES[@]}"; do
  branch="${entry%%:*}"
  path="${entry##*:}"
  section_name="${path#status/}"
  section_name="${section_name%.md}"
  content=""
  if git rev-parse --verify "$branch" &>/dev/null; then
    content=$(git show "$branch:$path" 2>/dev/null || true)
  fi
  if [ -z "$content" ]; then
    OUTPUT+="
## $section_name

（暂无进度碎片）
"
  else
    OUTPUT+="
$content
"
    PARTICIPATED+="  - $branch ($path)
"
  fi
done

printf '%s' "$OUTPUT" > INTEGRATION.md

echo "已根据 status 碎片更新 INTEGRATION.md。"
if [ -n "$PARTICIPATED" ]; then
  echo "参与汇总的分支/文件："
  echo "$PARTICIPATED"
else
  echo "（暂无任何分支提供 status 碎片）"
fi
