# Hooks

## 零、先用大白话

Hooks 像 **事件铃铛**。  
会话新建了、命令跑完了——**叮**。  
铃铛后面可以挂 **内置小脚本** 或 **插件**，不用改核心一大坨 if。

**这一节你会学到**：`triggerInternalHook` 从哪来；几个 bundled 例子。

---

**设计思想**：Hooks 为事件驱动的扩展点，事件类型与注册在 internal-hooks；bundled 提供 session-memory、llm-slug、command-logger、boot-md、soul-evil 等；hooks CLI 管理安装与状态。

---

## ASCII 核心四图

### 1) 结构图

```text
核心事件（会话创建、命令完成…）
        |
        v
internal-hooks（注册表）
        |
        v
bundled / 用户 hooks 处理器
```

### 2) 调用流图

```text
业务点 triggerInternalHook(type, payload)
  -> 查找 handlers[]
      -> 顺序或并行执行
          -> 吞错策略（不拖垮主流程）
```

### 3) 时序图

```text
Core flow      hook bus         session-memory      Disk memory
     |             |                  |                |
     | /new        |                  |                |
     |------------>| fire             |                |
     |             |----------------->| 归档会话      |
     |             |                  |--------------->|
```

### 4) 数据闭环图

```text
事件触发副作用（写文件、打点）
        |
        v
hooks CLI 展示安装状态
        |
        v
运维调整启用/禁用
        |
        v
下一次同类事件走新配置
```

---

## 一、事件与注册

- **internal-hooks.ts**：`registerInternalHook`、`triggerInternalHook`、`createInternalHookEvent`。
- **HookEventType**：事件类型枚举。
- **HookHandler**：处理函数签名。

---

## 二、Bundled Hooks

- **session-memory**：/new 时保存会话到 memory。
- **llm-slug**：LLM slug 生成。
- **command-logger**：命令日志。
- **boot-md**：启动 Markdown。
- **soul-evil**：特定业务逻辑。

---

## 三、Hooks CLI

- **hooks-cli**：`src/cli/hooks-cli.ts`（由 `register.subclis` 挂上），安装、列表、状态。  
- **gmail**：Gmail 相关 hooks，与 webhooks 配合。

---

## 常见误会

- **误会**：hook 抛错会崩整条聊天。**正解**：多数路径 **吞错或降级**；仍要看日志别静默坏数据。  
- **误会**：hooks 和 Cron 一样定时。**正解**：hooks 是 **事件驱动**；Cron 是 **时间驱动**。  
- **误会**：关掉 `hooks.internal` 就绝对零副作用。**正解**：还有 **别的 internal 事件路径**；读配置全名。
