# 测试分层与目录职责

> 适用范围：`tomcat-vscode-ext/` 下的全部测试。
> 写这一页的起因：这个扩展的测试实际分布在**四个位置**，而它们的位置差异不是随意摆放，是**运行时物理上无法合并**。新人（和 Agent）第一次看到会以为是历史遗留想去合并，所以把边界写下来。

**一句话结论**：分四处是因为有四种运行时 —— 假 `vscode` 模块、jsdom 浏览器、真 `tomcat serve` 子进程、真 VS Code Electron 宿主。**没有一个进程能同时是「把 vscode 换成假模块」和「真的 VS Code 宿主」。**

---

## 1. 四个位置一览

| 位置 | 运行器 | 环境 | `vscode` 模块 | 数量 | 跑什么 |
|---|---|---|---|---|---|
| `src/**/*.test.ts` | Vitest | Node | **假的**（stub） | 24 | 扩展宿主纯逻辑 |
| `gui/src/**/*.test.ts(x)` | Vitest | jsdom | 不可用 | 50 | React 组件与 webview 侧逻辑 |
| `tests/**/*.test.ts` | Vitest | Node | **假的**（stub） | 19 | 集成，常 spawn 真实 `tomcat serve` |
| `e2e-harness/` + `src/test/` | Mocha | 真 Electron | **真的** | 5 + 2 | 真 VS Code 宿主里的端到端 |

```text
  ┌──────────────────────────────────────────────────────────────────┐
  │ 快 ←──────────────────────────────────────────────────────→ 慢   │
  │                                                                  │
  │ src/ 单元      gui/ 组件      tests/ 集成      e2e-harness/      │
  │ 毫秒级         毫秒级         秒级             分钟级             │
  │ 纯函数         DOM 断言       真后端协议       真 VS Code         │
  │ 可并行         可并行         maxWorkers 1     串行、要下载       │
  │                                                Electron          │
  └──────────────────────────────────────────────────────────────────┘
```

---

## 2. 为什么不能合并

### 2.1 `src/` 与 `gui/`：工具链互斥

```text
  src/                                gui/
  ├ 扩展宿主，跑在 Node 里             ├ 跑在浏览器沙箱（webview）里
  ├ 依赖 vscode 类型，Node16 module    ├ lib:["DOM"] + JSX
  ├ tsc 编译到 out/                   ├ React / TipTap / mermaid / katex
  └ 根 tsconfig rootDir:"src"         └ Vite 打包到 gui/dist/
          │                                   │
          └──── guiAssets.ts 读 gui/dist 的 ──┘
                Vite HTML，asWebviewUri 注入
```

合并的话，React/DOM 这一大堆浏览器依赖会混进扩展主 `node_modules` 和 VSIX 的 `out/`，`vscode` 类型也会污染前端。`gui/` 是一个独立 npm 包（`tomcat-vscode-ext-gui`），有自己的 `package.json` 与 vitest 环境（`environment: "jsdom"`）。

### 2.2 `src/` 与 `tests/`：同一个运行器，边界靠约定

两者都是 Vitest + Node + 假 `vscode`（`vitest.config.ts` 把 `vscode` alias 到 `tests/stubs/vscode.ts`）。区别是**速度与依赖**：

```text
  src/**/*.test.ts     纯逻辑，无外部进程，maxWorkers 4 并行
  tests/**/*.test.ts   常 spawn 真实 tomcat serve，maxWorkers 1 串行
                       （并行会抢同一个端口 / 同一份 sessions 目录）
```

所以判据是：**要不要起一个真的后端进程**。要 → `tests/`，不要 → `src/`。

### 2.3 `e2e-harness/`：物理上必须是独立目录

这是最容易被误判为「摆错位置」的一个，实际上它必须独立：

```text
  tests/                          e2e-harness/
  ├ Vitest + Node 进程            ├ 一个【真正的 VS Code 扩展】
  ├ vitest.config.ts 把 vscode    │  （有自己的 package.json + main
  │  alias 到 tests/stubs/        │   + 空 activate 实现）
  │  vscode.ts —— 假的 vscode     ├ 作为 @vscode/test-electron 的
  └ 快、纯逻辑、可并行             │  extensionDevelopmentPath
                                  ├ 拉起真实 Electron 宿主，跑 Mocha
                                  └ 被测扩展从 VSIX 装进 --extensions-dir
```

