# 项目宪法（Constitution）

本文档为不可违反的核心规则，与本文档冲突的内容、代码、操作将被拒绝。修改须经完整评审。

---

## 一、安全红线（无例外）

1. **禁止静默执行高危操作**：write/edit/bash、插件安装、文件修改、代码编译等，必须明确告知用户并获二次确认，禁止静默执行。
2. **错误完全隔离**：插件、钩子、技能、Agent 任务的执行错误须完整捕获，禁止导致主程序崩溃。
3. **4 原语审计完整**：所有 read/write/edit/bash 须留存完整审计日志（时间、内容、用户确认状态、结果），可追溯。
4. **禁止恶意代码**：禁止生成、编译、执行病毒/木马/勒索/挖矿/窃取隐私等恶意代码，任何场景无例外。

（架构层面的最小权限、宿主隔离、分层与 4 原语地位见 **Architecture.md**。）

**TODO**：敏感数据加密（API 密钥、配置等）后续再考虑。

---

## 二、Agent 协作规范

1. **宪法优先**：Agent 行为、生成代码、执行操作须符合本宪法；与宪法冲突的需求须拒绝并说明原因。
2. **自测覆盖**: Agent开发的所有功能必须附带单元测试代码，代码覆盖率不得低于85%
3. **测试不通过则查因改码**：单元测试、集成测试、E2E 不通过时须查原因、修改代码或测试，直至真实通过；**不得**通过跳过、忽略、降低断言、滥用 `#[ignore]` 或仅为通过 CI/门禁而弱化断言来「假绿」。
4. **用户知情权**：高危操作前须清晰告知操作内容、风险与影响，获用户明确确认后执行，禁止隐瞒。
5. **质量与可追溯**：交付代码须可编译、可运行，通过门禁与测试；操作与变更须有日志或记录，可追溯。
6. **开发流程规则**（Agent 必须遵守）：
   - **开发前**：
        - 检查工作区状态，若处于detached HEAD则自动checkout -b 自己的工作分支
        - 同步 develop 分支，了解全局状态后再开新改动。
        - 阅读编码规范家族（须一并遵守）：
            - [架构与编码总纲](./guides/coding/Codeing&Architecture_Spec.md)
            - [Rust 文件行数规范](./guides/coding/RUST_FILE_LINES_SPEC.md)
            - [Rust 惯用写法与 Clippy 规则速查](./guides/coding/RUST_IDIOMS_SPEC.md)
            - [代码注释规范](./guides/coding/COMMENT_SPEC.md)
            - [单元测试文件组织规范](./guides/testing/UNIT_TEST_LAYOUT_SPEC.md)（业务与测试强制分离、目录与挂载的单一权威来源）
            - [单元测试编写规范](./guides/testing/UNIT_TEST_SPEC.md)
   - **开发流程**：
        - 根据编码规范编码开发(带注释) → 测试 → 修 bug → 单测通过 -> 写技术[文档](../../docs/)
   - **提交前**：更新本分支的 `docs/status/feature-xx.md`（与当前分支对应），再提交。提交时代码变更所需的 `[cov = xx.x%]` 从该 status 文件的 Cov% 读取，不在提交时现跑测试/覆盖率。
   - **提交策略**：每个任务完成提交一次，禁止囤积多个任务一次性提交；提交到本地与远端。
7. **阻塞主动上报**：多 Agent 协作时，遇依赖阻塞、技术问题、需求不明确，**必须在本分支的 `docs/status/<当前分支对应文件名>.md` 中更新状态**（含阻塞原因与预计解决时间），禁止静默阻塞；不直接修改 docs/INTEGRATION.md。
8. **提交规约**  
    - 检查所有区域：提交前不仅要看“已暂存”，还要检查“未暂存”和“未跟踪”区域。
    - 改动不打折：凡是属于本次功能的改动（包括新加的文件），必须全部 git add 并提交，严禁漏提。
    - 每次提交必须更新**本分支**的 `docs/status/feature-xx.md`
    - 遵循约定的 commit message 格式(见附录)，**讲清楚做了什么和为什么这么做(what+why)**，禁止记流水账或无意义提交。  
    - 具体格式见 [Status规范](./guides/workflow/STATUS_GUIDE.md)
9. **技术设计参考**：技术设计或代码实现有疑问时，可参考 **Architecture.md** 中的「**pi-mono** 生态参考原则（双仓对照）」：以 **pi-mono** 为兼容性契约与行为基准，以 **pi-agent-rust** 为 Rust 侧实现参考；二者不一致时以 pi-mono 为准。
10. **「说人话」辅助（先专业、后口语）**：撰写计划、问题描述、技术方案与文档时，**须先把**定义、契约、步骤、边界等技术内容写清楚、写准确；**再**用口语化文字帮助读者扫读——（1）主要技术小节或图、表整块之后，可跟短段 **「说人话」**（或小标题 **`说人话`** / **阅读顺序（说人话）**）；（2）信息密度高的 Markdown 对照表，宜在末列加 **`说人话`**（一行一句）。**禁止**仅用口语替代必须的技术表述。细则见 [Architecture Spec §14.1](./guides/workflow/ARCHITECTURE_SPEC.md) 与 [PLAN_SPEC.md](../agents/plan/PLAN_SPEC.md) 文首。

---

## 三、完成定义（Definition of Done） 

功能/迭代/任务视为完成须同时满足：

1. 符合本宪法及 Architecture.md 约束。
2. 通过规范门禁（rustfmt/clippy 等）与约定测试， 无静默高危操作、无越权。
3. 代码 review 通过，确保符合编码规范家族（[架构与编码总纲](./guides/coding/Codeing&Architecture_Spec.md) + [Rust 文件行数规范](./guides/coding/RUST_FILE_LINES_SPEC.md) + [Rust 惯用写法与 Clippy 规则速查](./guides/coding/RUST_IDIOMS_SPEC.md) + [代码注释规范](./guides/coding/COMMENT_SPEC.md)）、功能完整、注释完整、无设计缺陷、无需求遗漏
4. 单元测试通过；覆盖率测量为**可选项**，若需测量可执行 `/update-coverage` Command 或 `cargo tarpaulin --lib --package tomcat`，将结果填入 status 元数据表 Cov% 列（格式见 [Status规范](./guides/workflow/STATUS_GUIDE.md)），参考 [单元测试编写规范](./guides/testing/UNIT_TEST_SPEC.md) 与 [单元测试文件组织规范](./guides/testing/UNIT_TEST_LAYOUT_SPEC.md)
5. 集成测试通过，参考[集成测试规范](./guides/testing/INTEGRATION_TEST_SPEC.md)
6. E2E测试通过
7. 文档更新：配套说明或文档到位。
    - [技术文档规范](./guides/workflow/DOCUMENTATION_GUIDE.md)
    - [代码注释规范](./guides/coding/COMMENT_SPEC.md)
    - [Status规范](./guides/workflow/STATUS_GUIDE.md)
    

---

## 附录：提交格式

### A. Commit Message 格式

每次 Git 提交的 commit message 须遵循以下格式，禁止无意义提交：**首行写清楚做了什么（what），详细描述写为什么这么做、作用与意义（why）**。具体格式与更多示例见 [Commit Message 规范](./guides/workflow/COMMIT_MESSAGE_SPEC.md)。

