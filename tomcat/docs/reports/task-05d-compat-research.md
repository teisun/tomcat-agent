# TASK-05d 前置调研报告：pi-mono 插件兼容性 — TUI 组件与深度会话 API

> 本报告服务于 TASK-05d（Tier 3-4 TUI 组件 + 深度会话 API 兼容），在实施前解答三个核心问题：
> 1. 扩展依赖哪些 npm 包？需要 shim 哪些 API？
> 2. pi-mono / pi_agent_rust / tomcat 三者的 UI 渲染架构分别是什么？
> 3. tomcat 应采用什么方案处理 npm 包 import？与 pi_agent_rust 的做法有何异同？

---

## Part 1: pi-mono 生态库依赖清单

通过对 `pi-mono/.pi/extensions/` 下全部扩展（diff.ts、files.ts、tps.ts、prompt-url-widget.ts、redraws.ts 等）的源码分析，确认扩展使用的外部包如下：

### 值导入（需提供 shim）

| 包名 | 使用的导出符号 | 使用场景 |
|---|---|---|
| `@mariozechner/pi-tui` | Container, SelectList, Text, DynamicBorder, Key, matchesKey, Box, Editor, truncateToWidth, visibleWidth | diff.ts, files.ts 的 `ctx.ui.custom()` TUI 渲染 |
| `@mariozechner/pi-coding-agent` | DynamicBorder, CustomEditor, truncateHead, truncateTail, formatSize, parseSessionEntries, VERSION | diff.ts, files.ts 的边框/编辑器/文本截断工具 |
| `@mariozechner/pi-ai` | StringEnum, calculateCost, complete, completeSimple, createAssistantMessageEventStream, streamSimpleAnthropic, streamSimpleOpenAIResponses | tps.ts 等扩展的 LLM 调用和模型工具；pi_agent_rust 已提供完整 shim |
| `@sinclair/typebox` | Type (Type.String, Type.Object 等) | 部分扩展的工具参数 schema 定义 |

### 类型导入（SWC 自动 strip，无需 shim）

| 包名 | 使用的类型符号 | 说明 |
|---|---|---|
| `@mariozechner/pi-ai` | ModelInfo, ProviderMetadata 等 | 类型注解，SWC 编译后消失（值导出仍需 shim） |
| `@mariozechner/pi-coding-agent` | ExtensionAPI, ExtensionContext 等 | 类型注解，编译后消失 |

### 结论

需要提供 shim 的 npm 包共 4 个：`@mariozechner/pi-tui`、`@mariozechner/pi-coding-agent`、`@mariozechner/pi-ai`、`@sinclair/typebox`。

参考文档已拷贝到 `Tomcat/pi-mono_docs/` 下：
- `pi-tui-README.md` — Component 接口、TUI 类、渲染机制
- `pi-coding-agent-README.md` — ExtensionAPI、types、DynamicBorder
- `pi-ai-README.md` — Model/Provider 类型（ctx.model/modelRegistry 参考）

---

## Part 2: pi-mono TUI 架构解析

pi-mono 是纯 Node.js 项目，**没有宿主/客体边界**——扩展代码和 TUI 引擎运行在同一个进程中。

### 渲染流程

```
扩展调用 ctx.ui.custom(factory)
     |
     v
factory(tui, theme, keybindings, done) 被调用
     |   直接在同一 Node 进程中执行
     v
返回 Component 对象（实现 render / handleInput 方法）
     |
     v
TUI 引擎 render loop:
     |
     +---> component.render(terminalWidth)
     |         返回 string[] （每行一个字符串）
     |
     +---> process.stdout.write(ansiEscapes + 渲染内容)
     |         差分渲染：只重绘变化的行
     |
     +---> process.stdin 监听键盘事件
     |         component.handleInput(keyData)
     |
     +---> 组件调用 done(result) 退出自定义 UI
```

### 关键特征

- **同进程**：扩展返回的 Component 对象被 TUI 引擎直接调用，无序列化、无 IPC
- **process.stdout**：渲染结果直接写入 Node 进程的 stdout
- **差分渲染**：TUI 引擎对比前后两帧，只重绘变化的行，减少终端闪烁
- **事件驱动**：process.stdin 设为 raw mode，键盘事件逐字符分发给 Component

### 对 tomcat 的影响

这种架构**不可直接移植**到 tomcat，因为：

