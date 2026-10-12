# 01 · 草稿该放哪一层：横向调研、决策与推翻条件

> 上位文档：[`../image-attachments.md`](../image-attachments.md)
> 本文记录一个**已被推翻的设计**和推翻它的依据。原设计把 composer 草稿（用户打了但还没发的内容）存在 Rust 后端，用 CAS + 跨进程两阶段提交保证一致性。它是当时 8 条 P0/P1 缺陷的共同根因。
> 证据引用的外部仓库与本仓同级，位于 `/Users/yankeben/workspace/`（`vscode/`、`cline/`、`continue/`、`codex/`、`opencode/`、`pi/`、`cc-fork-01/`），仅作证据引用、不进本仓。文中行号是调研当日快照，符号名与测试名更耐久。

**一句话结论**：**编辑状态归扩展层，图片字节归 Rust 但用内容寻址引用而非 base64 内联。**

---

## 1. 先问对问题

判断一份状态归谁，第一性原理上只看三件事：

```text
  ① 谁是语义拥有者      —— 谁定义它的 schema、谁能解释它的内容
  ② 生命周期跟谁绑定    —— 它必须随谁生、随谁死
  ③ 丢失的后果是什么    —— 这决定一致性机制该做多强
```

逐条套到 composer 草稿上：

### 第一问：Rust 存的是它读不懂的东西

原设计的 Rust 侧模型长这样（`tomcat/src/core/session/composer_draft.rs`，已删除）：

```rust
pub struct ComposerDraftSegment {
    pub segment_type: String,                 // "text" | "reference" —— 前端的分类法
    pub text: Option<String>,
    pub reference: Option<serde_json::Value>, // ← 完全不透明
}
pub struct ComposerDraftAttachment {
    pub id: String,    // 前端 mint 的 UUID
    pub kind: String,  // "image" | "file" —— 前端的分类法
}
```

`reference` 是 `serde_json::Value`，**Rust 全程不解释、原样存取**。`segments` 是编辑器的文档模型，`id` 与 `kind` 都是前端的词汇。后端在这里就是一个看不懂自己所存 schema 的键值仓库 —— 这也正是「四个类型描述同一件事、响应体没有 schema、TS 侧只能 `as any`」的根源。

### 第二问：生命周期确实绑在 Rust 的 session 上

这是**唯一真正指向 Rust 的论据**：草稿以 `sessionId` 为键，而 session 身份归 Rust。

但它不决定性 —— 悬挂草稿可以懒清理：hydrate 时问一句「这个 session 还在吗」，不在就丢。这是一次读取，不是一套同步机制。

### 第三问：一致性机制的强度远超数据本身的价值

草稿丢了用户重打一遍，不是数据损坏，和 transcript 丢失不是一个量级。但原设计为它上了一套**跨进程两阶段提交**：

```text
  submitting marker 落盘
    → 写 transcript
      → 按 userMessageId 回查 transcript，判断该消费还是该恢复
```

这套机制之所以必要，纯粹因为**草稿和「发送是否成功」这个信号被放在了两个进程里**。把它们放回同一个进程，机制本身就不需要存在。

---

## 2. 七个参考实现的横向调研

对 7 个仓库逐个查证三个问题：未发送输入放哪、图片字节怎么走、有没有草稿 CAS。

### 2.1 落点：7/7 都不在后端做权威存储

| 实现 | 草稿放哪 | 证据 |
|---|---|---|
| **VS Code** | workbench 的 `InputModel` 内存 + `IStorageService`(WORKSPACE) + `workspaceStorage/…/chatSessions/` 文件 | 无独立后端进程参与 |
| **Cline** | 只在 **webview React state**，webview 一销毁即丢 | `src/shared/storage/state-keys.ts` 的 `GLOBAL_STATE_FIELDS` 里**零个** composer/draft 相关 key |
| **Continue** | **TipTap/ProseMirror 内存**，打字完全不跨 IPC | 三层架构（gui + 独立 core Node 进程 + 扩展），与 Tomcat 最同构；core 的 `history/save` 只存已发送会话 |
| **Codex** | **TUI 进程内存** | `codex-rs/tui/src/bottom_pane/chat_composer.rs:395` 有 `struct ComposerDraft`；而 `codex-rs/core/` 与 `codex-rs/app-server/` 里 `ComposerDraft`/`composer_draft` **零命中** |
| **OpenCode** | **客户端本地**（Web 落 localStorage / 桌面存储，TUI 内存） | server 的 `session_input` 表只存 admitted 已提交的 prompt |
| **pi** | 单进程 TUI，内存 React state | 粘贴时写 `os.tmpdir()`，编辑器里只存路径字符串 |
| **Claude Code** | 单进程 TUI，内存 React state | 图片写 `~/.claude/image-cache/<sessionId>/` |

