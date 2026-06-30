# `docs/architecture` 文档地图

本目录存放 **tomcat** 的模块级技术方案。顶层蓝图与分层总索引见 [`../openspec/specs/Architecture.md`](../openspec/specs/Architecture.md)；本文负责给出 `docs/architecture/` 内部的阅读顺序、主题分组与链接约束。

## 阅读顺序

1. 先读 [`project-overview-panorama.md`](./project-overview-panorama.md)，建立整体分层与主调用链心智模型。
2. 再按主题进入对应入口页：
   - 宿主主链：[`host-core-layer.md`](./host-core-layer.md)
   - 插件系统：[`plugin-system-overview.md`](./plugin-system-overview.md)
   - 会话与上下文：[`session-modes.md`](./session-modes.md)、[`session-storage.md`](./session-storage.md)、[`context-management.md`](./context-management.md)
   - 计划与执行态：[`plan-runtime.md`](./plan-runtime.md)、[`plan-exec-code-verification.md`](./plan-exec-code-verification.md)
3. 最后按需下钻到工具、LLM、权限、审计和实现侧 README。

## 主题分组

### 入口页

- [`project-overview-panorama.md`](./project-overview-panorama.md)：项目全貌与分层总览。
- [`host-core-layer.md`](./host-core-layer.md)：宿主核心能力入口页。
- [`plugin-system-overview.md`](./plugin-system-overview.md)：插件系统入口页。
- [`directory-structure.md`](./directory-structure.md)：工作目录树的可视化 companion。

### 宿主主链

- [`infrastructure-layer.md`](./infrastructure-layer.md)
- [`interaction-layer.md`](./interaction-layer.md)
- [`agent-server-and-ui-gateway.md`](./agent-server-and-ui-gateway.md)：把 CLI 能力暴露给 VSCode / 桌面 GUI 的进程边界（Agent Server / Gateway / stdio）方案。
- [`security.md`](./security.md)
- [`permission-system.md`](./permission-system.md)
- [`audit-log.md`](./audit-log.md)

### 会话、上下文与运行态

- [`session-modes.md`](./session-modes.md)
- [`session-storage.md`](./session-storage.md)
- [`chat-resume-hydration.md`](./chat-resume-hydration.md)
- [`transcript-stable-id-and-stream-reconciliation.md`](./transcript-stable-id-and-stream-reconciliation.md)：assistant 稳定 `entry.id` 如何贯穿 streaming / transcript / history replay，并为 webview `upsert-by-id` 提供协议锚点。
- [`context-management.md`](./context-management.md)
- [`agent-loop.md`](./agent-loop.md)
- [`interrupt-and-cancellation.md`](./interrupt-and-cancellation.md)
- [`current-tail-aggregate-guard.md`](./current-tail-aggregate-guard.md)
- [`multi-agent.md`](./multi-agent.md)

### 插件系统

- [`plugin-system-overview.md`](./plugin-system-overview.md)
- [`plugin-system/plugin-source-scan-register-load.md`](./plugin-system/plugin-source-scan-register-load.md)
- [`plugin-system/js-bridge-and-host-api.md`](./plugin-system/js-bridge-and-host-api.md)
- [`plugin-system/host-call-protocol.md`](./plugin-system/host-call-protocol.md)
- [`plugin-system/runtime-and-sandbox.md`](./plugin-system/runtime-and-sandbox.md)
- [`plugin-system/events.md`](./plugin-system/events.md)
- [`../../src/ext/README.md`](../../src/ext/README.md)：实现侧代码入口图。

### 计划、工具与扩展子系统

- [`plan-runtime.md`](./plan-runtime.md)
- [`plan-exec-code-verification.md`](./plan-exec-code-verification.md)
- [`tools/`](./tools/)
- [`skill-system.md`](./skill-system.md)
- [`package-manager.md`](./package-manager.md)

### LLM 与模型集成

- [`llm-multiprovider-integration.md`](./llm-multiprovider-integration.md)
- [`llm-openai-deepseek-reasoning-continuity.md`](./llm-openai-deepseek-reasoning-continuity.md)
- [`llm-multi-llm-productization.md`](./llm-multi-llm-productization.md)
- [`llm-stream-events-cli-pipeline.md`](./llm-stream-events-cli-pipeline.md)
- [`llm-files-upload-manager.md`](./llm-files-upload-manager.md)

### 存储与工作目录

- [`work-dir-and-data-layout.md`](./work-dir-and-data-layout.md)：规则与语义的单一事实源。
- [`directory-structure.md`](./directory-structure.md)：面向人阅读的目录树示意。

## 链接规则

- **父文档负责向下导航**：`Architecture.md`、本文、各入口页负责组织阅读顺序。
- **子文档默认只回链父文档**：专题页只在需要声明协议归属、单一事实源或迁移关系时，才引用同级文档。
- **避免链式跳转**：不要把 A 文档写成“去看 B”，B 又写成“去看 C”，C 再回 A。
- **历史背景内聚到现行文档**：已废弃方案直接删除；确有保留价值的背景统一写进当前文档的“历史决策 / 跨文档修订”小节。
