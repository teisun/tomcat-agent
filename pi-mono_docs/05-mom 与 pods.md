# pi-mom 与 GPU pods（@mariozechner/pi）

## 先用大白话

- **pi-mom**：挂在 **Slack** 上的机器人。有人 @它或私聊它，它就把话转给「和 **pi** 同一套」的编码 Agent 逻辑，并在自己管的工作区里读写文件、跑命令。
- **pods 包**：名字在磁盘上是 **`packages/pods/`**，但 npm 包叫 **`@mariozechner/pi`**。它提供 **`pi pods` / `pi start` / `pi agent`** 等命令，帮你在远程 GPU 机器上装 **vLLM**、起模型、看日志。

别把 **`@mariozechner/pi-coding-agent` 安装出来的 `pi`** 和 **`@mariozechner/pi`（pods 包）提供的 `pi`** 搞混；两个都能叫 `pi`，同时全局安装时要靠 PATH 或别名区分。

---

## pi-mom（`@mariozechner/pi-mom`）

### 做什么

- Slack **Socket Mode**；频道 @mention 或 DM 触发。
- 工作区里常见：`log.jsonl`（全量）、`context.jsonl`（送进模型的裁剪视图）、`MEMORY.md`、`attachments/`、`skills/` 等（以源码与 `packages/mom/docs` 为准）。

### 怎么跑

- 命令形态：`mom [options] <working-directory>`（工作目录即 data 根）。
- **`--sandbox=docker:…`** 比 host 更安全。
- 环境变量：`MOM_SLACK_APP_TOKEN`、`MOM_SLACK_BOT_TOKEN`；模型密钥可参考仓库文档链到 `~/.pi/agent/auth.json`。

### 文档在哪

仓库内：**`packages/mom/docs/`**（如 `slack-bot-minimal-guide.md`、`sandbox.md`、`events.md` 等）。

---

## @mariozechner/pi（目录 `packages/pods/`）

### npm 与二进制

- **`package.json` 的 `name`**：`@mariozechner/pi`
- **`bin`**：字段里映射为 **`pi-pods`** 指向 `dist/cli.js`（若全局安装该包，可执行文件名以 npm 安装结果为准；**帮助文案里自称为 `pi v…`**，即该 CLI 的主名可能是 `pi` 或 `pi-pods`，取决于安装方式）。

### 常用子命令（摘自 CLI help）

- **Pod**：`pi pods setup …`、`pi pods`、`pi pods active …`、`pi pods remove …`；还有 `pi shell`、`pi ssh`。
- **模型**：`pi start …`、`pi stop …`、`pi list`、`pi logs …`。
- **试 agent**：`pi agent …`（单次或 `-i` 交互）。

### 文档在哪

**`packages/pods/docs/`**（模型说明、实现笔记等）。

---

## ASCII：和 coding-agent 的关系

```
  Slack  --->  pi-mom  --->  复用 pi-coding-agent / Agent 那套能力
                               |
  远程 GPU  --->  @mariozechner/pi (pods) ---> vLLM + OpenAI 兼容 HTTP
                               |
                               +---> 与 pi-coding-agent 无强制同进程关系
```

---

## 关键路径

| 用途 | 路径 |
|------|------|
| mom 源码与文档 | `packages/mom/` |
| pods 源码与文档 | `packages/pods/`（npm 名 `@mariozechner/pi`） |
