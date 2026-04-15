# Media 与 Media-understanding

## 零、先用大白话

媒体管线像 **安检 + 翻译**。  
先量体重、看格式（太大就拒或压缩）。  
再决定要不要把图/音/视频 **变成文字说明**，让「只认字」的模型也 **看得见附件里大概有啥**。

**这一节你会学到**：`src/media` 和 `src/media-understanding` 谁管哪段。

---

**设计思想**：媒体管道负责大小限制、临时文件、HEIC 转 JPEG 等；media-understanding 在入站消息进入 Agent 上下文前，对 image/audio/video 附件应用理解能力（描述、转录等），结果注入用户文本或消息体。

---

## ASCII 核心四图

### 1) 结构图

```text
入站附件（二进制）
        |
        v
src/media/*（校验/转码/落盘）
        |
        v
media-understanding（多提供商）
        |
        v
文本描述注入消息 -> Agent
```

### 2) 调用流图

```text
探测 MIME / 大小
  -> 拒绝或落临时文件
      -> 选用理解 provider
          -> 得到 caption / transcript
              -> 合并进 user visible text
```

### 3) 时序图

```text
Channel      media pipeline      understand svc      Agent
  |              |                    |               |
  | attachment   |                    |               |
  |------------->| resize/transcode   |               |
  |              |------------------->| infer        |
  |              | caption            |               |
  |              |----------------------------------->|
```

### 4) 数据闭环图

```text
大文件策略（丢弃/外链）
        |
        v
理解结果缓存路径引用
        |
        v
模型使用描述而非原始二进制
        |
        v
策略调整 -> 下一消息走新阈值
```

---

## 一、职责概览

- **Media**：**`src/media/`** 解析、存储、获取媒体；**`src/media/web-media.ts`** 的 **`loadWebMedia`** 处理 file:// 与远程 URL，HEIC 转 JPEG、resize 等。
- **Media-understanding**：`src/media-understanding/apply.ts` 的 `applyMediaUnderstanding`，按 image→audio→video 顺序执行能力，结果合并到 ctx。

---

## 二、关键实现

- **applyMediaUnderstanding**：`src/media-understanding/apply.ts`，接收 `MsgContext`、**`OpenClawConfig`**（见 `applyMediaUnderstanding` 的 `cfg` 参数），调用 `runCapability` 处理各类型。
- **normalizeMediaAttachments**：从 ctx 提取附件。
- **runCapability**：`src/media-understanding/runner.ts`，根据 provider 执行理解。
- **formatMediaUnderstandingBody**：将理解结果格式化为可注入 Agent 的文本。

---

## 三、与上下游衔接

| 消费者 | 使用方式 |
|--------|----------|
| auto-reply | 入站消息进入 Agent 前调用 applyMediaUnderstanding |
| web/media | loadWebMedia 供 Control UI 或 WebChat 展示媒体 |

---

## 常见误会

- **误会**：大文件会完整塞进 prompt。**正解**：常变成 **描述/外链**；二进制乱灌已被多次修（见上游 CHANGELOG）。  
- **误会**：理解结果 100% 准。**正解**：多提供商、多语言；要当 **提示** 不当 **司法证据**。  
- **误会**：关掉 media-understanding 就零流量。**正解**：仍可能下载缩略图做展示；看各子配置。
