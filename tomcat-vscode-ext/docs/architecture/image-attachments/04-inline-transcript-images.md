# 04 · Transcript 里的本地图片：什么时候直渲，为什么只认 `![...](path)`

> 上位文档：[`../image-attachments.md`](../image-attachments.md)
> 本文只回答一件事：assistant 在 transcript 里提到一张本地图片时，什么时候应该直接显示成图，什么时候应该继续只是一个可点击路径。
> 证据引用的外部仓库与本仓同级，位于 `/Users/yankeben/workspace/`（`vscode/`、`cline/`、`continue/`、`codex/`、`opencode/`、`pi/`），仅作调研证据，不进本仓。

**一句话结论**：**把 transcript 内联图做成“显示层能力”，不是新的附件协议。** 模型只有在想让用户直接看到图片时才写 `![alt](path)`；host 只把**已授权目录**里的本地路径改写成 webview URL；不把这些图片重新搬进 Rust CAS，也不放开任意本地绝对路径。

---

## 1. 先把问题说人话

用户要的是下面这件事：

```text
assistant 回答里出现

  这是当前草图：
  ![首页线框图](docs/mockup.png)

用户在 transcript 里立刻看到图
  - 鼠标悬浮：放大镜光标
  - 点击：轻量大图
  - 点遮罩或按 Esc：关闭
```

用户**不要**下面两件事被误伤：

```text
  `docs/mockup.png`
    这只是“提到一个文件”，应该继续是可点击路径，不该变成图片

  ![图](https://example.com/mockup.png)
    这是远程 URL。既不可信，也不在 webview 授权范围内，不该加载
```

所以真正的问题不是“能不能把 `<img>` 画出来”，而是**怎么把三种意图分开**：

| 模型写出来的东西 | 用户真正想要什么 | 我们应该怎么渲染 |
|---|---|---|
| `` `src/app.ts` `` | 打开文件 | 可点击文件链接 |
| `![mockup](docs/mockup.png)` | 直接看图 | 内联图片 |
| `![mockup](https://...)` | 远程内容 | 不显示图片，降级成普通链接/文本 |

---

## 2. 先看别人怎么做

### 2.1 五家实现的硬证据

| 实现 | 它怎么处理“图片/文件显示” | 硬证据 |
|---|---|---|
| **VS Code** | 图片是**结构化 attachment widget**，点开走图片轮播/预览；输入状态与图片载荷分开存。没有“模型随手写本地路径，transcript 自动出图”这条路。 | `vscode/src/vs/workbench/contrib/chat/test/browser/widget/input/chatInputStatePersistence.test.ts:16`，测试名 `stores image payloads separately from frequently updated input state`；`vscode/src/vs/workbench/contrib/chat/browser/attachments/chatAttachmentWidgets.ts:530` 的 `clickHandler()` 与 `:561` 的 `openInCarousel()` |
| **Codex** | 本地图片是**一等输入类型**，直接传 `path: PathBuf`；草稿留在 TUI 进程内存，不让 transcript 自己去猜路径字符串。 | `codex/codex-rs/app-server-protocol/src/protocol/v2/turn.rs:301` 的 `LocalImage { path: PathBuf }`；`codex/codex-rs/tui/src/bottom_pane/chat_composer.rs:395` 的 `struct ComposerDraft` |
| **Cline** | 选图时把文件读成 **data URL**；打开图片时再写到 `os.tmpdir()` 后交给编辑器打开。不是“本地 markdown 路径直渲”。 | `cline/apps/vscode/src/integrations/misc/process-files.ts:12` 的 `selectFiles()`；`cline/apps/vscode/src/integrations/misc/open-file.ts:7` 的 `openImage()` |
| **Continue** | 输入历史存在浏览器 `localStorage`；发给模型时把图片转成 `image_url.url` / base64 block。重点在“模型 payload”，不是“transcript 本地路径直渲”。 | `continue/gui/src/hooks/useInputHistory.ts:13` 的 `useInputHistory()`；`continue/packages/openai-adapters/src/apis/openaiResponses.ts:50` 的 `convertImagePart()`；`continue/packages/openai-adapters/src/apis/Anthropic.ts:203` 的 `dataUrl?.startsWith("data:")` 分支 |
| **OpenCode** | 用户文件 part 在分享页里就是“附件标签 + 文件名”；图片放大是独立 dialog 组件，不靠 markdown 本地路径自动渲染。 | `opencode/packages/web/src/components/share/part.tsx:171` 的 `props.part.type === "file"` 分支；`opencode/packages/ui/src/components/image-preview.tsx:10` 的 `ImagePreview()` |
| **pi** | 粘贴图片时先写到 `os.tmpdir()`，再把**文件路径字符串**插进编辑器。也就是说，“图片路径”本身仍然只是文本。 | `pi/packages/coding-agent/src/modes/interactive/interactive-mode.ts:2596` 的 `handleClipboardPaste()`；`pi/packages/tui/src/components/editor.ts:1021` 的 `insertTextAtCursor()` 注释“clipboard image markers” |