1. tomcat 的扩展跑在 Wasm 沙箱内，无法访问 `process.stdout` / `process.stdin`
2. Wasm 沙箱和 Rust 宿主之间只能通过 hostcall（JSON 序列化）通信
3. Component 对象（JS 类实例）无法跨越 Wasm/宿主边界传递

---

## Part 3: pi_agent_rust 的做法 — ESM 模块加载器详解

### 技术背景

pi_agent_rust 使用 **rquickjs**（QuickJS 的原生 Rust 绑定），它支持 ECMAScript Modules（ESM）——即 `import ... from "..."` 语法。rquickjs 引擎在遇到 `import` 语句时，会调用 Rust 侧注册的回调来解析和加载模块。

这两个回调由 rquickjs 的两个 trait 定义：

- **`JsModuleResolver::resolve(ctx, base, name)`** — 将 import 说明符（如 `"@mariozechner/pi-tui"`）解析为一个模块标识字符串。pi_agent_rust 在此检查是否命中 `static_virtual_modules` 或 `dynamic_virtual_modules` HashMap。
- **`JsModuleLoader::load(ctx, name)`** — 根据模块标识返回源码字节数组。pi_agent_rust 从 HashMap 中取出 shim 的 JS 源码，调用 `Module::declare()` 编译为 QuickJS 模块。

### 核心实现

`default_virtual_modules()` 函数（extensions_js.rs:7099）创建一个 `HashMap<String, String>`，注册所有需要 shim 的包：

```rust
fn default_virtual_modules() -> HashMap<String, String> {
    let mut modules = HashMap::new();
    modules.insert("@sinclair/typebox".to_string(), r#"
        export const Type = { String: ..., Object: ..., ... };
    "#.to_string());
    modules.insert("@mariozechner/pi-tui".to_string(), r#"
        export class Container { constructor(..._args) {} }
        export class SelectList { ... }
        export const Key = { escape: "escape", enter: "enter", ... };
        export function matchesKey(_data, _key) { return false; }
        ...
    "#.to_string());
    modules.insert("@mariozechner/pi-coding-agent".to_string(), r#"
        export class DynamicBorder { ... }
        export function truncateHead(text, opts) { ... }
        ...
    "#.to_string());
    // ... 更多包
    modules
}
```

shim 内容是**降级实现**：Container 是空壳 class、SelectList 维护基本的 items/selected 状态、matchesKey 始终返回 false。足够让扩展代码不报错地加载和执行，但不做真实 TUI 渲染。

### 架构图

```
pi_agent_rust 架构（运行时 ESM 模块加载）
================================================

原始 .ts 扩展
  含 import { Container } from "@mariozechner/pi-tui"
     |
     v
+----------------------------------------------------+
|  SWC 编译（Rust 层）                                 |
|    - strip TypeScript type annotations              |
|    - 保留 import 语句不动                             |
|    输出: JS（仍含 import ... from "..."）             |
+----------------------------------------------------+
     |
     v
+----------------------------------------------------+
|  rquickjs 引擎加载 JS 模块（ESM 模式）               |
|                                                     |
|  遇到 import "@mariozechner/pi-tui"                 |
|       |                                             |
|       v                                             |
|  PiJsResolver::resolve()                            |
|       |  查询 static_virtual_modules HashMap         |
|       v                                             |
|  +-----------------------------------------------+  |
|  | "@mariozechner/pi-tui" =>                      |  |
|  |   export class Container { ... }               |  |
|  |   export class SelectList { ... }              |  |
|  |   export function matchesKey() { return false }|  |
|  |   export const Key = { escape, enter, ... }    |  |
|  +-----------------------------------------------+  |
|       |                                             |
|       v                                             |
|  PiJsLoader::load()                                 |
|    -> 取出 shim 源码                                 |
|    -> Module::declare() 编译为 QuickJS 模块          |
|    -> import 解析成功                                |
|    -> Container 等绑定到 JS 变量                     |
+----------------------------------------------------+
     |
     v
  扩展正常执行
    new Container() 调用的是 shim 降级类
    ctx.ui.custom(factory) -> hostcall -> Rust 宿主处理
```

### 要点

- import 在**运行时**被 rquickjs 引擎拦截，对扩展源码**零改动**
- shim 以标准 ESM `export` 语法提供，QuickJS 能正确解析绑定关系
- 除了 `@mariozechner/*` 系列，还 shim 了 `@sinclair/typebox`、Node.js 内建模块（fs, path, crypto 等）

---

## Part 4: 方案 C 详解 — tomcat 推荐方案（编译时 import 重写 + globalThis 注入）

