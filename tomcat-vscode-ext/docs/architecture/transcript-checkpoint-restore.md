# Tomcat Transcript Checkpoint：恢复点协议与前后端协作

> 适用范围：`tomcat-vscode-ext` transcript 中的 checkpoint 分隔条、恢复确认框、`restore checkpoint` 前后端协作链路。
> 一句话定位：**checkpoint 不是前端自己“画出来的一条线”，而是后端拥有的会话边界，前端只是把它投影成可点击的恢复入口。**
> 单一事实源：
> 1. Rust restore 真相以 [`../../../tomcat/src/api/chat/commands/cmd_restore.rs`](../../../tomcat/src/api/chat/commands/cmd_restore.rs) 与 [`../../../tomcat/src/api/serve/commands.rs`](../../../tomcat/src/api/serve/commands.rs) 为准；
> 2. serve 协议入口以 [`../../../tomcat/src/api/serve/types.rs`](../../../tomcat/src/api/serve/types.rs) 与 [`../../src/serveClient/wire.d.ts`](../../src/serveClient/wire.d.ts) 为准；
> 3. extension host 的刷新时机与状态收敛以 [`../../src/ui/webview/provider.ts`](../../src/ui/webview/provider.ts)、[`../../src/serveClient/sessionRouter.ts`](../../src/serveClient/sessionRouter.ts)、[`../../src/ui/webview/state.ts`](../../src/ui/webview/state.ts) 为准；
> 4. GUI 交互以 [`../../gui/src/App.tsx`](../../gui/src/App.tsx)、[`../../gui/src/components/TranscriptView.tsx`](../../gui/src/components/TranscriptView.tsx)、[`../../gui/src/components/CheckpointMarker.tsx`](../../gui/src/components/CheckpointMarker.tsx)、[`../../gui/src/components/RestoreConfirmDialog.tsx`](../../gui/src/components/RestoreConfirmDialog.tsx) 为准。
> 关联文档：[`webview-transcript-stable-id-upsert.md`](./webview-transcript-stable-id-upsert.md)、[`tomcat-vscode-extension-phase2/05-webview-ui-architecture.md`](./tomcat-vscode-extension-phase2/05-webview-ui-architecture.md)。

---

## 1. 先说人话

checkpoint 真正解决的不是“在 transcript 中插一条分隔线”，而是：

```text
把一次会话切成若干“可恢复边界”

边界之前   = 保留
边界之后   = 可撤销
文件是否回滚 = 由 restore 时的 revertFiles 决定
```

用户看到的是：

```text
assistant: 已完成第 2 轮
──────────── Restore Checkpoint ────────────
user: 第 3 轮问题
assistant: 第 3 轮回答
```

系统内部真实在做的是：

```text
checkpointId   = 这是哪个边界
messageAnchor  = 这个边界挂在哪条消息后面
revertFiles    = 恢复时是否回滚磁盘文件
superseded     = 这个边界之后那段旧对话，应该被隐藏
```

一句话记忆：

**后端记录边界，前端显示边界；后端做 restore，前端重建视图。**

---

## 2. 三个核心对象

### 2.1 `checkpointId`

```text
它回答的问题：
  “你要恢复到哪个 checkpoint？”
```

它像“书签编号”。

### 2.2 `messageAnchor`

```text
它回答的问题：
  “这个 checkpoint 在 transcript 里挂在哪？”
```

它像“书签夹在第几页后面”。

### 2.3 `revertFiles`

```text
true   = 回滚文件 + 截断后续对话
false  = 保留当前文件，只截断后续对话
```

所以 restore 实际上有两种语义：

```text
Revert
  = 文件回到旧状态
  + 对话回到旧边界

Don't revert
  = 文件保持现在
  + 对话回到旧边界
```

---

## 3. 总体分层图

