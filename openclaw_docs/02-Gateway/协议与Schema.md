# Gateway 协议与 Schema

## 零、先用大白话

协议像 **对讲机口令手册**。  
大家都说同一种 JSON 句式：**请求**里写 `method`；**回答**里带回同一个 `id`。  
这样 CLI、网页、手机不用各编一套黑话。

**这一节你会学到**：契约文件在哪；改了以后要跑哪个脚本；Swift 那边从哪生成。

---

**设计思想**：Gateway 与客户端通过 WebSocket 交换 JSON，采用类 JSON-RPC 的请求/响应与事件推送。契约定义在 **`src/gateway/protocol/`**，并由仓库根 **`scripts/protocol-gen.ts`**、**`scripts/protocol-gen-swift.ts`** 生成校验产物与 Swift 类型。

---

## ASCII 核心四图

### 1) 结构图

```text
src/gateway/protocol/（手写/生成的契约）
        |
        v
scripts/protocol-gen*.ts
        |
        +--> TS 校验产物 / 文档
        +--> Swift OpenClawProtocol 等
```

### 2) 调用流图

```text
改协议定义
  -> 跑 protocol-gen
      -> 更新 schema 与类型
          -> 客户端与 Gateway 同版本对齐
              -> WS 帧按统一字段序列化/反序列化
```

### 3) 时序图

```text
维护者      protocol 目录        protocol-gen        客户端仓库
  |              |                    |                  |
  | 改 JSON 契约  |                    |                  |
  |------------->|                    |                  |
  |              | 生成产物          |                  |
  |              |------------------>|                  |
  |              |                    |----------------->|
```

### 4) 数据闭环图

```text
契约（source of truth）
        |
        v
生成代码 + CI 校验
        |
        v
运行时 WS 请求必须符合 schema
        |
        v
不合规 -> 修契约或修调用方 -> 再生成
```

---

## 一、协议目录结构（与源码一致）

```text
src/gateway/protocol/
  index.ts
  schema.ts
  schema/
    types.ts
    frames.ts
    protocol-schemas.ts
    sessions.ts
    config.ts
    agents-models-skills.ts
    nodes.ts
    cron.ts
    channels.ts
    ...
  client-info.ts
  ...
```

---

## 二、协议帧

- **请求**：`{ method, params, id? }`。
- **响应**：`{ id, result? }` 或 `{ id, error }`。
- **事件**：`{ event, data }`，无 `id`。

帧与聚合 schema：见 **`schema/frames.ts`**、**`schema/protocol-schemas.ts`**。

---

## 三、Schema 主题（节选）

- **sessions**：会话条目、`sessions.patch` 等。
- **config**：`config.get`、`config.set`、`config.patch` 等 payload。
- **agents**：`agents.list` 等与 Agent 相关的参数。
- **nodes**：`node.list`、`node.invoke` 等。
- **cron**：定时任务相关。

定义主要使用 **TypeBox**（`@sinclair/typebox`）；生成物见 **`pnpm protocol:gen`** 产出的 **`dist/protocol.schema.json`**。

---

## 四、protocol-gen 脚本

- **`scripts/protocol-gen.ts`**：根据 schema 生成 **`dist/protocol.schema.json`**。
- **`scripts/protocol-gen-swift.ts`**：生成 Swift 侧的 **`GatewayModels.swift`**，输出路径包括（以 `package.json` 的 `protocol:check` 为准）：
  - `apps/macos/Sources/OpenClawProtocol/GatewayModels.swift`
  - `apps/shared/OpenClawKit/Sources/OpenClawProtocol/GatewayModels.swift`
- **校验**：`pnpm protocol:check`（gen + `git diff` 守卫生成物与仓库一致）。

---

## 延伸阅读

- [02-Gateway.md](../02-Gateway.md)  
- 上游 `docs/gateway/protocol.md`（若与实现有出入，以 **`src/gateway/protocol/`** 与 `protocol:check` 为准）

---

## 常见误会

- **误会**：我可以随便在 WS 里塞 JSON 字段，反正会忽略。**正解**：帧会按 **schema** 校验；多了少了都可能被拒。  
- **误会**：只改 TypeScript 不用管 `protocol-gen`。**正解**：契约源在 `protocol/`；改了要 **生成 + `protocol:check`**，否则 CI 会红。  
- **误会**：Swift 的 `GatewayModels.swift` 手写就行。**正解**：那是 **生成物**；改手写会在下次 gen 被覆盖。
