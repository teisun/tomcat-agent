# 图片附件：粘贴、缩略图、预览与草稿的分层方案

> 适用范围：Composer 里的图片/PDF 附件从「用户粘贴」到「发给模型」到「历史里回看」的全链路，以及 transcript 里 assistant 提到的本地内联图片显示，横跨 webview / 扩展宿主 / Rust 后端三层。
> 单一事实源：协议与类型以 `tomcat/src/api/serve/types.rs`（生成 `wire.d.ts`）为准；字节存储以 `tomcat/src/core/session/attachments.rs` 为准；草稿存储以 `tomcat-vscode-ext/src/shared/composerDraft.ts` 为准。本组文档不复制定义，只解释「为什么长这样」。
> 上位文档：[`tomcat-vscode-extension.md`](tomcat-vscode-extension.md)（扩展整体三层职责）。
> 本文替代了早期版本中「草稿存 Rust + CAS + 两阶段提交 + `resvg` 栅格化」的设计。那一版为什么被推翻、依据是什么，见 [`01-placement-decision.md`](image-attachments/01-placement-decision.md) —— 它是本组文档里最该先读的一篇。

**一句话定位**：**图片字节只搬一次**（粘贴那一刻交给 Rust，之后全链路只传 32 字节的哈希），**编辑状态归编辑器**（草稿文本在扩展层落盘，打字不跨进程），**像素工作归 Chromium**（缩略图降采样、SVG 栅格化、以及 transcript 本地图显示都在 webview 做，Rust 一个图形库都不装）。

---

## 子文档索引

| 子文档 | 内容 | 何时读 |
|--------|------|--------|
| [`01-placement-decision.md`](image-attachments/01-placement-decision.md) | 草稿该放哪一层：7 个同类产品的横向调研证据、决策、以及**什么条件下该推翻它**；另附「为什么不放扩展宿主做栅格化」的排除记录 | 想改动分层、或者怀疑「这为什么不放后端」时，先读它。 |
| [`02-storage-and-gc.md`](image-attachments/02-storage-and-gc.md) | 草稿文件布局、blob store 目录布局、租约与 GC 语义、依赖体积实测数据 | 要动存储、写 GC、排查「我的图去哪了」时查它。 |
| [`03-user-guide.md`](image-attachments/03-user-guide.md) | 面向用户：怎么加图、怎么预览、限制是什么、升级注意事项 | 要写 release note 或回答用户问题时查它。 |
| [`04-inline-transcript-images.md`](image-attachments/04-inline-transcript-images.md) | transcript 里什么时候把 `![...](path)` 画成图、为什么只认工作区/临时目录、为什么提示词必须同步改 | 想改 assistant 正文里的本地图片显示，先读它。 |

测试怎么分层、四个测试位置各自跑什么，见 [`../testing-layers.md`](../testing-layers.md)。

---

## 1. 背景：这个功能为什么值得一份架构文档

功能本身听起来很小：「让用户能粘贴图片给模型看」。但它同时踩到三条边界，任何一条走错都会变成性能事故：

```text
  ① 图片是【大字节】，而聊天界面的其他一切都是【小文本】
     一张手机截图 4.5MB，一次对话的全部文字可能才 20KB。
     把图片和文字用同一条通道、同一种搬运方式处理，量级差 200 倍。

  ② 未发送的草稿是【易变状态】，已发送的历史是【不可变记录】
     草稿每按一次键都在变；历史一旦落盘就永不改动。
     两者的一致性要求差着一个数量级，用同一套机制必然一头过设计、一头欠设计。

  ③ 显示一张图需要【解码成位图】，而位图比文件大一个数量级
     4000×3000 的 JPEG 文件 4.5MB，解码成 RGBA 位图是 48MB（宽×高×4 字节）。
     所以「加载了几张图」和「占了多少内存」不是同一个问题。
```

早期版本每条都走错了一次，代价是两个可量化的缺陷 —— **写放大**与**内存放大**。

### 1.1 写放大：打一个字，搬运全部图片