### 问题

tomcat 使用 **WasmEdge QuickJS**（编译为 Wasm 运行在沙箱中），采用**脚本模式**——所有 JS 拼接为一个字符串通过 `eval()` 执行。这意味着：

- 没有 ESM 模块加载回调（不支持 `import ... from`）
- JS 运行时遇到 import 语句会直接报语法错误
- 无法像 pi_agent_rust 那样在运行时拦截 import

### 方案 C：两步走

**第一步：SWC 编译时重写 import（ts_compiler.rs 扩展）**

在现有的 SWC 编译流程中新增一个步骤：识别已知 npm 包的 import 语句，将其重写为 `globalThis` 引用。

```
源码：  import { Container, Text } from "@mariozechner/pi-tui";
重写为：var { Container, Text } = globalThis.__pi_tui;
```

```
源码：  import { DynamicBorder } from "@mariozechner/pi-coding-agent";
重写为：var { DynamicBorder } = globalThis.__pi_coding_agent;
```

这个重写发生在 SWC AST 层面，在 strip type annotations 之后、代码生成之前插入一个新的 transform pass。

**第二步：globalThis 前置注入 shim 脚本（instance_wasmedge.rs）**

在 JS 产物拼接阶段，在 pi_bridge.js 之后、扩展 JS 之前，注入 shim 脚本，将降级类/函数挂载到 globalThis 上。

### 架构图

```
tomcat 方案 C 架构（编译时 import 重写）
================================================

原始 .ts 扩展
  含 import { Container } from "@mariozechner/pi-tui"
     |
     v
+----------------------------------------------------+
|  SWC 编译 + import 重写（ts_compiler.rs）            |
|                                                     |
|  1. strip type annotations（已有）                   |
|  2. export default -> __pi_plugin_default（已有）    |
|  3. [新增] 重写已知 npm 包的 import:                 |
|                                                     |
|     import { Container, Text }                      |
|       from "@mariozechner/pi-tui"                   |
|           |                                         |
|           v                                         |
|     var { Container, Text } =                       |
|         globalThis.__pi_tui;                        |
|                                                     |
|  输出: 纯 JS，无任何 import/export 语句              |
+----------------------------------------------------+
     |
     v
+----------------------------------------------------+
|  JS 产物拼接（instance_wasmedge.rs）                 |
|                                                     |
|  +----------------------------------------------+  |
|  | 1. pi_bridge.js                              |  |
|  |    globalThis.pi = { on, exec, ... }         |  |
|  +----------------------------------------------+  |
|  | 2. pi_tui_shim.js  [新增]                    |  |
|  |    globalThis.__pi_tui = {                   |  |
|  |      Container: class { ... },               |  |
|  |      SelectList: class { ... },              |  |
|  |      Text: class { ... },                    |  |
|  |      Key: { escape, enter, up, down, ... },  |  |
|  |      matchesKey: function() { ... },         |  |
|  |    };                                        |  |
|  +----------------------------------------------+  |
|  | 3. pi_coding_agent_shim.js  [新增]           |  |
|  |    globalThis.__pi_coding_agent = {          |  |
|  |      DynamicBorder: class { ... },           |  |
|  |      truncateHead: function() { ... },       |  |
|  |      ...                                     |  |
|  |    };                                        |  |
|  +----------------------------------------------+  |
|  | 4. pi_ai_shim.js  [新增]                     |  |
|  |    globalThis.__pi_ai = {                    |  |
|  |      StringEnum: function() { ... },         |  |
|  |      complete: async function() { ... },     |  |
|  |      calculateCost: function() { ... },      |  |
|  |    };                                        |  |
|  +----------------------------------------------+  |
|  | 5. 编译后的扩展 JS                            |  |
|  |    var { Container, Text } =                 |  |
|  |        globalThis.__pi_tui;                  |  |
|  |    function __pi_plugin_default(pi) {        |  |
|  |      pi.registerCommand("diff", ...);        |  |
|  |    }                                         |  |
|  |    __pi_plugin_default(globalThis.pi);        |  |
|  +----------------------------------------------+  |
|  | 6. __pi_start_event_loop()                   |  |
|  +----------------------------------------------+  |
+----------------------------------------------------+
     |
     v
+----------------------------------------------------+
|  WasmEdge QuickJS 执行（脚本模式）                   |
|                                                     |
|  顺序执行拼接后的脚本：                              |
|  1. globalThis.pi 就位                              |
|  2. globalThis.__pi_tui 就位                        |
|  3. globalThis.__pi_coding_agent 就位               |
|  4. globalThis.__pi_ai 就位                         |
|  5. var {Container} = globalThis.__pi_tui;          |
|     -> Container 绑定到局部变量，成功                 |
|  6. new Container() -> 调用 shim 的降级类            |
|  7. ctx.ui.custom(factory)                          |
|     -> hostCallAsync("context", "uiCustom", ...)    |
|     -> Rust dispatcher 降级处理                      |
+----------------------------------------------------+
```

