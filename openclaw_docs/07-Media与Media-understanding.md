# Media 与 Media-understanding

**设计思想**：媒体管道负责大小限制、临时文件、HEIC 转 JPEG 等；media-understanding 在入站消息进入 Agent 上下文前，对 image/audio/video 附件应用理解能力（描述、转录等），结果注入用户文本或消息体。

---

## 一、职责概览

- **Media**：`openclaw/src/media/` 解析、存储、获取媒体；`openclaw/src/web/media.ts` 的 `loadWebMedia` 处理 file:// 与远程 URL，HEIC 转 JPEG、resize。
- **Media-understanding**：`openclaw/src/media-understanding/apply.ts` 的 `applyMediaUnderstanding`，按 image→audio→video 顺序执行能力，结果合并到 ctx。

---

## 二、关键实现

- **applyMediaUnderstanding**：`openclaw/src/media-understanding/apply.ts`，接收 MsgContext、ClawdbotConfig，调用 `runCapability` 处理各类型。
- **normalizeMediaAttachments**：从 ctx 提取附件。
- **runCapability**：`openclaw/src/media-understanding/runner.ts`，根据 provider 执行理解。
- **formatMediaUnderstandingBody**：将理解结果格式化为可注入 Agent 的文本。

---

## 三、与上下游衔接

| 消费者 | 使用方式 |
|--------|----------|
| auto-reply | 入站消息进入 Agent 前调用 applyMediaUnderstanding |
| web/media | loadWebMedia 供 Control UI 或 WebChat 展示媒体 |
