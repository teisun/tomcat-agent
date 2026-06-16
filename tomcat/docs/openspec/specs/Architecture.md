# tomcat 整体技术架构

本文是项目级架构蓝图与总索引。模块级技术方案统一收口在 [`../../architecture/README.md`](../../architecture/README.md)；本文只保留全局原则、分层边界与阅读入口。

## 设计原则

1. **稳定契约优先**：对插件、工具、计划、事件和持久化格式的外部契约保持稳定，减少“代码跑了但文档没跟上”的分叉。
2. **宿主可信、插件受控**：对话、LLM、权限、审计与 4 原语都在宿主层实现；插件只能走显式 hostcall 契约。
3. **单向分层**：基础设施 → 宿主核心 → 宿主 API / 插件边界 → 交互层，避免循环依赖。
4. **渐进式披露**：文档与运行时都尽量减少无必要展开；入口页负责导航，专题页只展开自己的真相源。
5. **删除优于保留旧壳**：已退出主路径的历史方案直接删除，必要背景回写到当前文档的“历史决策”小节。

## 总体分层

```text
基础设施层
  ↓
宿主核心能力层
  ↓
宿主 API / 插件边界
  ↓
插件运行时与沙箱
  ↓
交互层
```

## 项目全貌

第一次进入本项目，推荐先读 [`../../architecture/project-overview-panorama.md`](../../architecture/project-overview-panorama.md)。

## 各层与主题入口

### 1. 基础设施层

底层可信能力：配置、路径、日志、审计、事件总线、跨平台适配。

详见：

- [`../../architecture/infrastructure-layer.md`](../../architecture/infrastructure-layer.md)
- [`../../architecture/audit-log.md`](../../architecture/audit-log.md)

### 2. 宿主核心能力层

承载会话、LLM、4 原语、Agent Loop、计划运行态、插件生命周期等宿主主链。

详见：

- [`../../architecture/host-core-layer.md`](../../architecture/host-core-layer.md)
- [`../../architecture/session-storage.md`](../../architecture/session-storage.md)
- [`../../architecture/session-modes.md`](../../architecture/session-modes.md)
- [`../../architecture/agent-loop.md`](../../architecture/agent-loop.md)
- [`../../architecture/context-management.md`](../../architecture/context-management.md)

### 3. 宿主 API 层

宿主向插件暴露的唯一可信边界。它定义 `pi.*`、`__pi_host_call`、`HostRequest` / `HostResponse` 与宿主扩展点契约。

详见：

- [`../../architecture/plugin-system/js-bridge-and-host-api.md`](../../architecture/plugin-system/js-bridge-and-host-api.md)
- [`../../architecture/plugin-system/host-call-protocol.md`](../../architecture/plugin-system/host-call-protocol.md)

### 4. 插件系统（统一入口）

当前插件体系以 **进程内 `rquickjs`** 为准。阅读顺序与专题划分统一从总览页进入。

详见：

- [`../../architecture/plugin-system-overview.md`](../../architecture/plugin-system-overview.md)
- [`../../architecture/plugin-system/plugin-source-scan-register-load.md`](../../architecture/plugin-system/plugin-source-scan-register-load.md)
- [`../../architecture/plugin-system/runtime-and-sandbox.md`](../../architecture/plugin-system/runtime-and-sandbox.md)
- [`../../architecture/plugin-system/events.md`](../../architecture/plugin-system/events.md)
- [`../../../src/ext/README.md`](../../../src/ext/README.md)

### 5. 交互层

CLI、slash command、流式前台输出与后续图形界面的统一入口语义。

详见：

- [`../../architecture/interaction-layer.md`](../../architecture/interaction-layer.md)
- [`../../user-guide.md`](../../user-guide.md)

### 6. 安全设计核心原则

最小权限、唯一通道、软隔离、用户知情与全链路审计。

详见：

- [`../../architecture/security.md`](../../architecture/security.md)
- [`../../architecture/permission-system.md`](../../architecture/permission-system.md)

### 7. 会话存储与模式

会话 transcript、元数据 store、模式切换与恢复路径。

详见：

- [`../../architecture/session-storage.md`](../../architecture/session-storage.md)
- [`../../architecture/session-modes.md`](../../architecture/session-modes.md)
- [`../../architecture/chat-resume-hydration.md`](../../architecture/chat-resume-hydration.md)

### 8. 工作目录与数据布局

`work_dir`、三层根、账本与可视化目录树。

详见：

- [`../../architecture/work-dir-and-data-layout.md`](../../architecture/work-dir-and-data-layout.md)
- [`../../architecture/directory-structure.md`](../../architecture/directory-structure.md)

### 9. Agent Loop 设计

LLM 调用、工具执行、容错重试、事件发布与上下文预算控制。

详见：

- [`../../architecture/agent-loop.md`](../../architecture/agent-loop.md)
- [`../../architecture/current-tail-aggregate-guard.md`](../../architecture/current-tail-aggregate-guard.md)
- [`../../architecture/interrupt-and-cancellation.md`](../../architecture/interrupt-and-cancellation.md)

### 10. 计划运行态、Checkpoint 与执行完成验证

PLAN / EXEC、`todos`、checkpoint、verifier 与执行态收口链路。

详见：

- [`../../architecture/plan-runtime.md`](../../architecture/plan-runtime.md)
- [`../../architecture/plan-exec-code-verification.md`](../../architecture/plan-exec-code-verification.md)
- [`../../architecture/tools/checkpoint-resume.md`](../../architecture/tools/checkpoint-resume.md)

### 11. 多 Agent 与扩展子系统

多会话并发、主子编排、skill、package、LLM 扩展与长期演进能力。

详见：

- [`../../architecture/multi-agent.md`](../../architecture/multi-agent.md)
- [`../../architecture/skill-system.md`](../../architecture/skill-system.md)
- [`../../architecture/package-manager.md`](../../architecture/package-manager.md)
- [`../../architecture/README.md`](../../architecture/README.md)

## 详细设计索引

完整主题分组与阅读顺序见 [`../../architecture/README.md`](../../architecture/README.md)。如果你已经知道自己要找哪个专题，直接从该 README 进入对应入口页，不必在同级文档之间连环跳转。

## 新增架构文档约束

新增到 `docs/architecture/` 的 `*.md` 仍需遵循 [`guides/workflow/ARCHITECTURE_SPEC.md`](guides/workflow/ARCHITECTURE_SPEC.md)。其中最重要的三条是：

1. 入口页负责导航，专题页负责单一事实源。
2. 必须有清晰的协议、One-Glance、测试矩阵与风险小节。
3. 非必要不要让同级文档互相套娃式引用。
