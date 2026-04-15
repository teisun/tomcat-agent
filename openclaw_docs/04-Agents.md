# Agents 模块

**版本**：见 [README.md](README.md) 同步表；路径相对 **`openclaw/` 仓库根**。

## 零、先用大白话

Agent 像 **会干活的副驾驶**。  
它读聊天、翻记忆、按工具。  
但 **同一组对话里不能两个人同时抢方向盘**：所以 OpenClaw 用 **一条车道（session lane）**，请求 **排队** 跑完再跑下一个。  
这样 `jsonl` 账本不会写乱。

**这一节你会学到**：谁触发 Agent；`runEmbeddedPiAgent` 从哪进；工具和沙箱在哪管。

---

## ASCII 核心四图

### 1) 结构图

```text
CLI / auto-reply / Cron / Hooks（谁喊一声「开工」）
        |
        v
enqueueCommandInLane（每个 session 一条队）
        |
        v
runEmbeddedPiAgent -> Pi session + 工具表
```

### 2) 调用流图

```text
命令入队
  -> lane 一次只取一个
      -> runEmbeddedAttempt
          -> 模型流式输出 + 工具往返
              -> 写 transcript / 触发 memory flush 等收尾
```

### 3) 时序图

```text
Caller       Lane queue        Pi runtime        Tool
  |              |                 |               |
  | prompt       |                 |               |
  |------------->| 出队            |               |
  |              |---------------->| stream        |
  |              |                 |------------->|
  |              |                 |<-------------|
```

### 4) 数据闭环图

```text
同 session 多请求同时到达
        |
        v
lane 串行化 -> 账本顺序一致
        |
        v
压缩 / 换模型 / 改 tool-policy
        |
        v
下一回合按新策略跑
```

---

## 一、职责（三件事）

1. **把 Pi 嵌进进程**：不另起一个「远程大脑服务」，省事、好调试（也有代价：占内存）。  
2. **排队**：`resolveSessionLane` + `enqueueCommandInLane`。  
3. **工具与安全**：哪些钮能按，看 **`tool-policy`**、**sandbox**、**审批**（见 [16-Sandbox与Approvals.md](16-Sandbox与Approvals.md)）。

---

## 二、入口（工程师锚点）

- **对外入口**：`src/agents/pi-embedded.ts` 的 **`runEmbeddedPiAgent`**。  
- **真正跑一轮**：`src/agents/pi-embedded-runner/run.ts` 与 **`run/attempt.ts`**。  
- **订阅事件**：`src/agents/pi-embedded-subscribe.ts`（流式、工具开始/结束等）。

**故事线**：

```text
runEmbeddedPiAgent
  -> resolveSessionLane(sessionKey)
  -> enqueueCommandInLane(...)
  -> runEmbeddedAttempt
  -> subscribeEmbeddedPiSession
  -> onPartialReply / onToolResult / ...
```

---

## 三、工具从哪来

- **目录**：`src/agents/tools/`（gateway、sessions、browser…）。  
- **总表**：`src/agents/tool-catalog.ts`（名字以仓库为准）。  
- **组装进 Pi**：`src/agents/pi-tools.ts`。  
- **策略**：`src/agents/tool-policy.ts`。

---

## 四、子文档

- [Pi嵌入式运行时](04-Agents/Pi嵌入式运行时.md)  
- [工具流与订阅](04-Agents/工具流与订阅.md)  
- [技能与Pi适配](04-Agents/技能与Pi适配.md)  
- 和 **Memory** 的边界见 [06-Memory.md](06-Memory.md)。

---

## 常见误会

- **误会**：Agent = Gateway。**正解**：Gateway 是塔台；Agent 是 **推理+工具** 那条线。  
- **误会**：同一 session 开多线程更快。**正解**：**故意串行**，为了账本一致；要快用别的产品形态（子 agent 等另说）。  
- **误会**：工具越多越好。**正解**：弱模型会被工具说明 **淹没**；有 `localModelMode: lean` 一类策略（见 [00-主PRD.md](00-主PRD.md)）。
