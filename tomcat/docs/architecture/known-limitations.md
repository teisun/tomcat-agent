# 已知限制与后续决策

本文只记录已确认、但不应在“会话卡死修复”批次中顺带改动的问题。每条都说明当前行为、风险边界和重新排期条件，避免它们在交接时丢失。

## P1

### 非 Unix 平台的阻塞 `read_line`

CLI 的交互输入在 Unix 以外的平台仍可能无法被取消信号及时打断。它不影响已落盘的会话恢复语义，但关闭 CLI 时可能比预期晚返回。

重新排期条件：支持 Windows 作为一等交付目标，或出现无法退出的实际报告。修复应选择跨平台 async stdin，而不是增加平台特判超时。

### 旧 `timeout_ms` 配置缺少运行时迁移覆盖

旧的 `ask_question.timeout_ms` 配置会产生迁移提示，但真实运行时组合尚没有端到端测试。当前产品语义仍是“无超时”。

重新排期条件：配置迁移或环境变量加载链重构时，补一条 CLI 真运行测试，断言旧值不会创建 deadline。

### Checkpoint 端到端屏障覆盖不足

Checkpoint 的单元和 serve 测试覆盖了记录、恢复和 busy 拒绝；真实 CLI 子进程在“写入完成后立即退出”的 fsync 屏障仍缺独立 E2E。

重新排期条件：checkpoint 存储格式或同步级别变化时。测试必须走子进程，不可只 mock store。

### Workbench Find 驱动器依赖启发式定位

VS Code E2E 的 `workbenchFindDriver` 依赖当前 workbench DOM 的可访问性结构。它对产品无运行时影响，但 IDE 升级可能令测试定位失败。

重新排期条件：首次因 workbench DOM 演进失败时。修复应提供有界 fallback，并在 fallback 也失败时输出诊断快照。

## P2

### Hangup 命名残留

若干内部变量/日志仍沿用 `hangup`，实际语义已区分用户 interrupt、已确认 host disconnect 和 restart-pending。它不会改变协议，但会增加排障认知负担。

重新排期条件：下一次统一重命名或 Ask Question 协议版本升级；不要在行为修复中混入大范围 rename。

### Plan Mermaid 的 test id 命名不准确

少数 Plan 图相关 test id 沿用早期组件名，测试覆盖本身有效。改名只会制造无功能收益的快照噪音。

重新排期条件：该组件的可访问性标识重构时，一并迁移并保留兼容选择器窗口。

### Verifier 目前是 dormant

`verify_gate` 配置、提示词和实现仍存在，但完成路径没有自动派发 verifier；现有测试明确锁住这一现状。把它重新接入会改变执行成本、失败语义和用户交互，应作为单独产品决策评审，而不是默认启用。