```text
  【旧】草稿存在 Rust 后端，打字要同步过去

    用户按一下 'a'
      → 扩展把【整份草稿】序列化：text + segments + 全部附件的 base64
      → 11 张图 × 4.5MB × 1.33(base64 膨胀) ≈ 66MB
      → 走 NDJSON 管道发给 Rust
      → Rust 原子写整个草稿文件到磁盘
      → 回推变更事件，又是 66MB 反向穿过管道

    也就是：打一个字符 = 搬运 132MB。按住键盘打一句话 = 上百次。

  【新】草稿存在扩展层，打字不出进程

    用户按一下 'a'
      → 扩展层内存改一下
      → 防抖 400ms 后写一次本地 JSON：text + segments + 附件的【哈希列表】
      → 约 200 字节
      → 协议上零流量（Rust 完全不知道有人在打字）
```

关键不是「优化了序列化」，是**图片字节根本不在打字这条路径上**。它在粘贴那一刻就已经交给 Rust 了，此后草稿里只剩 32 字节的哈希。

### 1.2 内存放大：48px 的缩略图按原图解码

```text
  【旧】同一份字节在内存里同时存在多份，且缩略图不降采样

    Rust 草稿文件里     base64 文本        6MB
    协议传输中          base64 文本        6MB
    扩展宿主快照里      base64 文本        6MB
    webview JS 堆里     base64 字符串      6MB   ← 被 data: URI 钉住，不可淘汰
    Chromium 位图       4000×3000 RGBA    48MB   ← 附件条那个 48px 方块用的也是这份

    11 张图 ≈ 528MB。而且这 528MB 里没有一个字节是可以被系统回收的。

  【新】附件条显示小缩略图，原件经资源 URL 按需加载

    Rust blobs/<sha>                     4.5MB（磁盘，唯一权威副本）
    webview 里拿到的                     一个 ~80 字节的 URL 字符串
    Chromium 位图（附件条，192px 源）     192×144 RGBA ≈ 110KB，可被内存压力淘汰
    Chromium 位图（预览大图）             仅当前 ±1 张才解全尺寸

    11 张图的附件条 ≈ 1.2MB 量级。
```

两个改动各自解决一半：

- **`asWebviewUri` 取代 `data:` URI** —— 字节走 VS Code 的资源协议直接进 Chromium 的图片缓存，JS 堆上只留一个 URL 字符串。这条是必要条件：它同时消掉了「base64 文本的多份副本」和「位图不可淘汰」两个问题。
- **192px 缩略图** —— 让附件条那个 48px 方块不再需要解码原图。这条是优化项而非必要条件（`asWebviewUri` 之后位图已经是可淘汰的），但它把常态内存又压低一个数量级。

上面是附件条**稳态像素尺寸的量级说明，不是总进程峰值保证**。准备缩略图时仍需读取压缩字节，Chromium 也可能在内部完整解码。`imagePipeline.ts` 不再创建全尺寸 ImageBitmap 来探测尺寸，而是复用 `<img>` → 有界 canvas；缩略图长边 192px、SVG provider PNG 长边 2048px，导出后清空画布，同次粘贴顺序处理。原始附件不缩小。

---

## 2. 数据流总图

