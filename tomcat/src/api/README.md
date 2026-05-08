# API 层说明（CLI / chat / render）

## 1. 职责

- **入口**：`main.rs` 调用 `run_cli()`（`pub use api::run_cli`）。
- **子命令**：`cli.rs` — clap 定义与 `run_init` / `run_doctor` / `run_config` / `run_session` / `run_plugin` / `run_audit` / `run_chat` 等。
- **对话**：`chat/mod.rs` 等 — `ChatContext`、`chat_loop`、与 `AgentLoop` / `SessionManager` / 工具执行的衔接。
- **渲染**：`render.rs` — Markdown 流式输出与 syntect 高亮。

会话存储、JSONL transcript、SessionManager 细节见 [../core/session/README.md](../core/session/README.md)。Agent 循环、Compaction、工具原语见 [../core/README.md](../core/README.md)。

## 2. 核心文件

| 文件 | 作用 |
|------|------|
| `src/api/cli.rs` | 子命令解析与各 handler |
| `src/api/chat/mod.rs` 等 | 交互式对话主循环、工具调用、确认流 |
| `src/api/render.rs` | 流式 Markdown / 代码块渲染 |
| `src/main.rs` | 二进制入口 → `run_cli()` |

## 3. CLI 与会话 / Agent 关系（ASCII）

```text
  tomcat <subcommand> / tomcat (默认 chat)
            |
            v
     +------+-------+
     | src/api/cli  |
     +------+-------+
            |
     +------v-------------+----------------------+
     | SessionManager      |  其他子命令 init/   |
     | + transcript        |  doctor/config/...  |
     +---------------------+----------------------+
            |
            v  (chat 路径)
     +------+-------+
     | api/chat     | -> AgentLoop + LlmProvider + Primitives
     +--------------+
```

## 4. 延伸阅读

- [src 模块索引](../README.md) 图 2（数据面）
- [infra/README.md](../infra/README.md)（配置与日志）
- [ext/README.md](../ext/README.md)（插件 / Wasm，若 chat 或 CLI 加载插件）