### 需要修改的文件

| 文件 | 改动 |
|---|---|
| `src/ext/ts_compiler.rs` | 新增 SWC transform pass：匹配已知包的 import 声明，重写为 `var {...} = globalThis.__xxx;` |
| `assets/js/pi_tui_shim.js`（新建） | 提供 `globalThis.__pi_tui` 对象，包含所有 TUI 类/函数的降级实现 |
| `assets/js/pi_coding_agent_shim.js`（新建） | 提供 `globalThis.__pi_coding_agent` 对象 |
| `assets/js/pi_ai_shim.js`（新建） | 提供 `globalThis.__pi_ai` 对象（StringEnum, complete, calculateCost 等） |
| `assets/js/pi_typebox_shim.js`（新建） | 提供 `globalThis.__pi_typebox` 对象（Type.String 等） |
| `src/ext/instance_wasmedge.rs` | 在 `build_combined_script` 中按顺序拼入 shim 脚本 |

---

## Part 5: 对比总结与推荐策略

### pi_agent_rust vs tomcat 方案 C 对比

| 维度 | pi_agent_rust | tomcat 方案 C |
|---|---|---|
| JS 引擎 | rquickjs（原生 Rust 绑定，直接调用 QuickJS C API） | WasmEdge QuickJS（QuickJS 编译为 Wasm 跑在沙箱中） |
| 模块模式 | ESM（支持 `import`/`export`，有模块加载回调） | 脚本模式（所有 JS 拼成一个字符串执行，无 import 支持） |
| import 解析时机 | **运行时**：rquickjs 引擎遇到 import → 调用 Rust 回调 → 返回 shim 源码 | **编译时**：SWC 在 AST 层面将 import 重写为 globalThis 引用 |
| shim 注册方式 | `HashMap<String, String>` 虚拟模块表，运行时按需查询 | globalThis 属性前置注入，脚本拼接时注入到扩展 JS 之前 |
| shim 内容 | 降级 JS class/function（Container 空壳、SelectList 基本状态等） | **相同**（可直接复用 pi_agent_rust 的 shim 实现） |
| 对扩展源码的影响 | 零改动（import 语句保留原样） | 零改动（重写在编译阶段自动完成，扩展开发者无感知） |
| 新增依赖 | 无（rquickjs 原生支持） | 无（SWC 已在依赖树中） |

### 它们的核心区别

两种方案**效果完全一致**——扩展开发者感知不到区别，`new Container()` 调用的都是 shim 降级类。区别仅在于底层机制适配了各自 JS 引擎的能力：

- pi_agent_rust 能在**运行时拦截 import**（因为 rquickjs 有 ESM 模块加载器回调），所以 import 语句可以原封不动保留
- tomcat 只能在**编译时重写 import**（因为 WasmEdge QuickJS 用脚本模式执行，无模块加载回调），所以需要 SWC 把 import 改成 globalThis 引用

### 推荐策略

**效仿 pi_agent_rust 的理念（为 npm 包提供 shim 降级实现），适配 tomcat 自身引擎约束（SWC 编译时 import 重写 + globalThis 注入）。**

具体而言：

1. **shim 内容直接参考 pi_agent_rust** 的 `default_virtual_modules()` 实现（extensions_js.rs:7099-12914），将其中的 ESM `export` 语法改写为赋值到 `globalThis.__pi_xxx` 的形式
2. **import 重写逻辑**在 ts_compiler.rs 中实现，维护一个包名到 globalThis 属性名的映射表
3. **shim 脚本**作为独立的 .js 文件放在 `assets/js/` 下，由 `instance_wasmedge.rs` 在拼接阶段注入
4. **TUI 渲染**在当前阶段采用降级策略（shim 类为空壳），`ctx.ui.custom()` 通过 hostcall 交给 Rust dispatcher 处理（日志输出 + 确定性返回），后续可根据需要升级为 `dialoguer`/`crossterm` 真实终端交互