**Codex 的对照最有说服力** —— 它也有一个叫 `ComposerDraft` 的 Rust struct，注意它的路径：

```text
  codex-rs/tui/src/bottom_pane/chat_composer.rs:395
           ^^^
           在【前端进程的内存里】，不在 core
```

同一个名字的东西，OpenAI 放前端内存，我们原来放后端落盘。而 Codex 的 core 是 Rust、前端有多个（TUI / app-server 客户端），架构与 Tomcat 高度同构 —— 它有充分动机把草稿放 core，但没有。

### 2.2 图片字节：三种流派，没有一家是原设计那样

```text
  ① 文件路径引用（Codex、pi）
     Codex 的线协议类型就是 LocalImage { path: PathBuf }
       codex-rs/app-server-protocol/src/protocol/v2/turn.rs:302
     core 直到真正调模型时才 std::fs::read + 转 base64
     pi 更直接 —— 粘贴时写 os.tmpdir()，编辑器里只存路径字符串

  ② 一次性落盘缓存（VS Code、Claude Code）
     粘贴那一刻写文件，草稿只留引用
     Claude Code 写 ~/.claude/image-cache/<sessionId>/，且按键不会重复 storeImage

  ③ base64 内联在客户端内存（Cline、Continue、OpenCode）
     不落盘，接受重启丢失

  【原设计】base64 内联，且每次按键都跨进程往返 + 落盘 ← 没有先例
```

我们最终采用的是 ①+② 的组合：内容寻址的一次性落盘（②），协议上只传哈希（①）。

### 2.3 决定性证据：微软已经把这个方案写成测试了

VS Code 是 7 家里唯一真把草稿落盘的，所以它必须面对写放大。它的解法恰好就是我们最终采用的两条：

**第一，文本 debounce：**

```ts
// vscode/src/vs/workbench/contrib/chat/browser/widget/input/chatInputPart.ts:738
this._syncTextDebounced = this._register(new RunOnceScheduler(() => {
    this._syncInputStateToModel();
}, 150));
```

**第二，图片载荷与频繁更新的输入状态分两个 storage key 存 —— 而且有一个测试专门守着这条不变量：**

```ts
// vscode/src/vs/workbench/contrib/chat/test/browser/widget/input/chatInputStatePersistence.test.ts:17
test('stores image payloads separately from frequently updated input state', () => {
    // ...
    attachmentPayloadIsSeparate:
        !serializedState.includes('$base64') && serializedAttachments.includes('$base64'),
```

测试名直译就是「把图片载荷与频繁更新的输入状态分开存储」。文本走 `chat.untitledInputState`，图片 base64 走 `chat.untitledInputAttachments`。

**这条不变量我们原本是当作一条待办提出的，而微软已经用一个名字直白的测试把它固化了。** 这是本次调研回报最高的一条发现 —— 它说明这个坑不但有人踩过，还留了路标。

### 2.4 并发控制：7/7 全部没有草稿 CAS

VS Code 的草稿写入就是朴素的 last-write-wins：

```ts
// vscode/src/vs/workbench/contrib/chat/common/model/chatModel.ts
setState(state: Partial<IChatModelInputState>): void {
    const current = this._state.get();
    this._state.set({ ...current, ...state }, undefined);
}
```

值得注意的是 VS Code 用的是 `StorageScope.WORKSPACE`，**同一工作区的多个窗口是共享草稿的** —— 即便在这种真有多写者的场景下它也没上 CAS，只靠 `onDidChangeValue` 的 `e.external` 事件刷新一次。

Codex 唯一的前置条件检查是 `expected_turn_id`，但那是给 steer 已提交 turn 用的，与草稿无关。

### 2.5 唯一的「草稿推后端」先例，形态和原设计相反

VS Code 的 Agent Host 确实会把草稿推给 agent 后端：

