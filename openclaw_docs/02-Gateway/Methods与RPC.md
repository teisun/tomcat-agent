# Gateway Methods 与 RPC

## 零、先用大白话

Methods 像 **塔台指令清单**。  
每条指令有 **名字** 和 **参数**：拉历史、改配置、点名某个 Node。  
`coreGatewayHandlers` 像 **接线员**：看一眼表，把活转给背后那间办公室。

**这一节你会学到**：方法从哪列出来；请求在代码里怎么走一圈。

---

**设计思想**：Gateway 通过 `listGatewayMethods` 注册一整套 JSON-RPC 风格方法，客户端通过 WebSocket 发送 `{ method, params, id }` 调用。方法实现分布在 `server-methods.ts` 与 `server-methods/*.ts`，由 `coreGatewayHandlers` 统一分发。

---

## ASCII 核心四图

### 1) 结构图

```text
listGatewayMethods（清单）
        |
        v
coreGatewayHandlers（总线）
        |
        +--> chat.* / sessions.* / config.* / node.* / cron.* ...
```

### 2) 调用流图

```text
{ method, params, id }
  -> 查找 handler 表
      -> 校验 params schema（若有）
          -> await 业务实现
              -> { id, result } 或 error shape
```

### 3) 时序图

```text
Client        message-handler      coreGatewayHandlers    Domain
  |                 |                      |                |
  | invoke method   |                      |                |
  |---------------->|--------------------->|--------------->|
  |                 |                      |<---------------|
  |<----------------|<---------------------|                |
```

### 4) 数据闭环图

```text
新增 server-method
        |
        v
登记到 methods 列表 + 文档
        |
        v
CLI/UI 调用验证
        |
        v
错误码/日志回流 -> 收紧校验或补集成测试
```

---

## 一、方法列表（节选）

**来源**：`src/gateway/server-methods-list.ts`

| 类别 | 方法示例 |
|------|----------|
| 健康与状态 | health、status、usage.status、usage.cost |
| 配置 | config.get、config.set、config.apply、config.patch、config.schema |
| 会话 | sessions.list、sessions.preview、sessions.patch、sessions.reset、sessions.delete、sessions.compact |
| 聊天 | chat.history、chat.abort、chat.send、agent、agent.wait |
| 执行审批 | exec.approvals.get、exec.approvals.set、exec.approval.request、exec.approval.resolve |
| Wizard | wizard.start、wizard.next、wizard.cancel、wizard.status |
| 模型与 Agent | models.list、agents.list |
| 技能 | skills.status、skills.bins、skills.install、skills.update |
| Nodes | node.list、node.describe、node.invoke、node.invoke.result、node.event、node.pair.* |
| Cron | cron.list、cron.status、cron.add、cron.update、cron.remove、cron.run、cron.runs |
| 通道 | channels.status、channels.logout |
| 其他 | send、wake、system-presence、system-event、last-heartbeat、set-heartbeats |

**插件扩展**：各 Channel 可通过 `gatewayMethods` 注册额外方法，与 `BASE_METHODS` 合并后返回。

---

## 二、调用路径

```
WS 消息入站
  → message-handler 解析 method、params、id
  → coreGatewayHandlers[method] 或 动态查找
  → server-methods/* 中具体实现
  → 返回 result 或 error
```

**核心分发**：`src/gateway/server-methods.ts` 的 `coreGatewayHandlers` 映射 method 到 handler。

---

## 三、关键方法实现位置

| 方法前缀 | 实现文件 |
|----------|----------|
| chat.* | server-methods/chat.ts |
| config.* | server-methods/config.ts |
| sessions.* | server-methods/sessions.ts |
| exec.approval* | server-methods/exec-approval.ts |
| node.* | server-methods/nodes*.ts |
| cron.* | server-methods/cron*.ts |
| wizard.* | server-methods/wizard.ts |
| agent、agent.* | server-methods/agent.ts |

---

## 四、错误处理

- 方法内部 throw 的 Error 会被捕获，转换为 `{ id, error: { code, message } }` 返回。
- 未知 method 返回 method not found 类错误。

---

## 常见误会

- **误会**：`listGatewayMethods` 里有的名字就一定能调用成功。**正解**：有的方法在启动阶段会暂时 **不可用**（维护模式或依赖没起）。  
- **误会**：插件方法和内置方法重名时两个都存在。**正解**：合并有规则；以 **`BASE_METHODS` + 插件** 合并实现为准，冲突要查代码。  
- **误会**：`agent` 和 `chat.send` 是一回事。**正解**：都是「让助理干活」，但 **参数和语义** 不同；读 `server-methods/agent.ts` 与 `server-methods/chat.ts`。
