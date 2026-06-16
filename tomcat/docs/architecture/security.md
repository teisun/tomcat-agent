本文为 [Architecture](../Architecture.md) 中「7. 安全设计核心原则」的详细设计，总览见主文档。

## 7. 安全设计核心原则

- **最小权限原则**：插件默认最小权限，仅授予完成任务所需的宿主 API，禁止过度授权；针对工作目录 / 凭据 / agent 自身审计目录的细粒度权限分级（read/write/bash 三层决策、`path_rules deny/readonly`、Bash 结构化解析）见 [权限子系统设计](permission-system.md)
- **完全隔离原则**：每个插件运行在独立的 `rquickjs` 插件 VM 中，结合 `VmActor`、超时、中断预算与堆上限实现软隔离，故障不扩散
- **唯一通道原则**：插件仅能通过显式注册的宿主 API 与宿主交互，禁止绕过 API 的系统访问
- **用户知情权原则**：4 原语与高危操作须告知用户并获二次确认，禁止静默执行；`ConfirmDecision { AllowOnce / AllowAndPersistRoot / Deny }` 三选项语义、拖拽 UX 5 选项菜单与 `WorkspaceStateSection` 启动横幅见 [权限子系统设计](permission-system.md)
- **错误完全隔离原则**：插件与事件回调的错误独立捕获，不导致宿主崩溃
- **全链路审计原则**：4 原语、工具调用、插件生命周期、高危操作留存完整审计日志，可追溯
- **代码安全校验原则**：插件加载前须安全扫描，禁止恶意或越权代码加载
- **资源硬配额 (Hard Quotas)**：配额数值由当前插件运行时配置决定（如 `js_heap_mb`、`call_timeout_ms`、`interrupt_budget`）。
  - **内存隔离**：每个插件 VM 可设置 `js_heap_mb`，防止插件 OOM 影响宿主。
  - **执行时限**：基于墙钟超时 + interrupt budget，限制单次任务的最大执行量，防止死循环导致宿主 CPU 挂起。
  - **API 调用限流**：在宿主 API 分发层实现逻辑限流，防止插件高频攻击宿主可信原语。

---

**TODO**：敏感数据加密（如 LLM API 密钥）后续再考虑。
