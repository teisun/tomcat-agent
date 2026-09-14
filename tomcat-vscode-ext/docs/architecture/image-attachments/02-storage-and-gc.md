# 02 · 存储布局、租约与 GC、依赖体积实测

> 上位文档：[`../image-attachments.md`](../image-attachments.md)
> 为什么是这个分层，见 [`01-placement-decision.md`](01-placement-decision.md)。
> 实现：`tomcat/src/core/session/attachments.rs`（字节）、`tomcat-vscode-ext/src/shared/composerDraft.ts`（草稿）、`tomcat-vscode-ext/src/ui/webview/provider.ts`（webview 资源授权）。

**这一页回答三个运维向的问题**：我的东西存在哪、什么时候会被删、删掉 `resvg` 到底省了多少。

---

## 1. 全景：三份东西，三种生命周期

```text
  ┌─ 扩展层 ────────────────────────────────────────────────────────────┐
  │  <storageUri>/composer-drafts/<sessionId>.json                       │
  │    未发送的文字、@ 引用、附件【引用】                                │
  │    ~200 字节/份 · 每次按键都变 · 丢了重打一遍                        │
  │    storageUri 为空（没打开文件夹）时退回 globalStorageUri            │
  └──────────────────────────────────────────────────────────────────────┘
                      │ blobSha（32 字节哈希）指向 ↓
  ┌─ Rust ──────────────────────────────────────────────────────────────┐
  │  ~/.tomcat/…/sessions/attachments/blobs/<sha256>                     │
  │    图片字节本身 · ~4MB/张 · 不可变 · 内容寻址                        │
  │                        thumbs/<源sha256>                             │
  │    192px 缩略图 · 纯派生，可随时重建                                 │
  │                        pending/<sessionId>/<sha256>                  │
  │    租约标记（空文件，mtime 即租约时间）                              │
  │                                                                      │
  │  ~/.tomcat/…/sessions/<sessionId>.jsonl                              │
  │    transcript：已发送消息的权威记录 · 只追加 · 引用 blobSha          │
  └──────────────────────────────────────────────────────────────────────┘
```

---

## 2. 草稿文件（扩展层）

### 2.1 位置与分片

```text
  <context.storageUri>/composer-drafts/<sessionId>.json     ← 有工作区时
  <context.globalStorageUri>/composer-drafts/<sessionId>.json ← 空窗口时
```

**为什么按 sessionId 分文件**，而不是一个大文件装全部草稿：

```text
  一个大文件                        一 session 一文件（采纳）
  ─────────                        ────────────────────
  两个窗口编辑不同 session          天然不争抢
    也会抢同一个文件
  文件损坏 = 全部草稿没了            损坏只影响一个 session
  写一次要序列化全部草稿             只写正在编辑的那份
```

**为什么是 `storageUri` 而不是 Memento / `globalState`**：

