---
name: /commit-with-status
id: commit-with-status
category: Workflow
description: 按宪法执行提交前全量检查、status 更新与合规提交（含未暂存/未跟踪检查，严禁漏提）
---

# 提交前全量检查与合规提交（宪法流程）

执行本 command 时，Agent 必须按以下顺序执行，**全部通过后再执行 git commit**。

---

## 1. 检查所有区域（严禁漏提）

提交前必须检查三类区域，**所有属于本次改动的文件必须全部 `git add` 并提交**：

- **已暂存 (staged)**：`git diff --cached --name-only`
- **未暂存 (unstaged)**：`git diff --name-only`
- **未跟踪 (untracked)**：`git status -u` 中的 Untracked files（排除无关路径如 `../openclaw/`、子仓库等）

**Agent 必须**：
1. 运行 `git status` 并列出上述三类变更。
2. 若存在未暂存或未跟踪的、属于本次功能/文档的变更，**禁止直接提交**；Agent **主动帮用户执行** `git add` 将全部相关文件加入暂存区（排除用户明确排除的路径），再执行提交。
3. 若用户明确仅提交部分文件，则仅对用户指定的文件执行 `git add`，并在 commit message 中说明范围。

---

## 2. 更新本分支的 status 文件（每次提交必须）

- **分支与 status 对应**（与 [Status 规范](../../openspec/specs/guides/STATUS_GUIDE.md) 一致）：
  - `feature/xxx` → 更新 `status/feature-xxx.md`
  - `develop` → 更新 `status/develop.md`
  - 其他分支名 → `status/<分支名/替换为->.md`
- **Agent 必须**：在 commit 前确认本次提交已修改或已包含对上述 status 文件的更新；若未更新，先根据本次改动更新 status 文件（含元数据表时间、DONE/INTERFACE/BLOCKED），再纳入本次提交。
- Status 格式细节见 [STATUS_GUIDE.md](../../openspec/specs/guides/STATUS_GUIDE.md)（仅用 H3、表格对齐、State 取值等）。

---

## 3. Commit Message 格式（宪法附录）

必须符合 [Commit Message 规范](../../openspec/specs/guides/COMMIT_MESSAGE_SPEC.md)：**首行写做了什么（what），详细描述写为什么这么做、作用与意义（why），禁止记流水账**。

- **类型**：`feat` | `fix` | `docs` | `style` | `refactor` | `test` | `chore`
- **豁免（可不写覆盖率）**：以下情况**不需要**填写 `[cov = xx.x%]`：
  - 本次**唯一**修改为 `status/*.md` 或 `INTEGRATION.md`；或
  - **仅修改文档、未改代码**：本次变更仅涉及文档（如 `docs/*.md`、`openspec/**`、`*.md`、guides 等），未修改 `src/`、`Cargo.toml`、测试代码等。  
  其他含代码的提交须跑测试并填写覆盖率（见 .cursor/rules/commit-guard.mdc）。

---

## 4. 执行顺序小结

1. `git status` → 列出 staged / unstaged / untracked。
2. **帮用户执行** `git add`：将属于本次改动的文件全部加入暂存区，严禁漏提。
3. 确认或补充更新 `status/feature-xx.md` 或 `status/develop.md`。
4. 若含代码变更：运行 `cargo test --all`、跑覆盖率并写入 commit message；**仅文档变更则不填覆盖率**。
5. 按附录格式书写 commit message，执行 `git commit`。
6. 提示：宪法要求「提交到本地与远端」，如需可执行 `git push`。