```text
 webview (Chromium)              扩展宿主 (Node)                Rust 后端
┌───────────────────────┐      ┌────────────────────┐      ┌──────────────────────┐
│ 粘贴 / 拖入 / 选择     │      │                    │      │                      │
│   DOM paste event      │      │                    │      │                      │
│   拿到 File            │      │                    │      │                      │
│        │               │      │                    │      │                      │
│  imagePipeline.ts      │      │                    │      │                      │
│  ├ <img> → 小 canvas   │      │                    │      │                      │
│  │   长边最多 192px    │      │                    │      │                      │
│  │   → 缩略图 PNG      │      │                    │      │                      │
│  └ SVG: canvas.toBlob  │      │                    │      │                      │
│      → provider PNG    │      │                    │      │                      │
│        │               │      │                    │      │                      │
│  attachFiles intent    │      │                    │      │                      │
│  （唯一带字节的一跳）  │─────▶│ attachmentIngest   │      │                      │
│                        │      │   .ts              │─────▶│ ingest_attachment    │
│                        │      │                    │      │  ├ MIME + 大小 + 魔术│
│                        │      │                    │      │  │   字节校验        │
│                        │      │                    │      │  ├ sha256 → blobs/   │
│                        │      │                    │      │  ├ thumbs/<srcSha>   │
│                        │      │                    │      │  └ pending/<sid>/    │
│                        │      │                    │      │      建租约           │
│                        │      │  { blobSha,        │◀─────│                      │
│                        │      │    hasThumb, ... } │      │                      │
│                        │      │        │           │      │                      │
│                        │      │  composerDraft.ts  │      │                      │
│                        │      │   防抖 400ms 落盘  │      │                      │
│                        │      │   （只有引用）     │      │                      │
│                        │      │        │           │      │                      │
│  <img src=…>           │◀─────│ attachmentUris.ts  │      │                      │
│  缩略图/大图直接由     │ URL  │  blobSha →         │      │                      │
│  Chromium 取字节       │ 字符串│  asWebviewUri      │      │                      │
│                        │      │                    │      │                      │
│ 打字                   │      │                    │      │                      │
│  syncComposerDraft ───▶│ ─────▶│ 只更新本地草稿     │  ✗   │ （协议零流量）        │
│                        │      │                    │      │                      │
│ assistant 文本         │      │                    │      │                      │
│  ![alt](path)          │◀─────│ mediaRoots 快照    │  ✗   │ （不进 Rust / CAS）   │
│   → 改写成 webview URL │ URL  │ workspace/tmp 授权 │      │                      │
│   → 点击放大           │      │                    │      │                      │
│                        │      │                    │      │                      │
│ 发送                   │      │                    │      │                      │
│  sendUserMessage ─────▶│ ─────▶│ prompt {           │─────▶│ prompt               │
│                        │      │   attachments:[    │      │  ├ 按 sha 取已校验字节│
│                        │      │     {blobSha}]}    │      │  ├ 落 transcript      │
│                        │      │                    │      │  └ promote: 删租约    │
│                        │      │                    │      │      字节原地不动      │
│                        │      │  收到 ack 才清草稿 │◀─────│      （零拷贝）        │
└───────────────────────┘      └────────────────────┘      └──────────────────────┘
```

**这张图最该看清的三件事：**

1. **带字节的箭头只有一根** —— `attachFiles` → `ingest_attachment`。全协议其他任何命令都只传哈希，这条由一个静态 schema 扫描测试守着（见 [`../testing-layers.md`](../testing-layers.md) 的契约测试一节）。
2. **打字那一行的箭头断在扩展宿主** —— 打字不产生任何 Rust 流量，这是写放大被结构性消除的地方。
3. **transcript 内联图这条支线只改 URL** —— 它吃的是“模型输出的本地路径”，不是“用户新交进来的字节”，所以不会多出一条 copy-to-CAS 管道。
4. **发送不搬字节** —— `promote` 只删掉一个空的租约标记文件，图片字节从 ingest 落盘那一刻起就没再动过。

---

## 3. 三层职责边界

