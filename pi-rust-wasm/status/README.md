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

与 INTEGRATION.md 中「每个角色一节」的格式一致：state、branch、DONE、INTERFACE、BLOCKED、覆盖率等。便于汇总脚本按节拼接。
