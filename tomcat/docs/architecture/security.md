本文为 [Architecture](../openspec/specs/Architecture.md) 中「6. 安全设计核心原则」的入口页，汇总 tomcat 在宿主、工具、插件与持久化层面的安全约束。

## 1. 核心原则

- **最小权限**：默认不给额外能力，真正需要时再显式放开。
- **唯一通道**：敏感操作必须走 4 原语、hostcall 与统一宿主调度链。
- **软隔离优先**：插件运行在受控 `rquickjs` VM 中，用预算、超时、堆上限和 `VmActor` 做隔离。
- **用户知情**：高风险动作需要可审计、可解释、可拒绝。
- **全链路可追踪**：权限决策、工具调用、插件生命周期、审计落盘能串成闭环。

## 2. 安全主题分工

- [`permission-system.md`](./permission-system.md)：工作区路径授权、确认模型、`PermissionGate`。
- [`audit-log.md`](./audit-log.md)：审计日志落盘、导出和保留。
- [`plugin-system/runtime-and-sandbox.md`](./plugin-system/runtime-and-sandbox.md)：插件运行时隔离、资源预算与 VM 生命周期。
- [`plugin-system/host-call-protocol.md`](./plugin-system/host-call-protocol.md)：插件只能通过显式 hostcall 使用宿主能力。

## 3. 当前边界

- 这里讲的是**原则与总约束**，不是某一项协议的完整细节。
- 具体权限字段、确认选项和路径判定规则只在 `permission-system.md` 里展开。
- 具体 VM 预算字段、隔离模型与回收策略只在插件运行时文档里展开。

## 4. 一句话总结

tomcat 的安全模型不是“插件绝对不能做事”，而是“插件和工具做的每件敏感事都必须走宿主可审计、可中断、可拒绝的统一通道”。
