#!/usr/bin/env bash
# 汇总各分支 status 碎片，覆盖生成 docs/INTEGRATION.md。建议在 develop 分支上执行。
set -e

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$REPO_ROOT"

GIT_ROOT=$(git rev-parse --show-toplevel 2>/dev/null || true)
if [ -z "$GIT_ROOT" ]; then
  echo "错误：当前目录不在 Git 仓库内。" >&2
  exit 1
fi

# status 路径前缀：若仓库根与脚本所在项目根不同（如 monorepo），则带相对前缀
if [ "$GIT_ROOT" != "$REPO_ROOT" ]; then
  STATUS_PREFIX="${REPO_ROOT#$GIT_ROOT/}/docs/status"
else
  STATUS_PREFIX="docs/status"
fi

CURRENT_BRANCH=$(git rev-parse --abbrev-ref HEAD 2>/dev/null || true)
if [ "$CURRENT_BRANCH" != "develop" ]; then
  echo "警告：当前分支为 $CURRENT_BRANCH，建议在 develop 上执行本脚本。" >&2
fi

# 动态获取所有 feature 分支（排序保证顺序稳定）
BRANCHES=($(git branch --format='%(refname:short)' | grep -E '^feature/' | sort))

OUTPUT="# 项目集成与进度看板

以下由 develop 与各 feature 分支的 status 碎片自动汇总，执行 \`/aggregate-status\` 更新。

"

# 先汇总 develop 的 docs/status/develop.md（从当前分支读取，建议在 develop 上执行）
DEVELOP_PATH="$STATUS_PREFIX/develop.md"
develop_content=""
if [ -f "$REPO_ROOT/docs/status/develop.md" ]; then
  develop_content=$(cat "$REPO_ROOT/docs/status/develop.md" 2>/dev/null || true)
fi
OUTPUT+="
## develop
"
if [ -z "$develop_content" ]; then
  OUTPUT+="
*暂无进度*
"
else
  OUTPUT+="
$develop_content
"
  PARTICIPATED="  - develop ($DEVELOP_PATH)
"
fi
OUTPUT+="
---
"

PARTICIPATED="${PARTICIPATED:-}"

for branch in "${BRANCHES[@]}"; do
  # 分支名中 / 转为 -，得到 status 文件名，如 feature/infra -> feature-infra.md
  section_name="${branch//\//-}"
  path="$STATUS_PREFIX/$section_name.md"
  content=""
  if git rev-parse --verify "$branch" &>/dev/null; then
    content=$(git show "$branch:$path" 2>/dev/null || true)
  fi
  # 每个分支始终带 H2 标题，再拼内容或占位；块间用 --- 分隔
  OUTPUT+="
## $section_name
"
  if [ -z "$content" ]; then
    OUTPUT+="
*暂无进度*
"
  else
    OUTPUT+="
$content
"
    PARTICIPATED+="  - $branch ($path)
"
  fi
  OUTPUT+="
---
"
done

mkdir -p docs
printf '%s' "$OUTPUT" > docs/INTEGRATION.md

echo "已根据 status 碎片更新 docs/INTEGRATION.md。"
if [ -n "$PARTICIPATED" ]; then
  echo "参与汇总的分支/文件："
  echo "$PARTICIPATED"
else
  echo "（暂无任何分支提供 status 碎片）"
fi
