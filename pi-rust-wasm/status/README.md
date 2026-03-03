# status/ 进度碎片目录

各功能分支的 Agent **只在此目录下维护以本分支命名的进度文件**，不直接修改根目录的 `INTEGRATION.md`。  
`INTEGRATION.md` 由在 develop 分支上执行的「汇总 status 到 INTEGRATION」command 自动生成。

## 命名约定

分支名中的 `/` 转为 `-`，对应文件名如下：

| 分支 | 文件名 |
|------|--------|
| feature/infra | feature-infra.md |
| feature/session-cli | feature-session-cli.md |
| feature/llm | feature-llm.md |
| feature/wasm-plugin | feature-wasm-plugin.md |
| feature/primitives-tools | feature-primitives-tools.md |
| feature/chat | feature-chat.md |

## 内容格式

**格式与示例**见 [Constitution.md](../openspec/specs/Constitution.md) 附录 A「status/feature-xx.md 进度格式」。本目录仅约定文件名与分支对应关系，不重复书写格式细节。

若已有碎片为旧格式（每个字段单独 `## who`、`## date` 等），建议逐步迁移到 Constitution 约定新格式，以便看板汇总后层次清晰。