```ts
// vscode/src/vs/workbench/contrib/chat/browser/agentSessions/agentHost/agentHostSessionHandler.ts:3815
const delayer = store.add(new Delayer<void>(AgentHostSessionHandler.DRAFT_SYNC_DEBOUNCE_MS)); // 500ms
store.add(autorun(reader => {
    const state = inputModel.state.read(reader);
    delayer.trigger(() => {
        const draft = this._inputStateToDraft(sessionResource, state);
        this._config.connection.dispatch(chatKey, { type: ActionType.ChatDraftChanged, draft });
    });
}));
```

三个特征恰好与原设计相反：

```text
  VS Code                              原设计
  ───────                              ──────
  debounced（500ms）                    每次按键
  单向 dispatch 通知                    后端权威 + 前端回读
  workbench 侧仍持有权威 inputModel     后端是权威，前端只是缓存
```

而且外部会话落 metadata 时附件被**显式清空**：

```ts
// vscode/src/vs/workbench/contrib/chat/common/model/chatSessionStore.ts:857
const rawInputState = isExternal ? session.inputModel.toJSON() : undefined;
const inputState = rawInputState ? { ...rawInputState, attachments: [] } : undefined;
```

也就是说，业界唯一的「草稿给后端」先例，也明确**不把附件字节送过去**。

---

## 3. 决策

**编辑状态归扩展层，图片字节归 Rust 但用内容寻址引用而非 base64 内联。**

```text
  扩展层（workspaceStorage 下的文件，非 Memento、非 Settings Sync）
    composer 编辑状态
    { text, segments,
      attachments: [{ id, kind, filename, mimeType, bytes,
                      blobSha,       ← 原图（预览大图 / 发给模型）
                      providerSha?,  ← 仅 SVG 有：ingest 时 webview 转好的 PNG
                      hasThumb }] }  ← 是否已有 192px 缩略图
    ↑ 小、UI schema、可丢弃、防抖落盘

  Rust 后端
    内容寻址 blob store（纯字节仓库，零图形库）
      attachments/blobs/<sha256>
    ↑ 大、不可变、发送后与 transcript 共用同一份字节
```

**存哪个目录：照抄 VS Code。** 用 `context.storageUri`（workspaceStorage）按 sessionId 分文件；没打开文件夹的空窗口退回 `globalStorageUri`。这正是 VS Code chat 的做法：

```ts
// vscode/src/vs/workbench/contrib/chat/common/model/chatSessionStore.ts:71
this.storageRoot = isEmptyWindow ?
    joinPath(this.userDataProfilesService.defaultProfile.globalStorageHome, 'emptyWindowChatSessions') :
    joinPath(this.environmentService.workspaceStorageHome, workspaceId, 'chatSessions');
```

### 3.1 由此得到的恢复能力（这是本方案对用户可感知的承诺）

```text
  切到别的 session 再切回来      草稿在  ✓
  webview 重新加载              草稿在  ✓
  扩展重启                      草稿在  ✓
  VS Code 完全退出重开           草稿在  ✓
  tomcat 后端进程崩溃 / 升级      草稿在  ✓  ← 原设计做不到，草稿在后端手里
  图片附件随草稿一起恢复          是      ✓  ← 字节仍在 Rust 落盘，哈希指得到

  同工作区开两个窗口、都在同一 session 打字
    → 抢同一个文件，后写的赢（VS Code 就是这个行为，连 CAS 都没上）
    → 场景罕见，最坏后果「刚打的字被覆盖」，重打即可
  不同工作区的窗口
    → 天然隔离，本来也不该互相看到
```

准确的作用域措辞是 **per-workspace + last-write-wins**（不是 per-window —— VS Code 并不提供「按窗口持久化」的存储），与 VS Code chat 完全一致。

### 3.2 每条设计都有至少两家背书

```text
  编辑状态放扩展层 + 防抖落盘          VS Code / Cline / Continue / OpenCode
  图片粘贴时一次性落盘、草稿只持哈希    VS Code / Claude Code / Codex / pi
  稳态协议永不传图片 base64            Codex（LocalImage { path }）
  删掉 CAS / submitting marker / 崩溃恢复   7/7
  草稿作用域跟着前端走                  7/7（VS Code 是 per-workspace，同样无 CAS）
```