```text
┌────────────────────────────────────────────────────────────────────┐
│ React Webview                                                     │
│ App / TranscriptView / CheckpointMarker / RestoreConfirmDialog    │
│                                                                    │
│ 职责：                                                             │
│ 1. 显示 checkpoint 分隔条                                          │
│ 2. 弹确认框                                                        │
│ 3. 发 restore intent                                               │
│ 4. restore 完成后回填 composer                                     │
└──────────────────────▲──────────────────────────────┬──────────────┘
                       │ state/event                   │ intent
                       │ postMessage                   │ postMessage
┌──────────────────────┴──────────────────────────────▼──────────────┐
│ VS Code Extension Host                                             │
│ provider / sessionRouter / state                                   │
│                                                                    │
│ 职责：                                                             │
│ 1. 把 GUI intent 翻译成 serve 命令                                 │
│ 2. 拉 checkpoints / history                                        │
│ 3. 把 checkpoint 元数据注入 timeline                               │
│ 4. 过滤 superseded 历史                                             │
└──────────────────────▲──────────────────────────────┬──────────────┘
                       │ response / event              │ request
                       │ NDJSON                        │ NDJSON
┌──────────────────────┴──────────────────────────────▼──────────────┐
│ Rust `tomcat serve`                                                │
│ ServeCommand / restore_core / checkpoint_store / transcript        │
│                                                                    │
│ 职责：                                                             │
│ 1. 每轮结束自动记录 checkpoint                                     │
│ 2. 暴露 list_checkpoints / restore_checkpoint                      │
│ 3. 真正回滚文件或截断对话                                           │
│ 4. 把 checkpoint 之后的消息标成 superseded                         │
└────────────────────────────────────────────────────────────────────┘
```

这一层拆法的重点是：

```text
后端 = 真相层
host = 翻译层
前端 = 交互层
```

---

## 4. 协议图

### 4.1 Webview 内部协议

```text
GUI 发给 host 的 intent（camelCase）

listCheckpoints
  └─ { sessionId }

restoreCheckpoint
  └─ { sessionId, checkpointId, revertFiles }
```

### 4.2 Host 到 serve 的线协议

```text
host 发给 tomcat serve 的命令（snake_case）

list_checkpoints
  └─ { sessionId }

restore_checkpoint
  └─ { sessionId, checkpointId, revertFiles, dryRun? }
```

### 4.3 后端返回的两类 payload

```text
checkpoint 列表项
  = id
  + kind
  + createdAt
  + messageAnchor
  + changedFiles

restore 结果
  = checkpointId
  + messageAnchor
  + revertFiles
  + changedPaths
  + restoredPaths
  + transcriptTruncated
  + warnings
```

一张图看懂协议翻译：

```text
GUI intent
  listCheckpoints / restoreCheckpoint
           │
           ▼
provider.ts / sessionRouter.ts
  翻译字段风格 + 调 messenger
           │
           ▼
ServeCommand
  list_checkpoints / restore_checkpoint
           │
           ▼
ResponseFrame.payload
           │
           ▼
state.ts / App.tsx 消费
```

---

## 5. checkpoint 是怎么“长出来”的

```text
一轮对话结束(turn_end)
        │
        ▼
Rust 构造 checkpoint record request
  messageAnchor = 本轮最后一条消息 id
        │
        ▼
checkpoint_store.record(...)
        │
        ▼
provider.ts 收到 turn_end
        │
        ▼
refreshCheckpoints(sessionId)
        │
        ▼
sessionRouter.listCheckpoints()
        │
        ▼
serve: list_checkpoints
        │
        ▼
返回 checkpoints[]
        │
        ▼
state.ts.setCheckpoints()
        │
        ▼
injectCheckpointMarkers()
        │
        ▼
TranscriptView 在“下一条 user 消息之前”画出 marker
```

这里最关键的一点：

```text
checkpoint 本体 ≠ transcript 原始消息

checkpoint 本体存在于后端元数据里
marker 是前端根据 checkpoint 元数据“注入出来”的 timeline 项
```

所以分隔条不是落盘文本，而是一个 UI 投影。

补两个实现细节：

```text
1. checkpoint.messageAnchor 指向的是后端 assistant message id
2. 但前端 timeline 不一定总有同名 message 节点
   - 若 assistant 没有正文、只有 tool_calls / thinking
   - timeline 里可能只有 `${messageAnchor}-thinking`
3. injectCheckpointMarkers() 会先找 messageAnchor
   找不到再回退 `${messageAnchor}-thinking`
```

---

## 6. restore 流程图

### 6.1 交互链路

```text
用户点击 Restore Checkpoint
        │
        ▼
App.tsx
  先找到 marker 后第一条 user 消息
  暂存它的 prompt + references
        │
        ▼
打开 RestoreConfirmDialog
        │
        ├─ Cancel
        │    └─ 结束，无副作用
        │
        ├─ Don't revert
        │    └─ restoreCheckpoint { revertFiles:false }
        │
        └─ Revert
             └─ restoreCheckpoint { revertFiles:true }
```