- Memento 会进 Settings Sync，草稿是本机临时内容，不该同步到别的机器
- Memento 没有原子写语义，也不方便隔离损坏的内容
- 这也是 VS Code chat 自己的选择（`workspaceStorage/<id>/chatSessions/`，见 [`01`](01-placement-decision.md#3-决策)）

### 2.2 文件内容

```jsonc
{
  "schemaVersion": 2,
  "updatedAt": 1753400000000,
  "text": "看看这张图",
  "segments": [ /* @ 引用的文件/选区 */ ],
  "attachments": [
    {
      "id": "att_01H…",              // 前端 mint，用于 UI 增删
      "kind": "image",
      "filename": "screenshot.png",
      "mimeType": "image/png",
      "bytes": 4581234,              // 仅用于显示
      "blobSha": "3f2a…",            // ← 唯一指向字节的东西
      "sourcePath": "/workspace/docs/screenshot.png", // 可选：仅给 UI hover / reopen 用
      "hasThumb": true,              // 缩略图在 thumbs/<blobSha>
      "providerSha": null,           // 仅 SVG 有：webview 转出的 PNG
      "providerText": null           // 仅 SVG 栅格化失败时有：源码
    }
  ]
}
```

**这里面没有一个字节是图片/PDF 数据。** 整份草稿约 200~400 字节，与附件数量几乎无关（每个附件多约 200 字节的元数据）。这是写放大被消除的直接体现。

`sourcePath` 是这次新增的一个**纯 UI 字段**，只解决两个用户体验问题：

```text
  ① PDF 方块 hover 时，要能显示完整本地路径
  ② 历史里的文件方块，用户点一下要能回到原文件
```

它**不进 Rust，不参与任何权威判定**。Rust 只认 `blobSha` 和字节本身，因为：

```text
  路径会变，哈希不会变
  UI 想显示“它原来来自哪里”
    != 后端需要把“原路径”当事实源保存一份
```

### 2.3 耐久性三条机制

```text
  ① 防抖 400ms
     按住键盘连打 100 个字符 → 落盘 1 次，不是 100 次
     （VS Code chat 用 150ms，我们取 400ms —— 草稿比它的输入状态更廉价）

  ② 原子写：temp + rename
     写 <sessionId>.json.tmp → rename 覆盖 <sessionId>.json
     崩在中间：磁盘上要么是旧草稿、要么是新草稿，绝不会是半截 JSON

  ③ 损坏隔离，不删除
     JSON 解析失败 / schemaVersion 比当前构建新 → rename 成 <sessionId>.json.corrupt
     用户立刻得到一个可用的空输入框，坏文件还留在磁盘上可供排查
```

第 ③ 条里「schemaVersion 比当前构建新就当作无草稿」是刻意的：读一个更新版本写的文件会**静默丢掉不认识的字段**，那比从空开始更糟 —— 用户会以为草稿恢复了，其实少了东西。

### 2.4 清理时机

```text
  收到 prompt ack          → 清（发送成功才清，见下）
  session 已不存在         → hydrate 时丢弃（悬挂草稿懒清理）
  草稿变空                 → 删文件，而不是留一个 {"text":""}
                             （留着会在下次 hydrate 时「复活」成一份空草稿，
                               让「已清空」看起来像「没保存」）
```

**为什么必须等 ack 才清**：乐观清理的话，一次失败的发送会把用户正要发的内容弄丢。代价是「ack 之后、清理之前」崩溃会让已发送的文字留在输入框里 —— 轻微烦人、一眼可辨、零数据损失。这个取舍的完整论证见 [`01` §3.4](01-placement-decision.md#34-新的失败模式以及为什么它更好)。

### 2.5 切换、重载与发送的保留规则

一份草稿只属于一个 `sessionId`。会话 A 和 B 的输入、引用和附件互不覆盖；切换会话只换屏幕上显示的草稿，不算一次编辑，也不会清空另一份。

```text
  用户在 A 输入 ──→ 保存 A 的最新值
  切到 B        ──→ 只显示 B 的最新值
  再切回 A      ──→ 只显示 A 的最新值

  旧磁盘读取 / 旧 state 帧晚到  ──→ 不得覆盖较新的内存输入
  发送失败                       ──→ 保留原草稿
  发送成功                       ──→ 只清除“这次发送的那一次编辑”
  发送期间又输入                 ──→ 保留后来输入（即使又改回相同文字）
```

实现上，每个会话有自己的防抖队列和“最新值”。冷读会检查它开始读取后是否已有新修改；完整状态帧在真正发送给 webview 前会再读取一次内存草稿。这样“迟到”的读取或界面刷新只会失效，不能倒写用户输入。

**回归测试入口**：

```bash
# 草稿文件的迟到读取、清空和并发读取
npm exec -- vitest run src/shared/tests/composerDraft.test.ts --maxWorkers 1

# host 的完整状态帧和发送期间的新编辑
npm exec -- vitest run src/ui/webview/tests/provider_broadcast.test.ts \
  tests/webview_provider_flow.test.ts --maxWorkers 1

# 真实 VS Code：A/B 各自输入、来回切换、重载后检查 DOM 和状态
TOMCAT_E2E_GREP='keeps multiple Tomcat webview sessions isolated' \
  npm run test:e2e:webview-devhost
```

### 2.6 升级影响（一次性）

旧版本把草稿存在后端 `sessions_dir/drafts/<sessionId>.json`。**这些草稿不做迁移，直接丢弃** —— `SessionManager::discard_legacy_draft_dir()` 在 serve 启动时删掉整个旧目录。

理由：草稿是「未发送的临时内容」，为一次性的格式变更写迁移代码不划算。用户可见的影响是**升级后未发送的草稿会清空一次**，这一点已写进用户文档（[`03-user-guide.md`](03-user-guide.md#升级注意)）。

---

## 3. blob store（Rust）

### 3.1 目录布局

```text
  sessions/attachments/
    blobs/<sha256>                  全部图片字节，内容寻址，不可变
    thumbs/<源sha256>               192px 缩略图，纯派生
    pending/<sessionId>/<sha256>    租约标记（空文件）
```

### 3.2 为什么只有一个装字节的目录

早期设计分成 `blobs/`（权威）与 `cache/`（从 transcript 物化的历史图）两个目录。这个划分被推翻了，原因很具体：

```text
  宿主拿到一个 blobSha，要拼出 <img src="…">。
  如果字节可能在两个目录里，它【无从判断该拼哪一个】——
  而 <img> 没法「试一个不行再试另一个」。
```

区别其实只在**保留策略**上，而保留策略看的是租约与 transcript 引用，不需要靠目录来表达。所以：原图、SVG 转出的 PNG、从 transcript 物化的历史图，全都进 `blobs/`，各按自己的内容哈希寻址。宿主的查找规则因此只有两行：

```text
  全图    blobs/<blobSha>
  缩略图  thumbs/<blobSha>
```

`thumbs/` 是唯一按「来源哈希」而非「自身哈希」寻址的目录 —— 因为它表达的是一个**映射**（某份字节的缩略图长什么样），不是一份独立内容。这样宿主只要有 `blobSha` 就能算出缩略图 URI，不需要多存一个 `thumbSha`。

### 3.3 内容寻址带来的三个性质

```text
  ① 天然去重
     同一张图粘 10 次 → 磁盘上只有 1 份（put 发现文件已存在就直接返回 sha）

  ② 完整性自校验
     读的时候重算哈希，与文件名不符说明字节被外部改动过
     → 隔离成 <sha>.corrupt-sha_mismatch 并当作不存在，而不是把坏字节喂给 provider

  ③ 零拷贝提升
     「发送」不需要搬字节 —— 只需要停止把它当作「待清理的草稿字节」
     promote() 的全部工作就是删掉一个空的租约标记文件
     并刷新 blob mtime，给并发 GC 一小时宽限
     （有测试断言 inode 不变、mtime 被刷新）
```

第 ③ 条是这套设计最省事的地方：传统做法会有一个「从草稿区搬到正式区」的动作，那既要拷贝 4MB 字节，又要处理「搬一半崩了」。内容寻址下这个动作根本不存在。

---

## 4. 租约与 GC

### 4.1 租约是什么

`pending/<sessionId>/<sha>` 是一个**空文件**，语义是「这份字节属于某个还没发出去的草稿」。文件的 mtime 就是租约时间。

用空文件而不是数据库表，是因为它要回答的问题极其简单（「还有人在等这份字节吗」），而文件系统已经免费提供了原子创建、时间戳和目录枚举。

### 4.2 完整生命周期

```text
  ingest   → put(bytes) 落 blob
             + mark_pending(sid, sha) 建租约
  打字     → 完全不碰这里（草稿文本在扩展层）
  hydrate  → touch_pending 续期，证明这份草稿还活着
             （租约已被 GC 回收但 blob 还在时会重新建立，不是报错）
  send     → promote(sid, sha) 只删租约标记，blob 原地不动
  GC       → 租约超过 TTL 后只删租约
             随后一次 mark-and-sweep：
               在 transcript / pending 名单中 → 保留唯一 blob
               两处都没有                  → 等 1 小时宽限后删 blob + thumb
```

### 4.3 三条 GC 分支

`gc_pending(ttl, is_referenced)` 只负责释放过期租约；它不直接删除 blob。随后
`sweep_orphan_blobs(live_shas, grace)` 用同一份在用名单执行实际删除：

| 租约是否超期 | 是否仍被 transcript 引用 | 是否还有别的 session 租着 | 动作 |
|---|---|---|---|
| 否 | — | — | 什么都不做 |
| 是 | 否 | 否 | 删租约；若 blob 已过 1 小时宽限，sweep 再删 blob + 缩略图 |
| 是 | 是 | — | 只删租约；名单保留 blob |
| 是 | 否 | 是 | 只删租约；pending 名单保留 blob |

在用名单由 `SessionManager::collect_live_blob_shas()` 一次流式扫描构建：

```text
pending/<session>/<sha>  ─┐
                          ├── HashSet<sha> ──► gc_pending / sweep_orphan_blobs
所有主 transcript 的 blob_sha ─┘
```

不维护 `referenced.json` 或引用计数。它们会引入崩溃时的双写一致性和修复路径；
transcript 已经是权威事实，低频后台扫描更简单可靠。若 sessions 目录超过 1GB 或扫描超过
2 秒，再增加可从 transcript 重建的每会话 `<session>.blob_refs` 派生文件。

### 4.4 TTL 取 7 天

```rust
pub const PENDING_BLOB_TTL: Duration = Duration::from_secs(7 * 24 * 60 * 60);
```

判据：足够覆盖「周五写了草稿、周一回来接着写」，又不至于让被遗忘的草稿字节长期占盘。而且 hydrate 会 touch 续期，所以只要用户还偶尔打开那个 session，租约就不会过期。

### 4.5 超预算淘汰：判据是「能不能重建」

`evict_rebuildable_over_budget(max_bytes, _)` 只把缩略图压到 256MB 以内，按最近最少使用淘汰。

关键是判据不是「在哪个目录」，而是**这份字节能不能重新造出来**：

```text
  缩略图                     能（webview 重新降采样一次）        → 可淘汰
  transcript 引用着的图      CAS 是唯一副本                      → 绝不动
  还持有租约的图             未发送，磁盘上就这一份              → 绝不动
```

所以超预算淘汰只会让下次打开图片时重建缩略图。已发送图片不再内联于 transcript，
删除它的 blob 会让历史图片永久裂开；总量治理应采用会话归档/保留策略，而不是淘汰 blob。

### 4.6 GC 什么时候跑

```text
  serve 启动时     run_attachment_housekeeping()（serve/mod.rs）
                     ├ discard_legacy_draft_dir()  丢弃旧版后端草稿目录
                     ├ collect_live_blob_shas()（一次流式扫描）
                     ├ gc_pending(7 天 TTL) + sweep_orphan_blobs(1 小时宽限)
                     └ evict_rebuildable_over_budget(256MB，仅 thumbs)
  delete_session   释放本会话租约后，再构建一次名单并 sweep
  物化历史图之后    只检查缩略图的 256MB 预算（commands.rs）
```

没有后台定时任务 —— GC 挂在本来就会发生的事件上，避免引入一个需要自己管生命周期的循环。
所以运行一次 `migrate_transcripts_v2.py` 后，还需要启动一次 Tomcat：迁移只搬运记录，
首次 `serve` 握手后的异步家务活才会开始清扫孤儿 blob；为防止崩溃时刚写入的引用丢失，
孤儿仍会保留一小时宽限期。

---

## 5. 依赖体积：删掉 `resvg` 省了多少

早期设计为了「把 SVG 转成 PNG 发给模型」在 Rust 引入 `resvg`。搬到 webview 之后这个依赖整体删除。以下是**实测数据**（macOS arm64，`resvg 0.45.1`），不是估算：

### 5.1 crate 数量

```text
  当前 tomcat/Cargo.lock                     539 个 crate
  加上 resvg 之后                            583 个 crate
                                             ─────────────
  避免的新增依赖                             44 个
```

这 44 个里包含**一整套字体系统**和**一组图片编解码器**：

```text
  字体（因为 SVG 里可能有文字，画文字就得有字体引擎）
    fontdb  fontconfig-parser  rustybuzz  ttf-parser
    unicode-bidi  unicode-bidi-mirroring  unicode-ccc
    unicode-properties  unicode-script  unicode-vo

  图片编解码
    png  gif  image-webp  zune-jpeg  zune-core  weezl  color_quant

  几何与栅格化
    resvg  usvg  tiny-skia  tiny-skia-path  kurbo  euclid  strict-num

  SVG 解析与其他
    roxmltree  simplecss  svgtypes  xmlwriter  imagesize  data-url
    arrayref  arrayvec  bytemuck  byteorder-lite  core_maths  libm
    memmap2  pico-args  quick-error  rgb  slotmap  tinyvec  tinyvec_macros
```

**核心 CLI 本来不需要认识字体。** 这是「这件事为什么要在 Rust 做」这个问题没被问出来的代价。

### 5.2 编译耗时

```text
  冷编译（debug，仅编译这 44 个依赖）            1 分 23 秒
  冷编译（release + LTO + codegen-units=1）      4 分 50 秒
```

每个开发者的每次干净构建、以及 CI 的每次冷缓存构建，都要付这份时间。

### 5.3 产物体积

用两个最小程序对比，同样是 `strip = true`、`lto = true`、`codegen-units = 1` 的 release 配置：

```text
  空 Rust 程序（基线）                    312,652 字节
  只调用一次 resvg::render 的程序       2,579,884 字节
                                        ─────────────
  resvg 贡献的机器码                   +2,267,232 字节 ≈ +2.16 MiB
```

**这是下限而不是典型值**：那个探针程序只调用了一条渲染路径，链接器把其余全部丢掉了。真实使用会触达更多 `usvg` / `fontdb` 代码，实际增量只会更大。

### 5.4 净结论

```text
  Rust 依赖        0 个新 crate（resvg 从未合入主干，本次是在合入前拦下）
  CLI 产物         不增加（省下 ≥2.16 MiB）
  冷编译           不增加（省下 83 秒 debug / 290 秒 release）
  VSIX             1.4MB 不变（像素工作在 webview，不需要任何原生模块）
  发布矩阵         不变
```

顺带消掉的还有整个「怎么在 Rust 里安全地解析 SVG」的问题域 —— 见 [`../image-attachments.md` §3.1](../image-attachments.md#31-为什么-rust-侧一个图形库都不装)。

---

## 6. 排查手册

**「我的草稿不见了」**

```text
  1. 看 <storageUri>/composer-drafts/ 有没有 <sessionId>.json.corrupt
     → 有：文件损坏被隔离了，内容还在，可人工恢复
  2. 看 session 是否还存在
     → 不存在：hydrate 主动丢弃了悬挂草稿（预期行为）
  3. 是否刚升级过
     → 旧版草稿存在后端，升级时一次性丢弃（预期行为，见 §2.5）
  4. 是否在另一个工作区的窗口里打的
     → 草稿是 per-workspace 的，不同工作区天然隔离
```

**「图片显示成裂图 / 失效附件」**

```text
  1. 看 blobs/<blobSha> 是否存在
     → 不存在：字节被 GC 或用户手删。这是【正常降级路径】，
                该附件显示为失效且可一键移除，草稿其余部分不受影响
  2. 存在但仍不显示 → 查 localResourceRoots 与 CSP
     两个 webview 的 img-src 都必须含 ${webview.cspSource}
     localResourceRoots 必须含 attachments/blobs、attachments/thumbs
     ※ 这里【没有】base64 兜底，配错就是裂图 —— 刻意的，见架构文档 §4
```

**「transcript 里的 `![...](path)` 没显示成图」**

```text
  1. 看 path 是不是工作区目录或系统临时目录里的本地文件
     -> 不是：这是预期降级，会显示成可点击文本，不显示 <img>

  2. 看 host 下发的 mediaRoots / localResourceRoots
     -> 必须含当前 workspace folders 与 os.tmpdir()

  3. 看 path 是不是远程 URL（http/https/data/blob）
     -> 是：一律不渲染图片
```

**「磁盘占用太大」**

```text
  安全删：thumbs/ 全部、被 transcript 引用的 blobs/（都会自动重建）
  别删：  pending/ 里还在租约期内的 sha 对应的 blobs/（那是未发送内容的唯一副本）
  或者直接等 GC —— 它在每次 serve 启动时跑。
```
