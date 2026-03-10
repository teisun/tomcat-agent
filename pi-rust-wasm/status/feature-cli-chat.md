# Status: feature/cli-chat

## 元数据

| 字段 | 值 |
|------|------|
| 分支 | `feature/cli-chat` |
| 任务 | TASK-03 / T1-P0-011 CLI 对话模式核心实现 |
| 负责人 | Spike |
| 状态 | DONE |
| Cov% | 60.9% |
| 创建时间 | 2026-03-10 |
| 更新时间 | 2026-03-10 |

---

## 完成子项

| 子项 | 状态 | 说明 |
|------|------|------|
| 11.1 对话主循环 | DONE | ChatContext + chat_loop + rustyline |
| 11.2 流式响应渲染 | DONE | StreamEvent::ContentDelta 实时输出 |
| 11.3 Markdown/代码高亮 | DONE | MarkdownRenderer + syntect |
| 11.4 多轮上下文 | DONE | build_context_messages + append_message |
| 11.5 工具/4 原语 | DONE | ToolCallDelta 解析、execute_tool_call、CliConfirmation |
| 11.6 快捷键/--resume | DONE | Ctrl+C/D、rustyline history、--resume flag |
| 11.7 边界/验收 | DONE | effective_model 会话隔离、单测 |

---

## 覆盖率说明

整体覆盖率 60.9%（1424/2338 行），较 develop 下降约 4.7%。
主要原因：chat.rs 中 70/279 行（25%）为交互式代码（主循环、stdin 读取、LLM 调用、流式消费），无法在单元测试中覆盖。
render.rs 覆盖率 62/76（82%），types.rs/openai.rs 变更部分已有测试覆盖。

---

## 变更文件

- `Cargo.toml` — 新增依赖 rustyline、syntect、ctrlc
- `src/api/mod.rs` — 新增 chat、render 模块
- `src/api/chat.rs` — 新增：ChatContext、chat_loop、工具调用、CliConfirmation
- `src/api/render.rs` — 新增：MarkdownRenderer（流式代码块高亮）
- `src/api/cli.rs` — Commands::Chat 加 --resume，run_chat 改为真实实现
- `src/core/llm/types.rs` — ChatMessage 支持 tool_calls/tool_call_id；ChatRequest 支持 tools；StreamEvent 新增 ToolCallDelta
- `src/core/llm/openai.rs` — SSE 解析支持 tool_calls 增量；请求体传 tools
- `src/ext/dispatcher.rs` — ChatRequest 构造加 tools 字段
- `tests/cli_tests.rs` — chat 测试改为验证无配置时的失败行为
- `tests/llm_tests.rs` — ChatRequest 构造加 tools 字段
- `docs/02-session-and-cli.md` — 补充对话模式文档
- `agents/TASK_BOARD.md` — TASK-03 认领为 DOING

---

## 阻塞

无。