两条硬约束：

1. **同一个进程不可能既把 `vscode` 换成假模块、又是真的 VS Code 宿主。**
2. `runTests({ extensionDevelopmentPath })` 要求指向一个**合法的扩展根** —— 得有 `package.json` + `main`。这就要求它是独立目录，不能是 `tests/` 下的一个子文件夹。

打包时 `scripts/package-vsix.ts` 的 `DISALLOWED_PREFIXES` 已经把 `e2e-harness/` 排除出 VSIX，边界是清楚的。

---

## 3. 我该把新测试写在哪

```text
  改了一个纯函数 / 一个类的行为
    → src/**/*.test.ts

  改了 React 组件的渲染、事件、可访问性
    → gui/src/**/*.test.tsx

  改了 wire 协议、或需要验证「Rust 那边真的这么回」
    → tests/**/*.test.ts（spawn 真实 serve）

  改了只有真 VS Code 才有的东西：
    webview 生命周期、localResourceRoots、asWebviewUri、
    剪贴板、SaveDialog、真实重启后的恢复
    → e2e-harness/src/test/*.test.ts
```

**优先往上面走。** E2E 每条都以分钟计并且要下载 Electron，所以只有当一个断言**在假环境里无法成立**时才放进去。

---

## 4. 契约测试：一类特殊的单元测试

图片附件这次整改引入了一组「契约测试」，它们跑在 `src/` 层但守的是**架构不变量**而不是函数行为。值得单独说明，因为它们的失败信息需要被正确理解。

```text
  src/ui/webview/tests/memory_contract.test.ts
    · postState 快照序列化后的字节数【与附件数量 N 无关】
      → 失败说明有人把图片字节又放回快照了
    · 稳态结构里不出现 dataBase64
    · CSP 与 localResourceRoots 断言

  tomcat 侧（Rust）schema 静态扫描
    · 断言全部 serve 命令里【只有 ingest_attachment】带字节字段
      → 失败说明有人给别的命令加了 base64 参数

  gui/src/attachments/imagePipeline.test.ts
    · 缩略图长边 ≤192px、字节低于阈值、与源 sha 一一对应

  gui/src/attachments/svgSecureMode.test.ts
    · SVG 走 <img> 加载时零网络请求（含 x:href 命名空间别名）
    · 带 style= / <style> / url(#grad) 的真实设计工具 SVG 必须被【接受】
      → 这一条是防「安全黑名单误杀」回归的
```

**为什么用测试而不是注释或文档来守这些：** 这类问题（写放大、内存放大、CSP 配错）在小规模手测里毫无症状 —— 粘一张图什么都正常，粘十一张才炸。code review 也挡不住，因为单看每一处改动都合理。只有把不变量写成断言，回归才会在 CI 上立刻可见。

---

## 5. E2E 断言不能只看一层

安装版 E2E 的两种断言各自回答不同的问题，不能为了让测试更稳定只保留其中一种：

```text
getWebviewState()
  └─ 数据契约：host 是否真的把正确的草稿、附件、引用写进状态？

captureWebviewDom()
  └─ 渲染回归：用户是否真的能在界面上看见这些数据？
```

例如拖放/选择器的用例必须同时断言：

1. `getWebviewState()` 里的 `composerDraft.segments` 与 `pendingAttachments`；
2. DOM 快照里的 `composer-reference-chip` 和对应文件名。

前者能准确定位数据或跨进程协议错误，后者能抓住「状态是对的、组件却没渲染/被样式藏掉」的问题。需要验证用户可见结果的 E2E 一律两层都写；仅验证纯后台副作用时，可只写数据契约并在用例说明原因。

## 6. 五分钟错误恢复的验证边界

