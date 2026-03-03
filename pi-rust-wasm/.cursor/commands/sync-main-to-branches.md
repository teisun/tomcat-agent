---
name: /sync-main-to-branches
id: sync-main-to-branches
category: Workflow
description: 将 main 分支同步到 develop 及各 feature 分支；执行后可选择 all 或指定分支
---

将 **main** 分支的变更同步到目标分支（develop 及所有 feature/*）。执行后由用户选择同步范围。

---

## 目标分支列表（动态获取）

**不要写死分支**。先执行 `git branch` 查看本地有哪些分支，排除 `main` 后列给用户选择。输出格式示例（实际以本地分支为准）：

| 编号 | 分支名 |
|------|--------|
| 1 | develop |
| 2 | feature/infra |
| 3 | feature/session-cli |
| 4 | feature/llm |
| 5 | feature/wasm-plugin |
| 6 | feature/primitives-tools |
| 7 | feature/chat |

---

## 执行步骤

1. **列出本地分支并询问用户**
   - 执行 `git branch`（或 `git branch --list 'develop' 'feature/*'` 若只关心 develop + feature/*）获取本地分支列表；排除当前分支标记 `*` 与 `main`，得到可同步目标列表。
   - 在对话中按**编号 + 分支名**列出表格，例如：
     ```
     | 编号 | 分支名 |
     |------|--------|
     | 1    | develop |
     | 2    | feature/infra |
     ...（按实际本地分支输出）
     ```
   - 使用 **AskUserQuestion** 询问用户：
     - 输入 **all**：将 main 同步到上一步列出的**全部分支**。
     - 或输入 **分支名**或 **编号**：仅同步到该分支。
   - 若输入无法解析（既不是 all 也不是有效分支名/编号），提示重新选择并再次询问。

2. **确认 main 已最新（可选）**
   - 建议先执行 `git fetch origin main`，必要时 `git checkout main && git pull`，确保本地 main 为最新后再同步到目标分支。

3. **对每个选中的目标分支执行同步**
   - 若用户选 **all**：依次对第 1 步列出的**全部分支**执行下面步骤。
   - 若用户选**单个分支**：仅对该分支执行。
   - 对每个目标分支：
     - `git checkout <目标分支>`（若本地不存在可先 `git fetch origin <目标分支>` 再 checkout）。
     - 合并前记录：`PREV=$(git rev-parse HEAD)`（用于后续列出本次同步的文件）。
     - `git merge main`（将 main 合并进当前分支；若有冲突则停止并提示用户解决）。
     - 合并后执行 `git diff --name-only $PREV HEAD`，将输出文件路径收集起来（供收尾时一并列出）。
     - 若用户需要推送到远端：`git push origin <目标分支>`（可先询问「是否推送」或由用户事先说明）。
   - 每完成一个分支，简短提示「已同步：<分支名>」。

4. **收尾**
   - 合并完成后执行 `git checkout main`，切回 main 分支。
   - 输出本次已同步的**分支列表**。
   - 输出本次同步涉及的**文件列表**（上述各分支 `git diff --name-only $PREV HEAD` 的结果合并去重后列出）。

---

## 注意事项

- 同步方式为 **merge**（main 合并进目标分支），不默认 rebase，避免改写已推送历史。
- 若某分支与 main 有冲突，在该分支上解决冲突后执行 `git add`、`git commit`，再继续后续分支或推送。
- 若目标分支在本地不存在，需先 `git fetch origin` 再 `git checkout -b <分支> origin/<分支>` 或由用户确认后再创建。
