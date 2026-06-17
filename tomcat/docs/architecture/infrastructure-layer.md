本文为 [Architecture](../openspec/specs/Architecture.md) 中「1. 基础设施层」的入口页，说明所有上层模块共同依赖的可信底座。

## 1. 基础设施层负责什么

基础设施层不直接承载对话、计划或插件业务语义，它负责把“系统怎么稳定、安全、可追踪地运行”这件事钉死。

当前这一层主要提供：

- 统一错误模型与错误传播约束
- 配置读取、默认值、环境变量覆盖
- 日志、审计与可观测性基础
- 路径、文件、进程等跨平台适配
- 事件总线与通用并发原语

## 2. 这一层不负责什么

- 不直接决定具体的权限策略
- 不直接定义 Agent Loop、插件事件或计划状态机的业务语义
- 不直接暴露用户可见命令面

换句话说，基础设施层解决的是“上层都得依赖的共性机制”，而不是“某个子系统自己的业务规则”。

## 3. 下钻阅读

- [`audit-log.md`](./audit-log.md)：审计日志结构、导出与存储。
- [`permission-system.md`](./permission-system.md)：建立在路径归一化、配置与审计之上的权限规则。
- [`work-dir-and-data-layout.md`](./work-dir-and-data-layout.md)：工作目录、账本与持久化根路径。

## 4. 与上层的关系

```text
infra
  ├─ config / path / fs / process
  ├─ tracing / audit
  ├─ event bus
  └─ shared error handling
       │
       ▼
host core / session / llm / tools / plugin runtime
```

一句话说，基础设施层负责把“系统怎么稳地跑起来”收口成统一底座；其上的层只在这套底座上组织自己的协议、生命周期与用户语义。
