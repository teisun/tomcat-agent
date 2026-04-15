# pi_agent_rust 与 pi-mono 扩展系统兼容说明

本文说明 **pi_agent_rust** 如何在「发现规则、多入口包、注册数据形状、验证手段」上与 **pi-mono**（`pi-coding-agent` 的扩展体系）对齐，以及运行时差异与限制。

**仓库内更完整的规范**（PiJS 合约、能力模型、性能目标等）见 `/Users/yankeben/workspace/Tomcat/pi_agent_rust/EXTENSIONS.md`。本文不重复该长文档，只聚焦与 pi-mono 的交叉点。

---

## 目录

1. [总览对照](#1-总览对照)
2. [扩展发现：pi-mono 与 Rust 的对应关系](#2-扩展发现pi-mono-与-rust-的对应关系)
3. [多入口包 `pi.extensions`](#3-多入口包-piextensions)
4. [运行时与 API 兼容面](#4-运行时与-api-兼容面)
5. [注册协议 `RegisterPayload`](#5-注册协议-registerpayload)
6. [静态兼容扫描（可选）](#6-静态兼容扫描可选)
7. [其它形态：Native / WASM](#7-其它形态native--wasm)
8. [Agent 中的加载顺序](#8-agent-中的加载顺序)
9. [如何验证兼容](#9-如何验证兼容)
10. [参考路径索引](#10-参考路径索引)
11. [常见问答（QA）](#qa-runtime)

---

## 1. 总览对照

| 维度 | pi-mono | pi_agent_rust |
|------|---------|---------------|
| 用户扩展语言 | TS/JS（jiti 直跑或可编译进 Bun） | TS/JS 在 **QuickJS（PiJS）** 中执行；无 Node/Bun 全量 API |
| 清单 | `package.json` → `pi.extensions`（字符串或数组） | 同样解析 `pi.extensions`（见下文代码引用） |
| 发现 | `loader.ts`：`cwd/.pi/extensions`、`agentDir/extensions`、配置路径等 | `package_manager` + `resources` 聚合 CLI、配置与自动目录（实现为 **pi-mono 行为的子集**） |
| 注册 | `ExtensionAPI`（`registerTool` / `registerCommand` / `on` 等） | 扩展执行后产生快照，归并为 **`RegisterPayload`** 交给宿主 |
| 强隔离扩展 | 非 WASM 路径为主 | 可选 **WASM 组件**（`wasm-host` feature） |

**架构对照（ASCII）**

```
  pi-mono (参考实现)                         pi_agent_rust (宿主)
+---------------------------+              +---------------------------+
|  coding-agent             |              |  CLI / Agent / TUI        |
|  loader.ts + jiti         |              |  package_manager          |
|  virtualModules (Bun 包)  |    概念对齐   |  resources.rs             |
|  ExtensionAPI (TS 类型)   |  <---------> |  RegisterPayload (JSON)   |
|  Node fs / child_process  |              |  hostcall + 能力策略       |
+---------------------------+              +---------------------------+
          |                                              |
          v                                              v
   真实 Node/Bun 环境                           QuickJS + 连接器 + 审计日志
```

---

## 2. 扩展发现：pi-mono 与 Rust 的对应关系

**pi-mono** 入口逻辑在：

- [pi-mono/packages/coding-agent/src/core/extensions/loader.ts](/Users/yankeben/workspace/Tomcat/pi-mono/packages/coding-agent/src/core/extensions/loader.ts)
- 用户文档：[pi-mono/packages/coding-agent/docs/extensions.md](/Users/yankeben/workspace/Tomcat/pi-mono/packages/coding-agent/docs/extensions.md)

**pi_agent_rust** 侧：

- [pi_agent_rust/src/resources.rs](/Users/yankeben/workspace/Tomcat/pi_agent_rust/src/resources.rs) 文件头注释写明实现 **pi-mono 资源发现行为的子集**（含 skills / prompts / themes / extensions 等管线中的扩展部分）。
- [pi_agent_rust/src/package_manager.rs](/Users/yankeben/workspace/Tomcat/pi_agent_rust/src/package_manager.rs) 负责解析包清单、收集 `extensions` 目录下的自动条目、CLI `-e` 来源等（如 `collect_auto_extension_entries`、`collect_extension_manifest_entries`）。

**`package.json` 中 `pi.extensions` 的解析**（与 pi-mono manifest 字段一致：`pi` 对象下的 `extensions`，可为单个字符串或字符串数组）：

```19325:19388:/Users/yankeben/workspace/Tomcat/pi_agent_rust/src/extensions.rs
fn read_pi_extensions_from_package(package_json_path: &Path) -> Result<Option<Vec<String>>> {
    if !package_json_path.is_file() {
        return Ok(None);
    }
    // ...
    let Some(pi) = json.get("pi") else {
        return Ok(None);
    };
    // ...
    let Some(entries_value) = pi.get("extensions") else {
        return Ok(None);
    };

    match entries_value {
        Value::String(entry) => {
            // ...
            Ok(Some(vec![entry.to_owned()]))
        }
        Value::Array(entries) => {
            // ...
            Ok(Some(out))
        }
        _ => Err(Error::config(format!(
            "Invalid package manifest {}: `pi.extensions` must be a string or array of strings",
            package_json_path.display()
        ))),
    }
}
```

**发现管线（ASCII，逻辑顺序）**

```
                    +------------------+
                    | 配置 / CLI 路径  |
                    +--------+---------+
                             |
                             v
+------------------+   +-----+--------------------+
| 全局 agent 目录   |   | 项目 .pi/extensions 等 |
| extensions/      |   | （与 pi-mono 规则对齐） |
+--------+---------+   +-----------+------------+
         |                         |
         +------------+------------+
                      |
                      v
            +---------------------+
            | package_manager     |
            | 过滤支持的入口类型    |
            +----------+----------+
                       |
                       v
            +---------------------+
            | 合并去重后的入口列表  |
            | -> JsExtensionLoadSpec |
            +---------------------+
```

---

## 3. 多入口包 `pi.extensions`

**问题**：若 `pi.extensions` 声明了多个文件，只执行其中一个入口，会导致同包其它模块内的 `registerTool` / `registerCommand` / `pi.on` 从未运行，行为与 pi-mono 不一致。

**Rust 策略**：对给定主入口，沿目录向上查找祖先 `package.json`，若其中 `pi.extensions` 解析后的路径列表**包含当前入口**，则将该列表中的**全部**解析路径作为关联入口；随后在加载阶段依次执行（并注册扩展根目录供 `readFileSync` 等与路径策略使用）。

核心函数：`discover_related_extension_entries`（节选结构）：

```19731:19812:/Users/yankeben/workspace/Tomcat/pi_agent_rust/src/extensions.rs
fn discover_related_extension_entries(primary: &Path) -> Result<Vec<PathBuf>> {
    let canonical_primary = safe_canonicalize(primary);
    let mut out = vec![canonical_primary.clone()];
    // ... 祖先 package.json 中 pi.extensions，选「包含 primary 且条目最多」的一组 ...
    if !selected_resolved.is_empty() {
        for path in selected_resolved {
            // ... 合并进 out ...
        }
        // ... 可选 workspace bundle 条目 ...
    } else if saw_manifest_extensions {
        return Ok(out);
    }
    // ... 兄弟目录、auxiliary、extensions/ 等补充发现 ...
    Ok(out)
}
```

**多入口加载（ASCII）**

```
  package.json
  pi.extensions: [ "./a.ts", "./b.ts" ]
        |
        |  用户配置或发现只指向 ./a.ts
        v
  discover_related_extension_entries(a.ts)
        |
        |  解析得到 [ a.ts, b.ts ]
        v
  +-----+-----+-----+
  | a.ts | b.ts | ... |
  +--+--+---+---+
     |     |
     v     v
  load   load
  (注册根目录、执行默认导出工厂)
```

---

## 4. 运行时与 API 兼容面

| 能力 | pi-mono | pi_agent_rust |
|------|---------|---------------|
| 模块解析 | Node 解析 + jiti；可访问 `node_modules` | PiJS：**无裸包名解析**、无任意 `node_modules` 遍历；见 `EXTENSIONS.md` PiJS 合约 |
| 内置/虚拟模块 | Bun 二进制内 `virtualModules` 注入若干包 | QuickJS 侧 **shim / polyfill / 虚拟模块**（具体列表与差距见 [CONFORMANCE.md](/Users/yankeben/workspace/Tomcat/pi_agent_rust/CONFORMANCE.md) 扩展一致性章节） |
| IO / 子进程 | 真实 `fs`、`child_process` 等 | **hostcall**：能力策略 + 审计；表面积刻意小于 Node |

**事件与注册（ASCII）**

```
  扩展源码 (TS/JS)
        |
        |  default export (pi) => { ... }
        v
  +-------------------+
  | PiJS: pi.* API    |
  | registerTool/on  |
  +---------+---------+
            |
            |  运行时收集快照
            v
  +-------------------+
  | JsExtensionSnapshot|
  +---------+---------+
            |
            |  可选: PI_EXT_COMPAT_SCAN 静态补齐
            v
  +-------------------+
  | RegisterPayload   |
  +-------------------+
```

---

## 5. 注册协议 `RegisterPayload`

扩展向宿主汇报的统一 JSON 形状（与 pi-mono 侧「注册到 Agent」的概念一一对应：工具、斜杠命令、快捷键、flag、事件钩子等）：

```10457:10476:/Users/yankeben/workspace/Tomcat/pi_agent_rust/src/extensions.rs
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegisterPayload {
    pub name: String,
    pub version: String,
    pub api_version: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capability_manifest: Option<CapabilityManifest>,
    #[serde(default)]
    pub tools: Vec<Value>,
    #[serde(default)]
    pub slash_commands: Vec<Value>,
    #[serde(default)]
    pub shortcuts: Vec<Value>,
    #[serde(default)]
    pub flags: Vec<Value>,
    #[serde(default)]
    pub event_hooks: Vec<String>,
}
```

**宿主消费（ASCII）**

```
RegisterPayload
      |
      +---> 工具表 (built-in + extension tools)
      |
      +---> 斜杠命令 / 快捷键 / flags UI 与状态
      |
      +---> event_hooks -> 生命周期与事件分发
      |
      '---> capability_manifest -> 能力声明与策略
```

---

## 6. 静态兼容扫描（可选）

用于 **测试或元数据补齐**：对关联入口的源文件做正则扫描，识别字面量形式的 `registerCommand('...')` / `registerTool({ name: '...' })` 等，在快照不完整时合并进 `RegisterPayload`（例如斜杠名、工具名占位）。

启用条件：

```29265:29267:/Users/yankeben/workspace/Tomcat/pi_agent_rust/src/extensions.rs
fn compat_static_registration_enabled() -> bool {
    cfg!(feature = "ext-conformance")
        || std::env::var("PI_EXT_COMPAT_SCAN").is_ok_and(|value| parse_truthy_flag(&value))
}
```

**说明**：这不是 pi-mono 生产路径的行为，而是 Rust 侧的 **兼容/诊断辅助**；正常运行仍依赖扩展在 QuickJS 内真实执行并完成注册。

**开关与数据流（ASCII）**

```
  PI_EXT_COMPAT_SCAN=1
        或
  cargo --features ext-conformance
                |
                v
  build_compat_registration_hints(specs)
                |
                v
  正则扫描 *.ts/*.js 源文件
                |
                v
  apply_compat_registration_hints(...)
                |
                v
  合并进 RegisterPayload.tools / slash_commands
```

---

## 7. 其它形态：Native / WASM

- **JS/TS**：与 pi-mono 用户扩展的主对齐路径；入口类型由 `package_manager` 过滤（如 `extension.json`、`.ts`/`.js`、`.native.json`、`.wasm` 等，详见该文件中的 `is_supported_extension_file` 与收集逻辑）。
- **WASM**（`wasm-host` feature）：组件经 WIT 链接，`init(manifest_json)` 返回与 `RegisterPayload` 一致的 JSON，适合新写扩展或强隔离；与 pi-mono「手写 TS 扩展」并存，而非一一替代。

---

## 8. Agent 中的加载顺序

在 [pi_agent_rust/src/agent.rs](/Users/yankeben/workspace/Tomcat/pi_agent_rust/src/agent.rs) 中，扩展加载顺序为：

1. `load_js_extensions`
2. `load_native_extensions`
3. （启用 `wasm-host` 时）`load_wasm_extensions`

随后触发 `startup` 等生命周期钩子（失败为 fail-open，不阻止 Agent 运行）。

```6754:6801:/Users/yankeben/workspace/Tomcat/pi_agent_rust/src/agent.rs
        if !js_specs.is_empty() {
            manager.load_js_extensions(js_specs).await?;
        }

        if !native_specs.is_empty() {
            manager.load_native_extensions(native_specs).await?;
        }

        #[cfg(feature = "wasm-host")]
        if !wasm_specs.is_empty() {
            let host = WasmExtensionHost::new(cwd, resolved_policy.clone())?;
            manager
                .load_wasm_extensions(&host, wasm_specs, Arc::clone(&tools))
                .await?;
        }

        // Fire the `startup` lifecycle hook once extensions are loaded.
        if let Err(err) = manager
            .dispatch_event(
                ExtensionEventName::Startup,
                // ...
            )
            .await
        {
            tracing::warn!("startup extension hook failed (fail-open): {err}");
        }
```

**顺序示意图（ASCII）**

```
  js_specs ------> load_js_extensions
                        |
                        v
  native_specs --> load_native_extensions
                        |
                        v
  wasm_specs ----> load_wasm_extensions  (feature wasm-host)
                        |
                        v
                 dispatch startup / session_start ...
```

---

## 9. 如何验证兼容

权威说明见 [pi_agent_rust/CONFORMANCE.md](/Users/yankeben/workspace/Tomcat/pi_agent_rust/CONFORMANCE.md) 中 **「Extension Conformance (Differential Oracle)」**：同一扩展分别在 **TypeScript 参考（Bun + jiti）** 与 **Rust QuickJS** 下运行，对比 **registration snapshot**。

相关路径（相对 `pi_agent_rust` 仓库根）：

- `tests/ext_conformance_diff.rs`
- `tests/ext_conformance/ts_harness/run_extension.ts`

**差分oracle（ASCII）**

```
        +------------------+
        |  同一扩展源码     |
        +--------+---------+
                 |
     +-----------+-----------+
     |                       |
     v                       v
+-------------+       +-------------+
| TS oracle   |       | Rust PiJS |
| Bun + jiti  |       | QuickJS   |
+------+------+       +------+-----+
       |                     |
       |   registration JSON  |
       +----------+----------+
                  |
                  v
           diff / assert
```

---

## 10. 参考路径索引

| 主题 | 路径 |
|------|------|
| Rust 扩展协议、多入口、`read_pi_extensions_from_package`、compat 扫描 | `/Users/yankeben/workspace/Tomcat/pi_agent_rust/src/extensions.rs` |
| QuickJS 桥与 hostcall 实现细节 | `/Users/yankeben/workspace/Tomcat/pi_agent_rust/src/extensions_js.rs` |
| 包与自动发现 | `/Users/yankeben/workspace/Tomcat/pi_agent_rust/src/package_manager.rs` |
| 资源加载（声明为 pi-mono 子集） | `/Users/yankeben/workspace/Tomcat/pi_agent_rust/src/resources.rs` |
| Agent 内加载与钩子 | `/Users/yankeben/workspace/Tomcat/pi_agent_rust/src/agent.rs` |
| 长文规范与 PiJS 合约 | `/Users/yankeben/workspace/Tomcat/pi_agent_rust/EXTENSIONS.md` |
| 一致性测试策略与扩展结果表 | `/Users/yankeben/workspace/Tomcat/pi_agent_rust/CONFORMANCE.md` |
| pi-mono 加载器 | `/Users/yankeben/workspace/Tomcat/pi-mono/packages/coding-agent/src/core/extensions/loader.ts` |
| pi-mono 扩展用户文档 | `/Users/yankeben/workspace/Tomcat/pi-mono/packages/coding-agent/docs/extensions.md` |

---

<a id="qa-runtime"></a>

## 11. 常见问答（QA）

### Q1：「Node/Bun 环境」是什么？是不是就是一个 JS 解释引擎？

**不完全是。** 日常说的 **Node/Bun 环境**，多半指 **完整的 JavaScript 运行时（runtime）**，而不仅是「解释/执行 JS 源码的那一层」。

- **Node.js**：在 **V8**（Google 的 JavaScript 引擎）之上，加上 **libuv**（事件循环、异步 I/O）以及自带的 **标准库**（如 `fs`、`http`、`child_process`、`path` 等），让 JavaScript 能在本机像「带操作系统能力的脚本平台」一样跑。
- **Bun**：是 **另一个独立产品**（不是 Node 的插件），内置 **JavaScriptCore**（Apple Safari 系使用的 JS 引擎）等组件，并提供自己的运行时 API、包管理、打包器等；与 Node 代码库不同，但常强调对 Node 风格 API 的兼容。

因此更准确的说法是：**运行时 = JS 引擎 + 事件循环/任务调度 + 标准库与原生能力**。「引擎」只是其中负责执行语言的核心一块；把整包 Node 或 Bun 叫作「只是一个解释引擎」会 **低估** 它们自带的系统 API 与生态假设。

---

### Q2：为什么把 Node、Bun 两个名词写在一起？它们是同一个安装程序吗？

写在一起，是因为在 **扩展 / CLI 工具** 语境里，二者都属于同一类：**在服务器或桌面侧运行 TS/JS，并默认附带一大套面向操作系统与网络的 API** 的运行时。文档里用 **「Node/Bun」** 是 **并列举例**，表示「pi-mono 那类参考路径通常建立在这类完整运行时之上」，而不是说它们是一个东西。

**不是同一个安装程序：** 要跑 Node 就安装/使用 **Node 发行版**（`node` 可执行文件）；要跑 Bun 就安装 **Bun**（`bun` 可执行文件）。版本、发布渠道、二进制与行为细节都各自独立，只是常被放在同一句里和 **嵌入式引擎（如 QuickJS）** 对照。

---

### Q3：Node/Bun 和 QuickJS 是什么关系、有什么区别？QuickJS 也是 JS 执行引擎吗？

**QuickJS 首先是 JavaScript 执行引擎（实现 ECMAScript 的小型引擎）**，适合嵌进别的程序（例如用 Rust 宿主驱动）。**pi_agent_rust** 在扩展路径上使用 **QuickJS**，并在其上实现 **PiJS** 合约与 **hostcall**（宿主提供的受控能力），而不是把整个 Node 标准库搬进进程。

关系与区别可概括为：

|  | Node / Bun | QuickJS（+ PiJS / pi_agent_rust 宿主） |
|--|------------|----------------------------------------|
| 定位 | 面向应用的 **完整运行时** | 偏 **嵌入式** 的 **引擎** + 宿主裁剪过的 API |
| 典型能力 | 默认暴露大量 **OS / 网络 / 子进程** 等 API | 侧效应走 **hostcall**，表面积由策略约束 |
| 与 npm 生态 | 强依赖 `node_modules`、动态解析等假设 | PiJS 下模块与解析规则更严（见 `EXTENSIONS.md`） |

**结论：** **QuickJS 是 JS 执行引擎**（pi 项目在其上叠了运行时壳）；**Node/Bun 是「引擎 + 完整 I/O 与工具链」的一整包**。二者不是包含关系，而是不同层级的「跑 JS 的方式」。

**对照（ASCII）**

```
  Node / Bun                              QuickJS (PiJS)
+------------------------+              +------------------------+
| JS 引擎 (V8 / JSC 等)   |              | QuickJS 引擎          |
| + 事件循环 + 标准库     |              | + Pi 宿主事件驱动      |
| + npm / 原生模块生态    |              | + hostcall (受控)    |
+------------------------+              +------------------------+
        完整「应用运行时」                      嵌入宿主内的「语言核心 + 窄 API」
```

---

*文档随代码演进可能滞后；以仓库源码与 `CONFORMANCE.md` 为准。*
