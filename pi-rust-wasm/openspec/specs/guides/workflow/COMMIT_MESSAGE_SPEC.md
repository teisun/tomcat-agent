# Commit Message 规范

与 [Constitution.md](../../Constitution.md) 附录「提交格式」一致；本条规范细化格式要求与 **what+why** 示例。

---

## 一、格式要求

每次 Git 提交的 commit message 须遵循以下格式，禁止无意义提交：

```
# 类型(模块): 简短描述(what) (不超过50字)
# feat: 新功能
# fix: 修复bug
# docs: 仅文档
# style: 格式(不影响代码运行)
# refactor: 重构(既不是新功能也不是改bug)
# test: 增加测试
# chore: 构建/工具/辅助变动

# 详细描述（why）

# 测试覆盖率
[cov = xx.x%]
```

- **首行**：类型(模块) + 简短描述，说清**做了什么（what）**，不超过 50 字。
- **详细描述**：说明**为什么这么做（why）**——动机、要解决的问题、改动的作用与意义；禁止记流水账或罗列文件名。
- **覆盖率**：常规代码提交须带 `[cov = xx.x%]`，数值从当前分支 status 文件中的 Cov% 读取（由宪法流程在研发时写入）；读不到时可不写 [cov]，不阻塞提交。

---

## 二、示例（体现 what + why）

**示例 1：功能类**

```
feat(ext,infra): 将 wasmedge_quickjs 路径纳入配置

QuickJS wasm 路径原先仅支持环境变量，多环境与 CI 下不便统一管理。纳入 config 后可由配置文件提供默认路径，环境变量 PI_WASM__WASM__QUICKJS_PATH 覆盖，与现有 llm/storage 等配置优先级一致，便于部署与复现。

[cov = 80.7%]
```

- **What**：把 wasmedge_quickjs 路径放进配置（config + env）。
- **Why**：解决仅靠环境变量带来的管理不便；统一配置语义、便于部署与复现。

**示例 2：修复类**

```
fix(session): 单测中 session 目录使用 canonicalize 避免并行竞态

run_session 单测依赖 PI_WASM__STORAGE__SESSIONS_DIR，并行时多用例写同一路径导致偶发失败。改为每用例 tempdir 并 canonicalize 后 set_var，保证进程内路径唯一，消除竞态。

[cov = 82.1%]
```

- **What**：单测里对 session 目录做 canonicalize 并每用例独立 tempdir。
- **Why**：消除并行测试下路径竞争导致的偶发失败，保证稳定通过。

**示例 3：仅文档/status 的提交（豁免时可不写 [cov]）**

```
chore(doc): 更新 status/feature-wasm-plugin.md 宪法流程走查项

记录本次分支已完成宪法流程验证与覆盖率达标，便于后续评审与合并前核对。

（豁免时不写 [cov = xx.x%]）
```

---

## 三、禁止行为

- 禁止无格式提交、标题过长（>50 字）。
- 禁止详细描述记流水账（如只列「修改了 A、B、C 文件」而不写原因与作用）。
- 禁止手动编造覆盖率；有 Cov% 时 commit 中 `[cov = xx.x%]` 数值须与当前分支 status 文件中 Cov% 一致。
