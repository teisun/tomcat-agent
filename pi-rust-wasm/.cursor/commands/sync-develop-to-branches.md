---
name: /sync-develop-to-branches
id: sync-develop-to-branches
category: Workflow
description: 将 develop 分支同步到各 feature 及其他分支；执行后可选择 all 或指定分支
---

将 **develop** 分支的变更同步到目标分支（各 feature/* 及其他非 main 分支）。执行后由用户选择同步范围。develop 作为同步源，不作为同步目标。

---

## 目标分支列表（动态获取）

**不要写死分支**。先执行 `git branch` 查看本地有哪些分支，**排除 `main` 与 `develop`** 后列给用户选择。输出格式示例（实际以本地分支为准）：

| 编号 | 分支名 |
|------|--------|
| 1 | feature/infra |
| 2 | feature/session-cli |
| 3 | feature/llm |
| 4 | feature/wasm-plugin |
| 5 | feature/primitives-tools |
| 6 | feature/chat |

---

## 执行步骤

1. **列出本地分支并询问用户**
   - 执行 `git branch --format='%(refname:short)'` 获取本地分支列表；**排除 `main` 与 `develop`**，得到可同步目标列表。
   - 在对话中按**编号 + 分支名**列出表格。
   - 使用 **AskUserQuestion** 询问用户：
     - 输入 **all**：将 develop 同步到上一步列出的**全部分支**。
     - 或输入 **分支名**或 **编号**（可多个，如 `1 3` 或 `feature/infra feature/llm`）：仅同步到所选分支。
   - 若输入无法解析，提示重新选择并再次询问。

2. **确认 develop 已最新（可选）**
   - 建议先执行 `git fetch origin develop`，必要时 `git checkout develop && git pull`，确保本地 develop 为最新后再同步到目标分支。

3. **对每个选中的目标分支执行同步**
   - 若用户选 **all**：依次对第 1 步列出的**全部分支**执行下面步骤。
   - 若用户选**单个或多个分支**：仅对所选分支执行。
   - 对每个目标分支：
     - `git checkout <目标分支>`（若本地不存在可先 `git fetch origin <目标分支>` 再 checkout）。
     - 合并前记录：`PREV=$(git rev-parse HEAD)`（用于后续列出本次同步的文件）。
     - `git merge develop`（将 develop 合并进当前分支；若有冲突则停止并提示用户解决）。
     - 合并后执行 `git diff --name-only $PREV HEAD`，将输出文件路径收集起来（供收尾时一并列出）。
     - 若用户需要推送到远端：`git push origin <目标分支>`（可先询问「是否推送」或由用户事先说明）。
   - 每完成一个分支，简短提示「已同步：<分支名>」。

4. **收尾**
   - 合并完成后执行 `git checkout develop`，切回 develop 分支。
   - 输出本次已同步的**分支列表**。
   - 输出本次同步涉及的**文件列表**（上述各分支 `git diff --name-only $PREV HEAD` 的结果合并去重后列出）。

---

## 注意事项

- 同步方式为 **merge**（develop 合并进目标分支），不默认 rebase，避免改写已推送历史。
- 若某分支与 develop 有冲突，在该分支上解决冲突后执行 `git add`、`git commit`，再继续后续分支或推送。
- 若目标分支在本地不存在，需先 `git fetch origin` 再 `git checkout -b <分支> origin/<分支>` 或由用户确认后再创建。