### 3.3 这一刀直接消灭的问题

| 原问题 | 为什么消失 |
|---|---|
| CAS 可被绕过（有一条不带 `expectedRevision` 的写入路径） | 单写者，不需要 CAS |
| 崩溃恢复挂在 GET 命令的副作用里 | 草稿与「发送成功」信号同进程，不需要跨进程恢复 |
| `mark_submitting` 早于校验，失败路径留下脏标记 | 不再有 marker |
| `get_draft` 的错误契约自相矛盾 | Rust 不再维护草稿文件 |
| 重复附件 id 未被拒绝 | id 由扩展层 mint 并自校验，不过协议 |
| `enforce_budget` 解码两遍且第二遍静默吞错 | 预算按 blob 索引的 `bytes` 字段求和，零解码 |
| 四个 wire 类型 + 无 schema 的响应体 | 草稿协议整体不存在 |
| 写放大（打一个字搬 132MB） | 打字不跨进程 |

### 3.4 新的失败模式，以及为什么它更好

扩展在收到 prompt ack 之后才清草稿。若扩展在 ack 与清理之间崩溃，用户会看到已发送的文字还留在输入框里 —— 轻微烦人、一眼可辨（消息已经在历史里了）、**零数据损失**，且不需要任何 marker。

这比原来的跨文件两阶段提交是**严格更优**的失败模式：原设计在同样的时刻崩溃需要靠回查 transcript 才能判断状态，而判断错的后果是草稿被误删（真丢数据）或消息被重发。

### 3.5 诚实列出新增的成本

- **blob 归属跨层**：扩展持哈希、Rust 持字节。草稿 blob 带 TTL 租约，发送时提升为 transcript 引用；Rust 侧 GC 清理既不被 transcript 引用、又超过 TTL 的字节。扩展 hydrate 草稿时对仍在用的 blob 做一次 touch 续期。细节见 [`02-storage-and-gc.md`](02-storage-and-gc.md)。
- **两边可能不同步**：用户手删 `~/.tomcat`、或草稿文件被同步工具带到另一台机器。对策是把「blob 缺失」当成**正常降级路径而非错误** —— 该附件显示为失效并可一键移除，草稿其余部分不受影响。这条有测试固化。
- **草稿与 session 的引用完整性**：扩展 hydrate 时校验 session 是否仍存在，不存在则丢弃草稿。

---

## 4. 什么情况下这个决策应当被推翻

只有一个条件：**产品上明确要求「草稿属于 session 而非窗口」** —— 即换窗口、换机器、换前端都要看到同一份草稿。

若这是有意的产品能力，那么后端权威存储是必要的，CAS 与两阶段提交就是这个能力的合理代价，此时应当回到 Rust 方案，并把上表 §3.3 里那些漏洞逐条补干净（它们不是「本可避免的错误」，而是这个能力的固有成本）。

**当前证据不支持这个要求：**

```text
  · CLI 侧对草稿零引用（rg composer_draft tomcat/src/api/cli/ 无命中）
  · 今天没有任何可与之同步的对等前端
  · VS Code / Cursor 自身的 chat 输入草稿都是窗口级的，用户预期也在这一侧
```

---

## 5. 已排除：扩展宿主的 Node 原生模块

这一节记录的是**另一个**被排除的方向：既然像素工作要搬出 Rust，为什么不搬到扩展宿主（Node），而是搬到 webview？

**本方案已明确排除宿主原生模块，它不在任何降级链上。** 记录在此只为留下判断依据，避免后人重走。

直觉上「放宿主比放 Rust 轻」，但对**原生代码**恰好相反。

### 5.1 Node 侧没有纯 JS 的 SVG 栅格化方案

能用的全是原生模块：

```text
  @resvg/resvg-js      napi-rs 绑定 —— 【它就是 resvg 本身】，每平台一个 .node 约 5~9MB
  sharp                libvips + librsvg，每平台 10MB+
  @napi-rs/canvas      Skia 绑定，每平台 10MB+
  node-canvas          Cairo + librsvg，安装出名地痛苦
  convert-svg-to-png   底层是 Puppeteer，会下载整个 Chromium —— 荒谬
```

注意第一条：最主流的 Node 方案就是我们正要删掉的 resvg。搬到宿主不是删掉它，是**把它重新打包成每平台一份的原生二进制，然后要发 6 份**。

