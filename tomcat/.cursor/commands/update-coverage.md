---
name: /update-coverage
id: update-coverage
category: Workflow
description: 执行 cargo tarpaulin 测量覆盖率，并自动更新当前分支 status 文件的 Cov% 列
---

# 覆盖率测量与 status 自动更新

按需执行本 command 以测量当前代码覆盖率，并将结果同步到当前分支对应的 status 文件。  
**此步骤完全可选**，不在日常开发流程中强制触发。

---

## 1. 确认当前分支与 status 文件路径

执行 `git branch --show-current` 获取当前分支名，推导对应 status 文件路径规则（与 [commit-with-status](./commit-with-status.md) 一致）：

| 分支 | status 文件 |
|------|------------|
| `feature/xxx` | `docs/status/feature-xxx.md` |
| `develop` | `docs/status/develop.md` |
| 其他 | `docs/status/<分支名中 / 替换为 ->.md` |

若 status 文件不存在，提示用户先创建，**不继续执行 tarpaulin**。

---

## 2. 执行 tarpaulin

在项目根目录（`tomcat/`）执行：

```bash
cargo tarpaulin --lib --package tomcat
```

- 捕获完整输出。
- 从输出中提取覆盖率数值，格式为 `XX.XX% coverage, YYYY/ZZZZ lines covered`。
- 若命令执行失败（非零退出码），输出错误信息并终止，**不更新 status 文件**。

---

## 3. 更新 status 文件的 Cov% 列

定位当前分支 status 文件中**第一个元数据表**（即文件最顶部的 markdown 表格，含 Owner / Update Time / State / Branch / Cov% 等列）：

- 将该行的 `Cov%` 列值更新为解析到的覆盖率数值（如 `85.2`）。
- 同时将 `Update Time` 更新为当前时间（精确到分钟，格式 `YYYY-MM-DD HH:MM`）。
- 其他列（Owner、State、Branch）**保持不变**。

若元数据表中没有 `Cov%` 列，在表头末尾追加该列后再填写数值。

---

## 4. 输出结果

完成后输出：

```
覆盖率已更新：XX.XX%
status 文件已同步：docs/status/feature-xxx.md
可执行 /commit-with-status 将此更新纳入提交。
```

---

## 注意事项

- tarpaulin 运行时间较长（通常 2–5 分钟），请在有需要时再执行。
- 仅更新 status 文件，**不自动执行 git add 或 git commit**；提交请使用 `/commit-with-status`。
- 若当前在 `develop` 分支上但 status 文件无 Cov% 列，可按上述规则追加后填写。