```text
┌──────────────────────────────────────────────────────────────────────────┐
│ webview（Chromium）—— 所有像素工作                                        │
│   为什么在这：复用浏览器现成图像栈，不把图形依赖带进 Rust / Node。        │
│   · <img> + 有界 canvas → 192px 缩略图，导出后释放画布像素               │
│   · SVG 同一路径生成长边 ≤2048px 的 provider PNG；原件保持不变           │
│   · 同次粘贴顺序处理；不承诺浏览器内部不会完整解码                       │
│   · 失败降级：SVG 源码当文本发给模型（≤50KB），显示仍走原生 <img>          │
│   · transcript: `![alt](path)` → 检查授权范围 → 改写成 webview URL         │
│   文件：gui/src/attachments/imagePipeline.ts                              │
└──────────────────────────────────────────────────────────────────────────┘
┌──────────────────────────────────────────────────────────────────────────┐
│ 扩展宿主（Node）—— 编辑状态 + 路径映射，自身不持有任何图片字节             │
│   · 草稿：text / segments / 附件引用，防抖原子落盘（composerDraft.ts）     │
│   · ingest 客户端：把字节交给 Rust，换回哈希（attachmentIngest.ts）        │
│   · 路径映射：blobSha → asWebviewUri（attachmentUris.ts）                 │
│   · localResourceRoots 授权 blobs/、thumbs/、workspace roots、tmp         │
└──────────────────────────────────────────────────────────────────────────┘
┌──────────────────────────────────────────────────────────────────────────┐
│ Rust 后端 —— 纯字节仓库，零图形库                                         │
│   · 存字节（内容寻址）、按哈希取字节、校验大小/MIME/魔术字节              │
│   · 租约与 GC、transcript 落盘与引用判定                                  │
│   · 明确【不做】：不解码图片、不解析 SVG、不渲染任何东西                   │
│   文件：tomcat/src/core/session/attachments.rs                            │
└──────────────────────────────────────────────────────────────────────────┘
```

### 3.1 为什么 Rust 侧一个图形库都不装

早期版本为了「把 SVG 转成 PNG 发给模型」（OpenAI / Anthropic 的视觉接口不认 SVG）引入了 `resvg`。实测代价：

| 指标 | 数据 | 说明 |
|---|---|---|
| 新增 crate | **44 个** | 含整套字体栈（`fontdb` / `rustybuzz` / `ttf-parser` / `fontconfig-parser` / `unicode-bidi` 等）与一组图片编解码（`png` / `gif` / `zune-jpeg` / `image-webp`） |
| 冷编译（debug，仅依赖） | **83 秒** | |
| 冷编译（release + LTO） | **4 分 50 秒** | |
| stripped release 产物 | **+2.16 MiB** | 2,579,884 字节 vs 空程序基线 312,652 字节 |

这些全部会装进**核心 CLI**，而不只是扩展。而 SVG 栅格化只有 webview 这一条路径会触发 —— CLI 至今没有任何图片附件入口。

第一性原理上的问题是：**谁手上已经有一个世界级的 SVG 渲染器？** 答案是 Chromium，它就在 webview 里。把这件事搬过去之后：

- 44 个 crate 与 2.16MiB 产物全部不需要
- 「怎么安全地解析 SVG」这个问题整体消失 —— Rust 不再解析 SVG。SVG 只被 Chromium 以 `<img>` 加载，而这条路在规范层面处于 **secure static mode**：强制不执行脚本、不加载外部资源。这是规范级保证，取代了早期版本自己写的小写文本黑名单（那个黑名单既误杀设计工具导出的正常 SVG，又挡不住 `x:href` 命名空间别名）
- 应用显式分配的派生画布有尺寸上限，且不再额外创建完整 ImageBitmap 探测尺寸；浏览器内部解码/缓存的峰值仍须实测，不能由 API 参数推断