### 5.2 平台矩阵是躲不掉的那部分

```text
  Rust 侧加 resvg
    tomcat CLI 本来就是【按平台编译发布】的二进制
    → 只是让这个二进制大一点、编译慢一点
    → 发布矩阵不变、VSIX 不变、Remote 场景不变

  扩展宿主加原生模块
    扩展现在是【平台无关的 1.4MB 包，dependencies 为空】
    → 必须二选一：
        (a) 发 6 个平台专属 VSIX（darwin-arm64/x64、win32-x64/arm64、linux-x64/arm64）
        (b) 打胖包塞 6 份 .node，VSIX 从 1.4MB 变 30~50MB
    → Remote-SSH / Dev Container / WSL 下扩展宿主跑在【远端】，
       .node 必须匹配远端平台 → 发布与验证矩阵都 ×6
    → 引入本扩展的第一个原生模块，从此每次发版都要管二进制
```

### 5.3 四个方案的代价对比

| 方案 | Rust 依赖 | 扩展原生模块 | VSIX | 发布矩阵 |
|---|---|---|---|---|
| **webview canvas**（采纳） | 0 | 0 | 1.4MB 不变 | 不变 |
| **SVG 源码当文本**（降级链第 2 档） | 0 | 0 | 不变 | 不变 |
| Rust resvg 瘦身（最后退路） | ~15 crate | 0 | 不变 | 不变 |
| 宿主原生模块（**排除**） | 0 | 有（就是 resvg） | 30~50MB 或拆 6 个包 | **×6** |

napi-rs 走 N-API，ABI 跨 Node / Electron 稳定，**不需要** `electron-rebuild` —— 这点上它比老式原生模块好，但平台矩阵躲不掉。

**结论：宿主原生模块严格劣于「留在 Rust」，而「留在 Rust」又劣于「搬到 webview」，所以它被两次超越，排除。**

### 5.4 降级链（栅格化根本不是必须的）

```text
  1. webview canvas → PNG              首选，已验证可用
  2. 失败 → SVG 源码作为文本块发给模型   零依赖；对 logo / 图标类可能效果更好
           显示仍走原生 <img>，用户可见的部分毫无变化
           源码上限 50KB，避免 token 爆炸
           已知不适用：SVG 内部嵌了 base64 位图时源码无信息量
  3. 都不接受 → Rust resvg 瘦身          最后退路，仅在 1、2 都被否时启用

  【已排除，不在链上】扩展宿主 Node 原生模块 —— 理由见 §5
```

第 2 条一旦成立，「栅格化必须有个图形库」这个前提本身就不成立了。这也是为什么 canvas spike 是**选路项而非阻塞项** —— 显示 SVG 与栅格化 SVG 是两条不同的代码路径：

```text
  显示（用户在历史消息里看到那张图）
    <img src="…blob:…svg">                 ← 浏览器最基础的功能，一定能用
                                            spike 失败也完全不影响这条
  栅格化（转成 PNG 发给模型）
    canvas.drawImage(svgImg) + toBlob()    ← 只有这一步有不确定性
```

---

## 6. 方法论教训

这一节写给后续的架构决定，不只是这一次。

**教训一：选架构前先做同类产品调研。** 手边就有 7 个可参考仓库，半天就能查完，能避免约 600 行 Rust 与 8 条 P0/P1 的返工。更值得注意的是，本次花大力气推导出的「图片载荷与频繁更新的输入状态分开存」，微软早已写成了一个名字直白的测试 —— 坑有人踩过，还留了路标。这一步的成本（半天）与它避免的返工量差了一个数量级。

已把这条固化为项目规则，见 `.cursor/rules/prior-art-before-architecture.mdc`。

**教训二：当一个方案的形态是「接受现状 + 局部优化」时，要警觉自己漏了上一层的问题。** 本次在 `resvg` 这一节就犯过这个错 —— 接受了「栅格化必须在 Rust」的前提，只想着给 `resvg` 瘦身，于是把「44 个 crate」当成一个体积问题去权衡，而没有问「这件事为什么要在 Rust 做」。

```text
  方案里出现「瘦身」「缓解」「降低」这类动词时，
  先回头确认那个被优化的东西是否本就不该存在。
```