### 2.2 反例：两个最同构的实现都没有做“本地路径自动出图”

对 Tomcat 最有参考价值的，其实是 **VS Code** 和 **Continue**：

```text
  它们都具备：
    多前端 / 独立宿主进程 / markdown transcript / 图片输入

  但它们都没有做：
    “模型随手写一个本地文件路径 -> transcript 自动渲染成 <img>”
```

这不是因为它们“不会画图”，而是因为这件事天然有两个坑：

1. **提示词歧义**\
   同一个 `docs/mockup.png`，有时是“请打开这个文件”，有时是“请直接把图显示出来”。如果不额外约定语法，模型只能乱猜。

2. **磁盘授权边界难收**\
   markdown 里的路径是模型输出，不能默认信任。只要一放开“任意绝对路径都能加载”，就是把整个用户磁盘往 webview 暴露。

所以我们不是“照抄竞品没做”，而是先承认这条路确实更危险，再把危险点收得足够窄。

---

## 3. 我们的决策：只把它做成一条很窄的显示支线

### 3.1 规则本身

```text
  只有标准 markdown 图片语法
    ![alt](path)
  才有资格被当成图片

  反引号路径
    `path/to/file.png`
  继续只是文件链接
```

这是这次设计里最重要的一条。因为它把“提到文件”和“显示图片”变成了两个不同语法，模型和渲染器都不必猜。

### 3.2 显示链路长什么样

```text
  assistant 文本
    -> marked 把 ![alt](path) 变成 <img src="path">
    -> rewriteLocalImages() 逐个检查每个 <img>
         ├─ 在授权目录内
         │    -> 改写成 webview 可读 URL
         │    -> 加 data-tc-image-src
         │    -> 加 zoom-in 光标
         └─ 不在授权目录内 / 是远程 URL
              -> 不保留 <img>
              -> 降级成普通链接文本
```

对应实现文件：

- `tomcat-vscode-ext/gui/src/components/markdown/localImages.ts:88` 的 `resolveLocalImageSrc()`
- `tomcat-vscode-ext/gui/src/components/markdown/localImages.ts:122` 的 `rewriteLocalImages()`
- `tomcat-vscode-ext/gui/src/components/markdown/markdownDecorators.ts` 把这一步接进既有 markdown 装饰管线

### 3.3 为什么授权范围只给“工作区目录 + 系统临时目录”

host 只把两类目录授权给 transcript 这条支线：

1. **当前工作区目录**\
   这些本来就是用户已经打开、也已经允许 agent 提到的文件。
2. **系统临时目录 `os.tmpdir()`**\
   剪贴板图片、验收产物、以及别家实现（例如 pi、Cline）的过渡文件都经常落在这里。

对应实现：

- `tomcat-vscode-ext/src/ui/webview/provider.ts:2466` 的 `resourceRoots()`
- `tomcat-vscode-ext/src/ui/webview/provider.ts:2483` 的 `workspaceMediaRootUris()`
- `tomcat-vscode-ext/src/ui/webview/provider.ts:2487` 的 `mediaRootsForWebview()`

这一步还顺手解决了两个细节：

```text
  ① 新加工作区文件夹
     立刻重算授权范围，不要求重开侧栏

  ② macOS /private/tmp
     前端先做路径规范化，把 /private/tmp 视作 /tmp
```

前者在 `provider.ts:489` 的 `onDidChangeWorkspaceFolders` 订阅里；后者在 `localImages.ts:40` 的 `canonicalizeFsPath()` 里。

### 3.4 为什么不把 transcript 内联图也塞进 Rust CAS

因为这条能力的目标只是**显示**，不是**把一张新图片纳入附件生命周期**。

两者完全不是一回事：

| 场景 | 用户在做什么 | 该不该进 CAS |
|---|---|---|
| 粘贴图片到 composer | 用户把一份新字节交给系统，准备随消息发送 | **该**，因为要持久化、去重、跟发送历史关联 |
| assistant 写 `![mockup](docs/mockup.png)` | assistant 只是提到“磁盘上已有的一张图” | **不该**，因为这不是新资产，也不该复制字节 |

如果硬塞进 CAS，会立刻多出三层不该存在的复杂度：

```text
  1. transcript 只是想显示一张现成文件
     -> 却要先读盘
     -> 再 copy 一份进 Rust
     -> 再回吐 hash
     -> 最后 webview 再按 hash 读回来

  这等于给“显示”平白加出一条“复制字节”的支线
```

所以我们刻意把它留在 display-only 路径里：**只改 URL，不搬字节。**

---

## 4. 放大图为什么做成最小可用，而不是第二个预览面板

这里也刻意做窄。

用户在 transcript 里点图，只想“看大一点”，不是想进入完整图片工作流。所以这次放大图只保留三件事：

```text
  - 显示大图
  - Esc 关闭
  - 点遮罩关闭
```

不会做：

```text
  - 缩放级别控制
  - 翻页
  - 下载 / 复制工具栏
  - 复杂状态同步
```

对应实现是 `tomcat-vscode-ext/gui/src/components/ImageLightbox.tsx`：