```text
真实任务失败 → 宿主收到现场失败 → 有效历史中有 Retry / Resume
                                      ↓
                               宿主等五分钟
                     ┌────────────────┼───────────────┐
                  自动执行         用户立即点击      取消 / 接管
                     └──── 原恢复 RPC ────┘          保留手动按钮
```

请求内的4/10次有限尝试由 Rust 拥有（含首次），每个逻辑模型请求独立计数。这里验证的是之后的恢复阶段：五分钟冷却可无限轮，包括有恢复按钮的账户错误；累计费用和等待没有任务总上限。窗口、宿主或服务生命周期结束就清理，不扫描旧历史重新启动；只切会话、隐藏侧栏、重建网页则保持原截止时间。取消不删错误卡，新输入接管不复活旧等待。

| 测试 | 证明什么 / 不证明什么 |
| --- | --- |
| `src/ui/webview/tests/provider_error_recovery.test.ts` | 注入宿主专用时钟，验证300000ms边界、接管清单、同拍竞争、过期历史、连续失败、权限与生命周期；不伪造整个RPC环境的时间。 |
| `gui/src/components/MessageBubble.test.tsx`、`App.test.tsx`、`stateReconcile.test.ts` | 数字刷新、稳定可访问名称、取消和立即恢复接线、full/sessionView/patch保持同一会话投影；jsdom不能替代视觉验收。 |
| `tests/webview_provider_flow.test.ts` | Provider广播与原Router接线；倒计时只在会话元数据，不写进消息、不新增patch操作。 |
| `tests/serve_error_recovery.test.ts` | 真实Rust服务、Router、RPC、历史与本地脚本模型；仅加速宿主五分钟。Retry保留原输入及附件审计，Resume携带已完成工具结果而不重放，并覆盖相同errorId再次失败、后台不换会话。 |
| `src/test/suite/host.e2e.test.ts` 的 `automatic error recovery countdown` | 真VS Code / Chromium + fake backend；使用生产五分钟等待，无缩短时长开关。验证页面真正销毁重建、取消后键盘恢复、中英文窄宽布局、后台恢复不抢编辑器焦点。不能用fixture截图声称Rust恢复正确。 |

手动恢复回归沿用原 `same-session retry` / `failed tool turn` 场景。时钟助手放在 `src/ui/webview/tests/errorRecoveryClock.ts`，共享给集成测试，但不从 `src/` 导入 `tests/` 外部源码（`tsconfig.rootDir=src`）。

```sh
npm run test:integration -- tests/webview_provider_flow.test.ts tests/serve_error_recovery.test.ts
# TOMCAT_VSIX_VISUAL_ARTIFACTS_DIR 显式指向项目资源目录的 shots/error-recovery-countdown
TOMCAT_E2E_GREP='automatic error recovery countdown' TOMCAT_E2E_SCREENSHOT=1 npm run test:e2e:webview-devhost
```

UI验收须读 PNG、ARIA、console 三类证据：Retry等待、Resume等待、取消后、成功后；中英文均实际渲染，至少桌面与390px窄宽，不做全排列。此场景独立使用360秒超时，不放宽全局。`__testing.reloadWebview()` 不是页面重建证据，使用浏览器真实 `location.reload()` 并验证文档身份变化、deadline不变。

## 7. 常用命令

```bash
npm run lint                      # tsc --noEmit，最快的门禁
npm run test:unit                 # src/ + gui/
npm run test:unit:core            # 只跑 src/
npm run test:unit:gui             # 只跑 gui/
npm run test:integration          # tests/，串行，会 spawn 真实 serve
npm run test:e2e:webview-devhost  # 真 VS Code，Dev Host 模式
npm run test:e2e:webview-install  # 真 VS Code，从 VSIX 安装后跑
npm run gate:fast                 # lint + test:unit
npm run gate:full                 # 全量门禁
npm run accept:image              # 图片附件视觉验收，产物落 artifacts/（不入库）
npm run verify:vsix               # 打包并验证发布物
```

`artifacts/` 与 `.artifacts/` 都在 `.gitignore` 里（两个不同路径，都要有）。验收截图属于一次性证据，贴 PR 描述或对话里，不入库。
