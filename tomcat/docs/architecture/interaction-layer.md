本文为 [Architecture](../openspec/specs/Architecture.md) 中「5. 交互层」的入口页，说明用户是通过哪些前台入口触达宿主核心能力的。

## 1. 当前交互层边界

交互层只负责“把用户动作接进来，并把宿主结果安全地送出去”，不自己承载业务真相。

当前主要前台包括：

- CLI 命令面
- 会话内 slash command
- 流式终端输出与事件展示
- 为未来 Web / TUI / IPC 预留的一致接口形态

## 2. 当前重点入口

- 用户使用与命令示例：[`../user-guide.md`](../user-guide.md)
- 会话模式与前台行为：[`session-modes.md`](./session-modes.md)
- PLAN / EXEC 相关前台命令：[`plan-runtime.md`](./plan-runtime.md)
- 流式事件与终端表现：[`llm-stream-events-cli-pipeline.md`](./llm-stream-events-cli-pipeline.md)
- 对外暴露给 VSCode / 桌面 GUI（Agent Server / Gateway / stdio 进程边界）：[`agent-server-and-ui-gateway.md`](./agent-server-and-ui-gateway.md)（§3「为未来 Web / TUI / IPC 预留一致接口形态」的具体落地方案）

## 3. 设计约束

- 前台命令面尽量薄：真正的协议、状态机和持久化规则都留在宿主核心层和专题页。
- CLI、slash command、后续图形界面应尽量复用同一套宿主能力，而不是各自复制逻辑。
- 用户看见的是统一的产品行为，内部可以有多条实现链，但不应产生多套真相源。
