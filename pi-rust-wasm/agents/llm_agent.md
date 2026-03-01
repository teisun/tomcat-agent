# llm_agent：LLM 统一接入

## 行为规范

本 Agent 的所有行为、生成代码与执行操作**须严格遵循** [Constitution.md](../openspec/specs/Constitution.md)。与宪法冲突的需求须拒绝并说明原因；安全红线、Agent 与协作规范、完成定义等均按宪法执行。

## 角色名称与目标

负责**LLM 统一接入模块**：定义 LlmProvider Trait、OpenAI 格式适配器、流式/非流式调用、限流与指数退避重试、Token 统计、会话级模型配置。交付可被宿主 API 层与 chat 调用的统一 LLM 能力。

## 负责任务 ID 与顺序

| 顺序 | 任务 ID | 说明 |
|------|---------|------|
| 1 | T1-P0-004 | LLM 统一接入模块落地 |

004 仅依赖 001，可与 infra（002）、session_cli（003）、wasm_plugin（007）并行开发。

## 依赖与协作

- **依赖**：T1-P0-001（AppError、配置、日志等）。
- **被依赖**：wasm_plugin（008 宿主 API 中的 LLM 调用）、chat（011 对话模式）。
- **接口约定**：
  - **LlmProvider**：Trait 含 `provider_name`、`chat`、`chat_stream`、`count_tokens`（与 design CODE_BLOCK_P1_005 一致）。
  - **ChatRequest / ChatResponse / ChatMessage / StreamEvent**：请求与响应类型，供上层与插件侧使用。
  - 会话级模型配置：从 SessionEntry 或配置层读取，与 session_cli 约定的会话级配置隔离一致。

## 参考文档

- [Constitution.md](../openspec/specs/Constitution.md) 行为规范与安全红线（必遵）
- [design.md](../openspec/changes/001-mvp/design.md) 第 2.2 节「LLM 接入模块」、CODE_BLOCK_P1_005
- [tasks_details.md](../openspec/changes/001-mvp/tasks_details.md) T1-P0-004

## 验收标准

- **T1-P0-004**：LlmProvider Trait 与 OpenAI 适配器；非流式 chat、流式 chat_stream（tokio-stream）；限流、指数退避重试、并发控制；Token 消耗统计与会话级汇总；会话级模型配置隔离；**边界**：流式中断/超时时的错误处理与资源释放；单测覆盖率≥80%。