### 6.2 后端执行链路

```text
provider.ts
  └─ sessionRouter.restoreCheckpoint(...)
              │
              ▼
serve: restore_checkpoint
              │
              ▼
restore_core(checkpointId, revertFiles)
              │
              ├─ revertFiles = true
              │    ├─ checkpoint_store.restore(...)
              │    └─ finalize_restore_transcript(...)
              │
              └─ revertFiles = false
                   ├─ 跳过路径收窄 / 跨会话冲突扫描
                   ├─ changedPaths 直接取 checkpoint.notes.changedPaths
                   └─ finalize_restore_transcript(...)
```

### 6.3 transcript 截断的真正发生点

```text
finalize_restore_transcript(...)
  1. 取出 checkpoint.messageAnchor
  2. mark_messages_after_anchor_superseded(anchor)
  3. append custom entry: checkpoint.restore
  4. 更新 last_checkpoint_id
```

这意味着 restore 后，前端不是“手动删掉几条消息”，而是：

```text
后端把旧尾巴标成 superseded
前端重新拉 history
前端重建 timeline 时把 superseded 过滤掉
```

---

## 7. restore 后前端为什么能“看起来像原地截断”

```text
restore 成功
   │
   ▼
provider.ts
  refreshSessionState()
  refreshSessionHistory()
  refreshCheckpoints()
  postState()
   │
   ▼
state.ts
  filterSupersededHistoryEntries()
  injectCheckpointMarkers()
   │
   ▼
React 重渲染 transcript
  旧尾巴消失
  新边界保留
```

这就是“截断感”的来源：

```text
不是前端临时 patch 一下 DOM
而是后端改真相，前端按新真相重建视图
```

补充：

```text
正常情况
  superseded span 由 checkpoint.restore 哨兵闭合

异常情况（旧数据 / 损坏数据丢了哨兵）
  filterSupersededHistoryEntries()
  会在“下一条非 superseded 的 user message”处兜底闭合

目的
  不让后续正常 turn 被整段一起吞掉
```

---

## 8. composer 回填为什么必须在 restore 之前预取

```text
marker 后第一条 user 消息
    = 用户真正想“回到并继续编辑”的那条 prompt
```

但一旦 restore 成功：

```text
这条消息就会进入 superseded 区
前端下一轮重建时会把它隐藏
```

所以顺序必须是：

```text
点 marker
  └─ 先抓旧 prompt + references

确认 restore
  └─ 再发 restore 请求

新 state 到达
  └─ 如果原消息已消失，则把旧 draft 回填 composer
```

一张小图：

```text
旧 user 消息还在时      restore 后

[user prompt X]         [user prompt X] 被 superseded 隐藏
      │                           │
      └─ 先缓存 draft             └─ 再回填 composer
```

---

## 9. 为什么这套设计是优雅的

### 9.1 后端拥有事实，前端只拥有投影

```text
后端决定：
  checkpoint 是否存在
  挂在哪
  是否回滚文件
  哪些消息作废

前端决定：
  如何把它画成分隔条
  如何弹确认框
  如何回填输入框
```

这样避免了两套 restore 逻辑。

### 9.2 marker 与 transcript 消息分离

```text
transcript 原始数据
  保持干净：message / tool / custom

checkpoint marker
  作为 timeline 合成项注入
```

这样 UI 可以自由调整 marker 位置，而不污染底层 transcript 形态。

### 9.3 `Revert` / `Don't revert` 复用同一个 restore 核心

```text
同一个 checkpoint
同一个 restore_core
只靠 revertFiles 分两条语义
```

这比写两套命令更稳，也让 CLI / serve / GUI 共用同一份恢复真相。

---

## 10. 边界与非目标

```text
首轮之前
  = 没有 checkpoint marker

Redo
  = 本方案明确不做

无 checkpoint store / 无 git 支撑
  = list_checkpoints 为空
  = transcript 中不出现 marker

messageAnchor
  = 消息 id，不是行号，不是文本片段
```

最后一句总结：

**checkpoint 的本质不是“UI 上的恢复按钮”，而是“后端定义的可回退会话边界”，UI 只是把这个边界做成了用户可理解、可操作的界面。**
