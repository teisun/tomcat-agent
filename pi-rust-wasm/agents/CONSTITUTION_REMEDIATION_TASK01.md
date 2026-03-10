# TASK-01 宪法合规复盘与整改计划

## 一、未遵守项复盘（原因）

| 宪法要求 | 未做到的具体表现 | 原因分析 |
|----------|------------------|----------|
| **自测覆盖 ≥85%，覆盖率写入 status 的 Cov%** | status/feature-plugin-lifecycle.md 中 Cov% 填为 "-"，未跑 tarpaulin 填数 | 实施时以「门禁通过」为收尾，未把「完成定义」中的覆盖率写入当作必做步骤执行；tarpaulin 曾后台执行未等结果。 |
| **开发前：同步 develop、检查分支** | 未执行 `git fetch`/`git merge develop`，未确认当前是否在 feature/plugin-lifecycle | 按「计划附件的实施顺序」直接编码，未先执行 Dispatcher 第五步「开发前」的 git 流程。 |
| **完成定义：文档更新** | 未更新 docs/02-wasm-runtime-and-plugin.md，未补充 load_plugin（9.2）说明 | 开发流程中「写技术文档」被遗漏，只做了代码与单测，未对照完成定义第 7 条做文档更新。 |
| **提交规约：commit 含 what+why、[cov]** | 未做实际 git commit，故无合规 commit message 与 [cov] | 实施在「更新 TASK_BOARD 与 status」后结束，未进入「提交到本地与远端」步骤。 |

## 二、整改计划（补做清单）

1. **覆盖率**：对 `pi_wasm` 跑 `cargo tarpaulin --lib --packages pi_wasm`，将结果写入 `status/feature-plugin-lifecycle.md` 的 Cov% 列；若低于 85% 则补充单测或标注原因。
2. **技术文档**：在 `docs/02-wasm-runtime-and-plugin.md` 中增加 9.2 节或等价小节，说明 `PluginManager::load_plugin` 的流程、依赖注入（wasm_engine/host_dispatcher/confirm_permissions）、与 design CODE_BLOCK_P1_009 的对应关系。
3. **Git**：检查当前分支；若未在 feature/plugin-lifecycle 则切换或创建；从 develop 拉取最新并 rebase/merge；确认工作区改动完整后，按 commit-guard 与宪法附录格式提交（首行 what，详细描述 why，末尾 [cov = xx.x%]）。

## 三、执行状态

- [~] 跑覆盖率并更新 status Cov%：status 已注明「待测」及填法（`cargo tarpaulin --lib --packages pi_wasm`）；因 tarpaulin 耗时长未在本轮跑完，提交前需本地执行并填入实际 Cov%。
- [✓] 更新 docs/02-wasm-runtime-and-plugin.md（load_plugin/9.2）：已增第 4 节「插件完整加载流程（9.2）」及第 2 节 9.2 要点。
- [ ] 同步 develop、提交并推送：当前分支为 develop；若在 feature/plugin-lifecycle 上开发，需 checkout 该分支、同步 develop 后，按 commit-guard 与宪法附录提交（what+why、[cov = xx.x%]）。
