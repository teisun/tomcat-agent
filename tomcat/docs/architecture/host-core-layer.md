本文为 [Architecture](../openspec/specs/Architecture.md) 中「2. 宿主核心能力层」的入口页。它不重复各专题页的细节，只负责说明宿主主链包含哪些能力，以及应该往哪里下钻。

## 1. 宿主核心能力层是什么

这一层承载 **tomcat** 的可信主引擎：所有核心业务逻辑都在宿主层完成，插件只能通过显式 hostcall 契约访问它提供的能力。

核心职责可以分成 6 组：

1. **会话与 transcript**
2. **LLM 接入与模型切换**
3. **4 原语与内置工具**
4. **Agent Loop 与上下文控制**
5. **PLAN / EXEC、Checkpoint 与验证链**
6. **插件生命周期与扩展接入**

## 2. 推荐下钻顺序

### 会话与存储

- [`session-storage.md`](./session-storage.md)
- [`session-modes.md`](./session-modes.md)
- [`chat-resume-hydration.md`](./chat-resume-hydration.md)

### Agent Loop 与上下文

- [`agent-loop.md`](./agent-loop.md)
- [`context-management.md`](./context-management.md)
- [`interrupt-and-cancellation.md`](./interrupt-and-cancellation.md)
- [`current-tail-aggregate-guard.md`](./current-tail-aggregate-guard.md)

### LLM 与模型集成

- [`llm-multiprovider-integration.md`](./llm-multiprovider-integration.md)
- [`llm-openai-deepseek-reasoning-continuity.md`](./llm-openai-deepseek-reasoning-continuity.md)
- [`llm-multi-llm-productization.md`](./llm-multi-llm-productization.md)
- [`llm-stream-events-cli-pipeline.md`](./llm-stream-events-cli-pipeline.md)
- [`llm-files-upload-manager.md`](./llm-files-upload-manager.md)

### 计划与执行态

- [`plan-runtime.md`](./plan-runtime.md)
- [`plan-exec-code-verification.md`](./plan-exec-code-verification.md)
- [`tools/checkpoint-resume.md`](./tools/checkpoint-resume.md)

### 插件与扩展

- [`plugin-system-overview.md`](./plugin-system-overview.md)
- [`skill-system.md`](./skill-system.md)
- [`package-manager.md`](./package-manager.md)

## 3. 主链关系

```text
user input
   │
   ▼
session + transcript
   │
   ▼
agent loop
   │
   ├─ built-in tools / permission gate
   ├─ llm provider
   ├─ plan runtime / verifier
   └─ plugin runtime
```

## 4. 当前边界

- 这页是**入口页**，不再重复各专题页的协议和状态机细节。
- 专题页之间尽量避免横向串链；宿主核心能力层统一由本页和 [`project-overview-panorama.md`](./project-overview-panorama.md) 负责导航。