- `:34` 监听 `Escape`
- `:60` 只在点遮罩时关闭
- `:72` 点弹层空白区关闭、阻断冒泡
- `:91` 点图片本体只 `stopPropagation()`，**不关闭**

这条边界很重要。因为 transcript 里的大图只是“临时放大看一眼”，而独立的图片预览面板仍然是“带工具栏的完整查看器”。

---

## 5. 为什么系统提示词必须一起改

这是这次整改里最容易被低估的一点。

### 5.1 UI 支持，不等于模型会用

在这次改动前，系统提示词只教模型一件事：

```text
  提到文件路径时
    -> 用反引号
```

对应证据：

- `tomcat/src/core/prompts/templates/system/output_conventions.txt:1`
- `tomcat/docs/status/feature-transcript-rich-render.md:32`（当时为了 clickable path，专门新增了 `SystemOutputConventions`）

但它**没有**教模型另一件同样重要的事：

```text
  想让用户直接看到图片时
    -> 用 ![alt](path)
```

这会造成一个必然结果：

```text
  渲染器会画图
  但模型不知道什么时候该用画图语法
  -> 最终大多数时候仍然只输出 `docs/mockup.png`
  -> 用户看到的还是链接，不是图
```

所以这次必须把提示词也一起改进来：

- `tomcat/src/core/prompts/templates/system/output_conventions.txt:3-6`
- `tomcat/src/core/llm/tests/system_prompt_test.rs:120`
- `tomcat/src/core/prompts/tests/load_test.rs:77`

### 5.2 这也是为什么发布顺序不能乱

系统提示词不是扩展运行时去磁盘上读的文本，它是 **CLI 编译进二进制** 的：

- `tomcat/src/core/prompts/mod.rs:1-4`
- `tomcat/src/core/prompts/mod.rs:30-36`

这意味着：

```text
  只发扩展，不发 CLI
    -> 新 UI 已经会画图
    -> 旧 prompt 还不知道 ![...](path)
    -> 功能“理论支持，实际很少触发”

  所以这类改动必须：
    CLI 与扩展同发
    或者先发 CLI，再发扩展
```

这不是发布洁癖，而是能力闭环问题。

### 5.3 mermaid / 表格为什么是这节的反面教材

聊天渲染器早就能画很多东西，比如代码高亮、mermaid、表格。但“能画”从来不等于“模型会稳定输出”。

最典型的例子其实就是 clickable path：直到我们给系统提示词补上明确规范之后，这项能力才从“偶尔碰巧触发”变成“稳定可用”。本次 `![...](path)` 是同一类问题，不是例外。

---

## 6. 什么时候该推翻这个决策

这次方案是故意做窄的。下面三种情况出现时，应该主动重审，而不是继续打补丁。

### 条件一：图片来源不再局限于工作区 / 临时目录

如果未来要支持这些来源：

- MCP 下载下来的临时文件
- 远端 SSH 工作区映射
- 浏览器沙盒内生成、但没有稳定本地路径的图片
- 云端 artifact / issue attachment

那“给一组文件系统根目录授权”就不够了。届时应改成**显式文件 token / 文件 ID 映射**，而不是继续放大 `localResourceRoots`。

### 条件二：一条回答里开始稳定出现很多张大图

当前方案默认可以直接把图插在 transcript 里，因为典型场景就是一两张设计稿。

如果未来变成：

```text
  一条回答 20 张图
  或一个长会话里滚动累积几十张内联图
```

那就该把“立即渲染 `<img>`”改成更强的懒加载/虚拟化策略。否则 DOM 虽然没复制字节，Chromium 位图解码也会重新变成成本。

### 条件三：模型输出从 markdown 走向结构化图片引用

如果未来 serve 或模型协议本身能返回：

```json
{ "type": "local_image", "path": "...", "alt": "..." }
```

那就不该继续拿 markdown 语法当信号。因为那时“图片是图片、路径是路径”已经在协议层区分好了，继续靠 `![...](...)` 只是多绕一层。

---

## 7. 这条能力靠哪些测试守住

```text
  单元测试
    localImages.test.ts
      - 允许工作区内路径
      - 拒绝越界路径
      - 拒绝 http/https/data/blob
      - 处理 /tmp 与 /private/tmp

  组件测试
    ChatMarkdown.test.tsx
      - 只有 ![...](path) 会出图
      - 远程图会降级
      - 点击图会走放大

    ImageLightbox.test.tsx
      - Esc / 遮罩关闭
      - 点图片本体不关闭

  宿主测试
    provider_broadcast.test.ts
      - workspace folder 变化后重算 media roots

  E2E
    image-acceptance.test.ts
      - 工作区真图 naturalWidth > 0
      - 工作区外图片降级成链接
      - lightbox 打开/关闭
```

这四层一起守的核心不是“截图像不像”，而是三条不变量：

1. **只认 `![...](path)`，不误伤反引号路径**
2. **只认已授权根目录，不偷看别处**
3. **只改显示，不复制字节进 Rust**
