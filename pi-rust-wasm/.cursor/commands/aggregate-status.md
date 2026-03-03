---
name: /aggregate-status
id: aggregate-status
category: Workflow
description: 汇总各分支 status 碎片，覆盖生成 INTEGRATION.md（在看板分支 develop 上执行）
---

汇总各功能分支的 `status/feature-xx.md` 进度碎片，**整份覆盖**生成根目录 `INTEGRATION.md`。避免多 Agent 争改同一文件导致的冲突。

**建议在 develop 分支上执行**。执行前可先 `git fetch` 以包含远端分支最新提交。

---

## 方式一：一键执行脚本（推荐）

在项目根目录（pi-rust-wasm）下执行：

```bash
./scripts/aggregate-status.sh
```

脚本会：检查当前分支、遍历已知分支用 `git show <branch>:status/feature-xx.md` 读取内容、按顺序拼接并覆盖写入 `INTEGRATION.md`。完成后列出参与汇总的分支。

---

## 方式二：按步骤手动/由 Agent 执行

1. **分支检查**  
   若当前分支不是 `develop`，提示「建议在 develop 上执行」；仍可继续。

2. **收集碎片（动态遍历 feature 分支）**  
   - 执行 `git branch --format='%(refname:short)' | grep -E '^feature/' | sort` 得到所有 **feature/** 分支列表（排序保证顺序稳定）。
   - 对每个分支：将分支名中的 `/` 替换为 `-` 得到 status 文件名（如 `feature/infra` → `feature-infra.md`）；若仓库根与当前项目根不同（如 monorepo），则路径带相对前缀（如 `pi-rust-wasm/status/feature-infra.md`）。
   - 对每个分支执行 `git show <分支>:<路径>` 读取该分支上的 status 文件内容；若某分支无该文件或读取失败，该节内容记为「（暂无进度碎片）」。

3. **生成 INTEGRATION.md（覆盖逻辑）**  
   - 固定头部：`# 项目集成与进度看板`
   - 按上述顺序拼接各分支对应内容块（无内容则输出 `## feature-xx` + 占位）。
   - 将完整内容**覆盖写入**项目根目录 `INTEGRATION.md`。

4. **输出**  
   提示「已根据 status 碎片更新 INTEGRATION.md」，并列出参与了汇总的分支/文件。

---

**说明**：汇总为覆盖逻辑，不基于当前磁盘上的 INTEGRATION.md 做合并；INTEGRATION.md 始终等于当前各分支 status 碎片的汇总结果。