「为什么不搬到扩展宿主用 Node 原生模块」这个方向已被明确排除，理由见 [`01-placement-decision.md` §3](image-attachments/01-placement-decision.md#3-已排除扩展宿主的-node-原生模块)。

### 3.2 两个存储边界

| 维度 | 未发送草稿 | 已发送历史 | 图片字节 |
|------|-----------|-----------|---------|
| **归属层** | 扩展宿主 | Rust | Rust |
| **位置** | `<storageUri>/composer-drafts/<sessionId>.json` | `sessions/<id>.jsonl` | `sessions/attachments/blobs/<sha256>` |
| **可变性** | 每次按键都变 | 只追加，不可变 | 不可变（内容寻址） |
| **丢失后果** | 用户重打一遍 | 数据损坏 | 图片失效（降级为可移除的失效附件） |
| **一致性机制** | 防抖 + 原子写 + last-write-wins | 追加写 | sha 自校验 |
| **生命周期** | 收到 prompt ack 后清理 | 随 session 删除 | 租约 TTL + transcript 引用判定 |

三者的一致性机制强度是**按丢失后果**配的，这是早期版本最主要的设计偏差 —— 它给「丢了重打一遍」的草稿上了一套跨进程两阶段提交。

---

## 4. 安全模型

```text
  ① webview 侧：MIME 白名单 + 大小上限（拦明显错误，快速反馈）
  ② Rust ingest：MIME 白名单 + 大小上限 + 魔术字节头部校验（零依赖，挡「声明是 PNG 实际是别的」）
  ③ 显示 SVG：Chromium <img> 的 secure static mode —— 规范强制不执行脚本、不取外部资源
  ④ 取字节：sha 必须是 64 位小写十六进制 —— 合法 sha 里不可能有 '/' 或 '..'，
     所以拼路径不需要信任调用方；session id 同理只允许单层目录名字符集
  ⑤ 内容自校验：读 blob 时重算哈希，与文件名不符则隔离为 .corrupt-* 并当作不存在
  ⑥ 伪造哈希：prompt 带一个未经 ingest 的 blobSha 会被拒绝 —— 客户端无法绕过 ② 的校验
  ⑦ CSP：两个 webview 的 img-src 均为 ${webview.cspSource} blob:，chat 侧已去掉 data:
  ⑧ transcript 本地图：只接受 workspace/tmp 内的普通文件路径；http/https/data/blob 一律降级
```

**刻意没有的东西：**

- **没有「取不到资源就回退 base64」的降级。** `localResourceRoots` 配错时图片就是裂开，这是故意的 —— 静默回退 `data:` URI 会让内存问题悄悄复活，在小规模手测里毫无症状，然后变成某个粘了 11 张图的用户的 OOM 报告。配置错误必须直接暴露。
- **Rust 不校验「这真的是一张合法 PNG 吗」。** 它从不解析这些字节，只是存起来转发给 provider，所以没有解码器攻击面。字节是垃圾的话 provider 会拒绝，错误可归因。

---

## 5. 关键文件地图

```text
  Rust
    core/session/attachments.rs          blob store：存取、租约、GC、校验（零图形库）
    api/serve/types.rs                   ServeAttachment（只有引用）、IngestAttachment*、AttachmentMode
    api/serve/commands.rs                ingest_attachment / cache_attachment_thumbnail 处理

  扩展宿主
    src/shared/composerDraft.ts          草稿存储：防抖、原子写、损坏隔离、session id 断言
    src/shared/attachmentIngest.ts       ingest 客户端（全链路唯一发送字节的地方）
    src/shared/attachmentUris.ts         blobSha → asWebviewUri，含 localResourceRoots 计算
    src/shared/attachmentProtocol.ts     附件候选校验与共享类型（图片/PDF）
    src/ui/webview/provider.ts           chat webview 宿主：草稿生命周期、附件视图模型、mediaRoots 授权
    src/ui/imagePreview/ImagePreviewPanel.ts 预览面板宿主：单实例复用、另存为

  webview
    gui/src/attachments/imagePipeline.ts 缩略图降采样、SVG 栅格化、降级链
    gui/src/components/AttachmentStrip.tsx 附件条（只用 thumbUri，懒加载，失效态）
    gui/src/components/markdown/localImages.ts transcript 本地图片路径解析与 DOM 改写
    gui/src/components/ImageLightbox.tsx transcript 轻量放大图
    gui/src/imagePreview/PreviewPanel.tsx 预览（大图只加载当前 ±1 张，filmstrip 用 thumbUri）
```

---

## 6. 一句话总结

图片附件的全部复杂度来自一个事实：**它是这个界面里唯一的大字节**。所以方案的每一条都在回答「怎么让这些字节少走一步」—— 粘贴时交一次给 Rust（内容寻址、天然去重），之后协议上只传哈希（写放大消失），显示时用 `asWebviewUri` 避免用常驻 base64 和全尺寸原图渲染附件条，像素工作交给本来就在那儿的 Chromium（44 个 crate 与整套 SVG 安全问题一起消失）。草稿则回到它本来该在的地方 —— 编辑器旁边，因为它是编辑状态，而且它跟着窗口走比跟着后端进程走更符合用户预期。
