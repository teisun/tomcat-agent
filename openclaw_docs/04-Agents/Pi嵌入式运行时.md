# Pi 嵌入式运行时

## 零、先用大白话

Pi 嵌入式像 **把引擎塞进同一辆车里**。  
不用另起一个远程「大脑服务」。  
模型想事、想调用工具，都在 **本进程** 转圈。  
OpenClaw 在外面再包一层：**排队、订阅、回调**，让 auto-reply、Cron 能稳稳收到 **流式字**。

**这一节你会学到**：一次 attempt 做啥；事件从哪冒出来。

---

**设计思想**：Pi Agent 以嵌入式方式运行在同一进程，通过 `@mariozechner/pi-agent-core` 等库创建 session，不跨进程 RPC。事件通过 `subscribeEmbeddedPiSession` 订阅，流式回调给上层。

---

## ASCII 核心四图

### 1) 结构图

```text
runEmbeddedPiAgent
        |
        v
runEmbeddedAttempt（单次尝试）
        |
        v
Pi AgentSession + subscribeEmbeddedPiSession
```

### 2) 调用流图

```text
上层触发
  -> 构造 Pi 配置与 tools
      -> run.ts 执行 attempt
          -> subscribe 消费 AgentEvent
              -> 映射为 OpenClaw 外向事件
```

### 3) 时序图

```text
auto-reply    run.ts           Pi session        LLM provider
     |          |                  |                 |
     | invoke   |                  |                 |
     |--------->| create+subscribe |                 |
     |          |----------------->| streamSimple   |
     |          |                  |--------------->|
     |          |<-----------------| events         |
```

### 4) 数据闭环图

```text
会话 JSONL（pi 格式）
        |
        v
嵌入式读消息 -> 模型 -> 工具写回
        |
        v
subscribe 把增量推给 Gateway/UI
        |
        v
失败重试 / continue 仍在本进程闭环
```

---

## 一、Run 与 Attempt

- **runEmbeddedPiAgent**：**`src/agents/pi-embedded-runner/run.ts`**（对外入口也经 **`src/agents/pi-embedded.ts`** 再导出）。
- **runEmbeddedAttempt**：**`src/agents/pi-embedded-runner/run/attempt.ts`**，创建 Pi session、构建 payload、调用 Pi API、订阅事件。

---

## 二、Subscribe 与事件流

- **subscribeEmbeddedPiSession**：**`src/agents/pi-embedded-subscribe.ts`**，订阅 Pi session 的 message、tool、agent 事件。
- **事件类型**：message_start/update/end、tool_execution_start/update/end、agent_start/end。
- **handlers**：`pi-embedded-subscribe.handlers.ts` 及同目录下 tools/messages/lifecycle 等处理各事件。

---

## 三、与 Pi 核心库边界

- Pi 库负责：session 管理、模型调用、streaming、tool 调用。  
- OpenClaw 负责：lane 队列、payload 构建、事件订阅与回调、工具实现（gateway-tool 等）、compaction、memory-flush。

---

## 常见误会

- **误会**：嵌入式 = 不能换模型。**正解**：换的是 **provider / 配置**；进程还是同一个。  
- **误会**：subscribe 漏了事件会丢数据。**正解**：关键状态还会 **落盘** 到 transcript；订阅主要是 **实时 UI**。  
- **误会**：Pi 挂了 Gateway 一定挂。**正解**：常见是 **单次 attempt 失败**；上层可重试或给用户报错。
