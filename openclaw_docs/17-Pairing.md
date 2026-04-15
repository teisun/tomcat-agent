# Pairing

## 零、先用大白话

Pairing 像 **陌生人按门铃要先对暗号**。  
对上了，名字写进 **allowlist**（能进小区）。  
对不上，要么只看到提示，要么消息干脆不进模型。

**这一节你会学到**：配对状态存哪；和 outbound 怎么配合。

---

**设计思想**：Pairing 管理设备配对码与 allowlist，控制哪些设备/用户可访问；与 Channels 的 outbound、deliveryMode 配合，决定消息投递目标。

---

## ASCII 核心四图

### 1) 结构图

```text
未知 DM / 新设备
        |
        v
配对码生成与展示
        |
        v
allowlist 持久化（~/.openclaw/…）
        |
        v
Outbound 仅向已批准目标投递
```

### 2) 调用流图

```text
入站 peer 未识别
  -> 返回配对挑战 / 丢弃内容
      -> 用户提交码
          -> 校验 -> 写入 allowlist
              -> 后续消息正常路由
```

### 3) 时序图

```text
Stranger     Channel        Pairing service      User owner
    |            |                 |                |
    | DM         |                 |                |
    |----------->| 未知             |                |
    |<-----------| 配对码提示      |                |
    |            |                 | 用户确认码    |
    |            |                 |<---------------|
```

### 4) 数据闭环图

```text
配对状态文件
        |
        v
与 sessions 路由联合生效
        |
        v
撤销配对 -> 旧 peer 再入挑战流程
        |
        v
安全审计后更新策略（open/open+allowlist）
```

---

## 一、职责

- **配对码**：生成、验证配对码。
- **allowlist**：存储允许的发送者/设备列表。
- **与 Outbound**：`src/infra/outbound/` 的 targets、deliver 使用 pairing 信息。  
- **与 Channels**：ChannelPlugin 的 pairing 适配器。

---

## 二、关键路径

- **`src/pairing/`**：配对逻辑与存储。  
- **allowlist**：白名单读写（具体文件名以仓库为准）。  
- **deliveryMode**：direct / gateway / hybrid 与「谁能收信」组合出不同形态。

---

## 常见误会

- **误会**：配对只拦坏人。**正解**：也拦 **误配 token、脚本扫你端口** 一类噪声。  
- **误会**：`dmPolicy: open` 不用配对。**正解**：仍可能要 **allowlist**；读渠道安全文档。  
- **误会**：配对一次全渠道通用。**正解**：常按 **channel + account** 维度分桶。
