# Cron 与 Webhooks

## 零、先用大白话

Cron 像 **闹钟**：到点喊 Agent 干一圈。  
Webhooks 像 **门铃**：外面世界（邮件、别的服务）**拍一下**，里面起来处理。

**这一节你会学到**：Cron 在 Gateway 里谁调度；Webhook 从哪进。

---

**设计思想**：Cron 由 Gateway server-cron 驱动，定时执行配置的 jobs；Webhooks 处理外部回调（如 Gmail Pub/Sub），与 hooks 配合。

---

## ASCII 核心四图

### 1) 结构图

```text
Gateway cron service
        |
        v
定时触发 -> Agent / Methods
        |
Webhooks HTTP 入口 -> 校验签名 -> 同源处理队列
```

### 2) 调用流图

```text
cron tick
  -> 选中 job 定义
      -> enqueue lane / invoke tool
          -> 写日志与可选通知

webhook POST
  -> verify
      -> map 到内部事件
          -> 与 hooks 链协作
```

### 3) 时序图

```text
Scheduler    cron job           Agent lane       External mail
     |            |                  |               |
     | tick       |                  |               |
     |----------->| run prompt       |               |
     |            |----------------->|               |
     |            |                  |               |
     | webhook    |                  |               |
     |------------------------------------------------>|
```

### 4) 数据闭环图

```text
外部世界时间/事件
        |
        v
OpenClaw 内产生与用户消息等价的处理
        |
        v
会话与 transcript 记录
        |
        v
调整 cron 表达式或 webhook secret 再观察
```

---

## 一、Cron

- **`src/cron/`**：types、store、schedule、normalize。  
- **`src/cron/service/`**：timer、jobs、ops、state。  
- **`src/gateway/server-cron.ts`**：Gateway 侧定时调度。  
- **`src/cron/isolated-agent/run.ts`**：隔离跑 Agent 任务。  
- **cron-cli**：添加、编辑、列表 jobs。

---

## 二、Webhooks

- **`src/cli/webhooks-cli.ts`**：Gmail 等 webhook 配置。  
- **gmail**：watch、Pub/Sub，与 **hooks** 集成。  
- **gmail-ops**：setup 与操作辅助。

---

## 常见误会

- **误会**：Cron 到了就一定成功跑完。**正解**：Agent 可能 **失败 / 超时**；要看 cron runs 日志。  
- **误会**：Webhook 不用验签。**正解**：外部入口必须 **验签/验 token**；否则等于给公网留洞。  
- **误会**：Webhooks 和 Gateway WS 是一路。**正解**：常见是 **HTTP 回调** 进来自家 handler，再转成内部事件。
