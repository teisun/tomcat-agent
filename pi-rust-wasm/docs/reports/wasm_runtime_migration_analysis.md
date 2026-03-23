# WasmEdge → Wasmtime + quickjs-ng 迁移分析报告

> 日期：2026-03-22（更新：2026-03-23）
> 项目：pi-rust-wasm
> 范围：从 WasmEdge + wasmedge-quickjs 迁移至 Wasmtime + quickjs-ng WASI

---

## 目录

1. [方案筛选与排除理由](#1-方案筛选与排除理由)
2. [架构匹配性分析与方案修正](#2-架构匹配性分析与方案修正)
3. [quickjs-ng WASI 与当前 WasmEdge 方案对比](#3-quickjs-ng-wasi-与当前-wasmedge-方案对比)
4. [Node.js API 兼容性分析与补齐方案](#4-nodejs-api-兼容性分析与补齐方案)
5. [当前 WasmEdge 集成范围（迁移影响面）](#5-当前-wasmedge-集成范围迁移影响面)
6. [运行时适配层设计](#6-运行时适配层设计)
7. [完整工作项清单](#7-完整工作项清单)
8. [推荐实施路径](#8-推荐实施路径)

---

## 1. 方案筛选与排除理由

初始评估了四个候选方案（含 Javy），最终推荐 quickjs-ng WASI 标准构建。

### 1.1 方案 B — Boa（纯 Rust JS 引擎）：已排除

决策者明确放弃，不再纳入分析。主要原因：

- 完全去掉 Wasm 沙箱层，安全隔离需自行实现
- ECMAScript 合规率 ~94%，无法保证社区插件兼容
- 架构变化过大，与当前 QuickJS 生态不兼容

### 1.2 方案 C — Wasmtime 加载 wasmedge-quickjs.wasm：技术不可行

基于 `wasmedge-quickjs` 源码分析，该 Wasm 模块存在三层 WasmEdge 深度耦合，无法在 Wasmtime 上运行。

#### 证据一：网络层 — WasmEdge 专有 WASI Socket 扩展

`wasmedge_wasi_socket` 是 SecondState 为 WasmEdge 开发的非标准 WASI 扩展 crate。Wasmtime 不提供此接口。

涉及文件：

| 文件 | 依赖方式 |
|------|----------|
| `Cargo.toml` | `wasmedge_wasi_socket = { version = "0.5", features = ["wasi_poll"] }` |
| `src/event_loop/mod.rs` | `wasmedge_wasi_socket::TcpListener::bind()` — 用于 `AsyncTcpServer` |
| `src/internal_module/wasi_net_module.rs` | `wasmedge_wasi_socket::ToSocketAddrs` — TCP/TLS 连接 |

#### 证据二：异步运行时 — SecondState WasmEdge 分叉

`Cargo.toml` 使用 `[patch.crates-io]` 将 tokio、mio、socket2 三个核心异步运行时 crate 替换为 SecondState 维护的 WasmEdge 专用分叉：

```toml
[patch.crates-io]
tokio = { git = "https://github.com/second-state/wasi_tokio.git", branch = "v1.40.x" }
mio = { git = "https://github.com/second-state/wasi_mio.git", branch = "v1.0.x" }
socket2 = { git = "https://github.com/second-state/socket2.git", branch = "v0.5.x" }
```

这些分叉内部依赖 WasmEdge 特有的 WASI 扩展 API（如异步 I/O、poll），标准 WASI Preview 1/2 实现无法满足其系统调用要求。

#### 证据三：C 库层 — 预编译静态库绑定

- `lib/libquickjs.a`：为 WasmEdge 的 `wasm32-wasi` 环境预编译的 QuickJS C 库
- `build.rs`：直接链接该静态库（`cargo:rustc-link-lib=quickjs`）
- `lib/binding.rs`：由 `rust-bindgen 0.68.1` 生成的 QuickJS C FFI 绑定

这是编译期和链接期的硬依赖，运行时替换无法解决。

#### 结论

wasmedge-quickjs.wasm 在 WasmEdge 专有扩展（网络、异步）、分叉运行时（tokio/mio/socket2）、预编译 C 库三个层面与 WasmEdge 深度绑定。无法通过"换个运行时加载同一个 .wasm"来解决。

### 1.3 方案 D — Javy（Bytecode Alliance JS → Wasm 编译器）：架构不匹配

Javy 是 Bytecode Alliance 出品的 JavaScript → WebAssembly 编译器，核心设计为 **构建时预编译**：

```
javy compile user.js → user.wasm（内嵌 QuickJS 字节码）
```

与 pi-rust-wasm 当前架构存在根本性冲突：

| 维度 | pi-rust-wasm 当前架构 | Javy 的模式 |
|------|----------------------|-------------|
| JS 加载 | **运行时** — 插件发现后动态读取 | **构建时** — 必须预编译为 .wasm |
| JS 组合 | `build_combined_script` 运行时拼接 bridge + 7 shim + 用户代码 | 构建时固化到字节码 |
| TS 支持 | SWC 运行时转译 → eval | 需先转译再 `javy compile` |
| 插件热加载 | 直接 eval 新 JS | 需重新 `javy compile` |

排除原因：

1. 插件在运行时从磁盘发现和加载，无法预编译
2. `build_combined_script` 的拼接结果在运行时才确定
3. 每次加载插件需调用 `javy compile` CLI，引入秒级延迟和外部依赖
4. Javy 不支持运行时 eval 任意 JS 文本（仅执行预编译字节码）

详见第 2.1 节的完整分析。

---

## 2. 架构匹配性分析与方案修正

### 2.1 Javy 的架构不匹配问题

**Javy 是一个 JS → Wasm 编译器，不是运行时 JS 解释器。**

Javy 的设计模式：

```
构建时: user.js ──► javy compile ──► user.wasm (内含 QuickJS 字节码)
运行时: Wasmtime 加载 user.wasm ──► 执行预编译字节码
```

pi-rust-wasm 的当前架构：

```
运行时: Wasmtime 加载 quickjs.wasm (通用 JS 运行时)
       ──► quickjs.wasm 读取 user.js 文件
       ──► eval 执行（运行时解释）
```

关键差异：

| 维度 | 当前架构（wasmedge-quickjs） | Javy |
|------|------------------------------|------|
| JS 加载时机 | **运行时** — 插件发现后动态读取 JS | **构建时** — JS 必须预编译为 .wasm |
| JS 组合方式 | 运行时拼接 bridge + shims + 用户代码 | 构建时固化到字节码中 |
| 插件热加载 | 直接 eval 新 JS 文件 | 需要重新 `javy compile` 生成 .wasm |
| wasm 模块数量 | 1 个通用 `quickjs.wasm`，所有插件共享 | 每个插件一个 .wasm |
| `__pi_host_call` 注册 | 在 quickjs.wasm 的 Wasm 侧 C/Rust 代码中 | 需构建自定义 Javy Plugin |

**Javy 不适合 pi-rust-wasm 的原因**：

1. 插件在运行时从磁盘发现和加载，不能要求用户预编译
2. `build_combined_script` 动态拼接 bridge + 7 个 shim + 用户脚本 + main_loop，这个组合在运行时才确定
3. TypeScript 插件通过 SWC 在运行时转译，转译结果不能预知
4. 每次加载插件都要调用 `javy compile` 会引入秒级延迟和外部 CLI 依赖

### 2.2 正确方案：Wasmtime + quickjs-ng WASI

**quickjs-ng**（https://github.com/quickjs-ng/quickjs）是 QuickJS 的社区活跃分支，官方提供标准 WASI 编译，与当前架构完美匹配。

| 属性 | 详情 |
|------|------|
| 项目 | quickjs-ng/quickjs |
| 定位 | QuickJS 的活跃维护分支（原 Fabrice Bellard 版已停更） |
| WASI 支持 | 官方提供 `qjs-wasi.wasm`（command）和 `qjs-wasi-reactor.wasm`（reactor） |
| 构建工具 | wasi-sdk + CMake，标准 WASI，无任何 WasmEdge 依赖 |
| 运行时兼容 | Wasmtime、Wasmer、WAMR 等任意 WASI 运行时 |
| ECMAScript | ES2023 合规（持续更新） |

**架构对比**：

```
当前: WasmEdge ──加载──► wasmedge_quickjs.wasm ──读取 eval──► user.js
迁移: Wasmtime ──加载──► qjs-wasi.wasm         ──读取 eval──► user.js
                         ^^^^^^^^^^^^^^^^^^^
                         quickjs-ng 官方构建，标准 WASI
```

**迁移后的执行流程与当前完全一致**：宿主加载一个通用的 QuickJS wasm → 通过 WASI argv 传入脚本路径 → QuickJS 读取并 eval JS 文件。`build_combined_script`、shim 注入、TypeScript 转译等所有上层逻辑零改动。

### 2.3 quickjs-ng 的两种 WASI 模式

**Command 模式（`qjs-wasi.wasm`）— 直接替代当前 wasmedge_quickjs.wasm**

```
wasmtime --dir . qjs-wasi.wasm -- script.js
```

- 与当前 `wasmedge_quickjs.wasm` 行为一致：argv 接收 JS 文件路径，执行后退出
- `_start` 入口，WASI 标准 I/O
- 当前 `run_script_file` / `init_vm` + `run_func("_start")` 流程直接适用

**Reactor 模式（`qjs-wasi-reactor.wasm`）— 更适合长生命周期 VM**

```
qjs_init_argv() → qjs_get_context() → JS_Eval() → ... → qjs_destroy()
```

导出四个函数：

| 函数 | 作用 |
|------|------|
| `qjs_init()` / `qjs_init_argv()` | 初始化 QuickJS Runtime + Context |
| `qjs_get_context()` | 获取持久 JSContext 指针 |
| `JS_Eval()` | 在同一 Context 上反复 eval JS |
| `qjs_destroy()` | 销毁 Runtime |

Reactor 模式的优势：

- **持久状态**：Context 跨调用保持，全局变量/注册的命令不丢失
- **无重初始化开销**：init 一次，多次 eval
- **天然适配长生命周期 VM**：当前 `VmActor` 的 `init_vm` → `run_func("_start")` 模式可改为 `qjs_init` → 反复 `JS_Eval`

### 2.4 推荐：分阶段采用

| 阶段 | 模式 | 说明 |
|------|------|------|
| 第一步 | **Command 模式** | 最小改动：用 `qjs-wasi.wasm` 替代 `wasmedge_quickjs.wasm`，上层代码几乎不动 |
| 第二步（可选） | **Reactor 模式** | 优化长生命周期 VM：VmActor 改为 init → 反复 eval，消除每次 _start 的重初始化 |

### 2.5 宿主函数 `__pi_host_call` 的适配

quickjs-ng 原生 WASI 构建不包含 `__pi_host_call` import。需要构建一个定制版本：

**方案 A（推荐）：定制 quickjs-ng 构建**

在 quickjs-ng 源码基础上添加 `pi_host_call.c`：

```c
// pi_host_call.c — 与 wasmedge-quickjs/src/host_call.rs 等价的 C 实现
__attribute__((import_module("env"), import_name("__pi_host_call")))
extern int __pi_host_call(char* buf, int req_len, int buf_cap);

// 注册为 JS 全局函数，逻辑与当前 host_call.rs 中的 PiHostCallFn 一致
```

编译时链接到 qjs，产出 `pi-qjs-wasi.wasm`。这是一次性工作，之后与 quickjs-ng 上游保持同步只需 rebase。

**方案 B：纯宿主侧注入**

不修改 quickjs-ng 源码，而是在 Wasmtime 宿主侧通过 `Linker` 注入 `__pi_host_call`。quickjs-ng 的 Wasm 模块需要 import 这个函数，所以仍需修改 quickjs-ng 的 C 代码添加 `extern` 声明。本质上与方案 A 相同。

**宿主侧（Rust）注册方式**（两个方案相同）：

```rust
linker.func_wrap(
    "env",
    "__pi_host_call",
    |mut caller: Caller<'_, HostState>, buf_ptr: i32, req_len: i32, buf_cap: i32| -> i32 {
        let memory = caller.get_export("memory").unwrap().into_memory().unwrap();
        // 读取请求 → 调用宿主回调 → 写回响应
        // 逻辑与当前 instance_wasmedge.rs 中的 host_call_impl 完全一致
    },
)?;
```

ABI 签名 `(buf_ptr: i32, req_len: i32, buf_cap: i32) -> i32` 保持不变，JS 侧 `pi_bridge.js` 无需修改。

### 2.6 异步/Promise/setTimeout 支持

quickjs-ng 内置 Promise 和 async-await 支持（QuickJS 原生能力）。当前方案中 Promise 的驱动方式：

- **Command 模式**：与当前一致 — `pi_main_loop.js` 的 `waitForEvent` 轮询驱动事件循环，QuickJS 在脚本执行期间自动处理 pending Promise
- **Reactor 模式**：宿主在每次 `JS_Eval()` 后调用 `JS_ExecutePendingJob()` 驱动 Promise 队列，更精细可控

`pi_main_loop.js` 和 `__session.waitForEvent` 轮询机制完全可以复用。

---

## 3. quickjs-ng WASI 与当前 WasmEdge 方案对比

| 维度 | 当前（WasmEdge + wasmedge-quickjs） | 迁移后（Wasmtime + quickjs-ng WASI） |
|------|--------------------------------------|---------------------------------------|
| **执行模式** | 通用 QuickJS.wasm + 运行时 eval JS | 同：通用 qjs-wasi.wasm + 运行时 eval JS |
| **冷启动延迟** | ~8.1 ms（WasmEdge）+ QuickJS 初始化 | ~5.2 ms（Wasmtime）+ QuickJS 初始化 |
| **执行性能** | QuickJS 解释执行 | 同为 QuickJS 解释执行，JS 层面性能一致 |
| **内存占用** | ~20 MB 基础 + Wasm 线性内存 | ~15 MB 基础 + Wasm 线性内存 |
| **沙箱隔离** | WasmEdge WASI 沙箱 | Wasmtime capability-based 沙箱，更规范 |
| **WASI 合规** | WasmEdge 专有扩展（非标准 socket 等） | 标准 WASI Preview 1（wasi-sdk 编译） |
| **async/Promise** | wasmedge-quickjs Rust event loop | QuickJS 内置 + `pi_main_loop.js` 轮询（不变） |
| **社区活跃度** | wasmedge-quickjs：SecondState，更新减缓 | quickjs-ng：社区活跃分支，持续更新 |
| **Rust 宿主依赖** | `wasmedge-sdk`（需安装 C++ 库） | `wasmtime` crate（纯 Rust，cargo build 即可） |
| **QuickJS 更新频率** | 锁定旧版 QuickJS | quickjs-ng 跟进 ES2023+，持续合规改进 |
| **Reactor 模式** | 不支持 | 官方 `qjs-wasi-reactor.wasm`，init-once/eval-many |
| **上层代码改动** | — | `build_combined_script`、shim 注入、TS 转译：**全部不变** |

### 关键优势总结

1. **架构完全匹配**：同为"通用 QuickJS.wasm + 运行时 eval"模式，`build_combined_script` 和所有 JS shim 零改动
2. **去除外部 C/C++ 构建依赖**：Rust 宿主侧只依赖 `wasmtime` crate，纯 `cargo build`
3. **性能提升**：Wasmtime 冷启动快 ~36%、内存低 ~25%
4. **标准合规**：quickjs-ng 用 wasi-sdk 编译，只依赖标准 WASI 接口
5. **Reactor 模式可选**：未来可优化长生命周期 VM，消除每次 `_start` 的重初始化开销

### 潜在风险

1. **需定制构建 quickjs-ng**：加入 `__pi_host_call` import 声明和 JS 全局注册（一次性工作，C 代码量 ~80 行）
2. **宿主函数注册方式变化**：从 `ImportObjectBuilder` 迁移到 `wasmtime::Linker`，API 差异显著但逻辑一致
3. **quickjs-ng 上游同步**：定制 fork 需与上游保持 rebase，但改动集中在独立文件（`pi_host_call.c`），冲突概率低

---

## 4. Node.js API 兼容性分析与补齐方案

### 4.1 wasmedge-quickjs 内置的 Node API 模块

通过源码分析 `wasmedge-quickjs/src/internal_module/` 和 `wasmedge-quickjs/src/quickjs_sys/mod.rs` 的初始化序列，wasmedge-quickjs 在 C/Rust 层内置了以下模块：

| # | JS 模块名 | 实现文件 | 导出的 API | 依赖 WasmEdge 专有功能 |
|---|-----------|----------|------------|----------------------|
| 1 | **`_node:fs`** | `internal_module/fs.rs` (1007行) | `statSync`, `lstatSync`, `fstatSync`, `mkdirSync`, `rmdirSync`, `rmSync`, `renameSync`, `truncateSync`, `ftruncateSync`, `realpathSync`, `copyFileSync`, `linkSync`, `symlinkSync`, `utimeSync`, `lutimeSync`, `futimeSync`, `fcloseSync`, `fsyncSync`, `fdatasyncSync`, `freadSync`, `fread`(async), `openSync`, `readlinkSync`, `fwriteSync`, `fwrite`(async), `freaddirSync` | **否** — 使用标准 WASI fd/path 系统调用（`wasi_fs` 封装） |
| 2 | **`_node:crypto`** | `internal_module/crypto.rs` (598行) | `timing_safe_equal`, `random_fill`, `pbkdf2_sync`, `scrypt_sync`, `hkdf_sync`, `gen_keypair`, `JsHash`(create/update/digest), `JsHmac`(create/update/digest), `JsCipher`(createCipheriv/createDecipheriv/update/final/setAAD/getAuthTag), `JsKeyObjectHandle`(export) | **是** — 依赖 `crypto_wasi` crate（WASI crypto 提案，非标准） |
| 3 | **`_node:os`** | `internal_module/os.rs` (22行) | `_memorySize` | **否** — 使用 `wasm32::memory_size` |
| 4 | **`_encoding`** | `internal_module/encoding.rs` (313行) | `text_encode`, `text_decode`, `text_encode_into` — TextEncoder/TextDecoder 等价实现，支持 30+ 编码（UTF-8/GBK/Big5/ISO-8859-x/Windows-125x 等） | **否** — 纯 Rust `encoding` crate |
| 5 | **`wasi_net`** | `internal_module/wasi_net_module.rs` (500行) | `WasiTcpServer`(bind/accept), `WasiTcpConn`(connect/read/write/end), `WasiTlsConn`(connect), `nsloopup` | **是** — 深度依赖 `wasmedge_wasi_socket` + SecondState tokio/mio fork |
| 6 | **`wasi_http`** | `internal_module/httpx/` (~800行) | `Buffer`(append/write/parseRequest/parseResponse/parseChunk), `WasiRequest`(encode/getHeader/setHeader), `WasiResponse`(encode/chunk), `ChunkResponse`(write/end), `URL`(href/scheme/host/port/path/query) | **部分** — `URL` 类纯 Rust (`url` crate)；HTTP 解析纯 Rust；但网络 I/O 依赖 `wasi_net` |
| 7 | **全局函数** | `internal_module/core.rs` (187行) | `setTimeout`, `clearTimeout`, `setImmediate`, `nextTick`, `sleep`, `exit`, `env`(环境变量对象) | **部分** — `setTimeout`/`sleep` 依赖 `tokio::time`（SecondState fork） |
| 8 | **QuickJS 内置** | `quickjs_sys/mod.rs` | `std`(QuickJS 标准库), `qjs:os`(QuickJS os 模块) | **否** — QuickJS 原生 |

### 4.2 pi-rust-wasm JS shim 层覆盖情况

`pi_node_shim.js` 在 JS 层提供了一套 **stub/mock 实现**，挂载到 `globalThis.__node_*`：

| globalThis 属性 | 覆盖的 Node 模块 | 实现方式 | 功能程度 |
|-----------------|-----------------|----------|---------|
| `__node_fs` | `fs` | stub：所有方法返回空值/false | **空壳** — `existsSync→false`, `readFileSync→''`, `statSync→{isFile:false}` |
| `__node_fs_promises` | `fs/promises` | stub：所有方法返回 `Promise.resolve()` | **空壳** |
| `__node_path` | `path` | 真实实现 | **功能完整** — `join`, `resolve`, `dirname`, `basename`, `extname`, `sep`, `delimiter` |
| `__node_child_process` | `child_process` | mock：spawn 返回 MockEventEmitter | **空壳** — 进程立即 emit close/exit(0) |
| `__node_os` | `os` | stub：返回固定值 | **空壳** — `platform→'linux'`, `arch→'x64'` 等硬编码 |
| `__node_crypto` | `crypto` | 极简实现 | **部分** — `randomUUID`(Math.random), `randomBytes`, `createHash`(stub) |

### 4.3 三层对比：wasmedge-quickjs 内置 vs JS shim vs quickjs-ng 原生

| API | wasmedge-quickjs 内置 | pi_node_shim.js | quickjs-ng 原生 | 迁移后需补齐 |
|-----|----------------------|-----------------|-----------------|-------------|
| **fs 同步操作** | `_node:fs` — 26 个低级 WASI 函数 | stub（空壳） | 无 | **需补齐**（见 4.4） |
| **fs 高级 API** | 无（`readFileSync`/`writeFileSync` 等不在 `_node:fs` 中） | stub（空壳） | 无 | 需要 JS 封装层将低级 WASI fd 操作包装为 Node 风格 API |
| **path** | 无 | **完整实现** | 无 | **不需要** — shim 已覆盖 |
| **crypto** | `_node:crypto` — Hash/Hmac/Cipher/pbkdf2/scrypt/hkdf/keypair | stub（空壳） | 无 | **需补齐**（见 4.4） |
| **os** | `_node:os` — 仅 `_memorySize` | stub（硬编码值） | 无 | **低优先级** — shim 的硬编码值对插件足够 |
| **encoding** | `_encoding` — TextEncoder/TextDecoder（30+ 编码） | 无 | 无 | **需补齐**（见 4.4） |
| **net (TCP)** | `wasi_net` — TCP Server/Client/TLS | 无 | 无 | **不需要** — pi-rust-wasm 不使用 TCP，网络通过 `__pi_host_call` 代理 |
| **http** | `wasi_http` — HTTP 解析/Buffer/URL | 无 | 无 | **部分需要** — `URL` 类如果被插件使用则需补齐 |
| **setTimeout/clearTimeout** | 全局 — tokio 异步实现 | 无（依赖内置） | 无 | **需补齐**（见 4.4） |
| **setImmediate/nextTick** | 全局 — 事件循环集成 | 无（依赖内置） | 无 | **需补齐** |
| **process.env** | 全局 `env` 对象 | 无 | 无 | **需补齐** |
| **process.exit** | 全局 `exit()` | 无 | 无 | **可选** — `std::process::exit()` 可在 C 层实现 |
| **child_process** | 无 | mock（空壳） | 无 | **不需要** — WASI 沙箱不支持进程创建，mock 足够 |

### 4.4 补齐方案

quickjs-ng 是纯 QuickJS JS 引擎，不内置任何 Node.js 兼容层。需要补齐的模块有两种实现路径：

**路径 A：在定制 quickjs-ng 构建的 C 代码中实现（与 wasmedge-quickjs 对齐）**

适用于需要系统调用或高性能的模块：

| 模块 | C 实现方式 | 工作量 | 说明 |
|------|-----------|--------|------|
| `_node:fs` | 调用标准 WASI fd/path 系统调用 | **中（2-3天）** | 可从 wasmedge-quickjs 的 `fs.rs` 移植逻辑，改写为 C。标准 WASI 调用，Wasmtime 全部支持 |
| `_encoding` | 链接 C 编码库或用 QuickJS 内置的 UTF-8 | **低（1天）** | TextEncoder/TextDecoder 的 UTF-8 部分 QuickJS 可原生处理；其他编码可用 `iconv` 或精简实现 |
| `setTimeout`/`clearTimeout` | 无法在 Command 模式下真正异步 | **无需** | 当前 pi-rust-wasm 通过 `pi_main_loop.js` 的轮询机制实现 setTimeout，不依赖 C 层 timer |
| `process.env` / `exit` | WASI `environ_get()` + `proc_exit()` | **低（0.5天）** | 标准 WASI 调用 |

**路径 B：在 JS shim 层实现（扩展 `pi_node_shim.js`）**

适用于可以纯 JS 实现或只需 stub 的模块：

| 模块 | JS 实现方式 | 工作量 | 说明 |
|------|-----------|--------|------|
| `path` | 已有完整实现 | **0** | `pi_node_shim.js` 已覆盖 |
| `os` | 已有 stub | **0** | 硬编码值对插件足够 |
| `child_process` | 已有 mock | **0** | WASI 不支持进程创建 |
| `crypto` (基础) | 扩展 shim | **低（0.5天）** | `randomUUID`/`randomBytes` 可改为调用 WASI `random_get`；`createHash` 需要 C 层支持或用纯 JS 实现 |
| `URL` | JS polyfill | **低（0.5天）** | 可用社区 `whatwg-url` 轻量实现或精简版 |
| `setTimeout`/`setImmediate` | 当前已通过 `pi_main_loop.js` 机制运作 | **0** | 不需要额外处理 |

**路径 C：通过 `__pi_host_call` 代理到宿主实现**

适用于 WASI 沙箱无法实现但宿主（Rust 侧）有能力提供的：

| 模块 | 代理方式 | 说明 |
|------|---------|------|
| `crypto` (高级) | `__pi_host_call({action:"crypto.hash",...})` | Hash/Hmac/Cipher 等可由 Rust 侧 `ring`/`sha2` 等 crate 实现，通过 host call 桥接 |
| `fs` (高级) | `__pi_host_call({action:"fs.readFile",...})` | `readFileSync`/`writeFileSync` 等高级 API 可由宿主代理 |

### 4.5 推荐策略与优先级

```
优先级 1（迁移必须）：
  ├─ _node:fs — C 层移植（标准 WASI，从 wasmedge-quickjs 的 fs.rs 翻译）
  ├─ setTimeout/setImmediate — 已通过 pi_main_loop.js 覆盖，无需额外工作
  ├─ process.env / exit — C 层，标准 WASI 调用
  └─ __pi_host_call — 已在阶段二中规划

优先级 2（按需补齐）：
  ├─ _encoding — C 层 UTF-8 编码/解码（其他编码按需添加）
  ├─ crypto 基础（randomUUID/randomBytes）— JS shim 或 WASI random_get
  └─ URL — JS polyfill

优先级 3（可通过 host call 代理）：
  ├─ crypto 高级（Hash/Hmac/Cipher）— Rust 侧 ring/sha2 + host call 桥接
  └─ fs 高级 API（readFileSync/writeFileSync）— Rust 侧 + host call 桥接

不需要补齐：
  ├─ wasi_net (TCP/TLS) — pi-rust-wasm 不直接使用，网络通过 host call 代理
  ├─ wasi_http — HTTP 解析在 Rust 宿主侧处理
  ├─ path — JS shim 已完整实现
  ├─ os — JS shim 硬编码值足够
  └─ child_process — WASI 沙箱不支持，mock 足够
```

### 4.6 对工作项的影响

需要在阶段二"定制 quickjs-ng WASI 构建"中增加 Node API 兼容层工作：

| # | 新增工作项 | 工作量 | 说明 |
|---|-----------|--------|------|
| 2.6 | 移植 `_node:fs` 模块（C 实现） | 2-3 天 | 将 wasmedge-quickjs `fs.rs` 的 26 个 WASI 函数翻译为 C，注册为 `_node:fs` QuickJS 模块 |
| 2.7 | 移植 `process.env`/`exit` 全局函数 | 0.5 天 | C 层 WASI `environ_get` + `proc_exit` |
| 2.8 | 移植 `_encoding` 模块（C 实现） | 1 天 | UTF-8 编解码优先；其他编码按需 |
| 2.9 | 扩展 `pi_node_shim.js` crypto 基础 | 0.5 天 | `randomUUID` 改用 WASI random_get 或保持现有 Math.random 实现 |

阶段二总工时从 2-3 天调整为 **4-6 天**。

---

## 5. 当前 WasmEdge 集成范围（迁移影响面）

### 4.1 直接依赖 WasmEdge SDK 的文件

| 文件 | 行数 | WasmEdge API 使用 | 迁移动作 |
|------|------|-------------------|----------|
| `Cargo.toml` | 55 | `wasmedge-sdk = "0.13.5-newapi"` | 替换为 `wasmtime`、`wasmtime-wasi` |
| `src/ext/engine_wasmedge.rs` | 70 | `Config`、`ConfigBuilder`、`CommonConfigOptions`、`RuntimeConfigOptions`、`StatisticsConfigOptions` | 重写为 `wasmtime::Engine` + `wasmtime::Config` |
| `src/ext/instance_wasmedge.rs` | 440 | `Vm`、`Store`、`Module`、`WasiModule`、`ImportObjectBuilder`、`CallingFrame`、`WasmValue`、`Instance` | 重写为 `wasmtime::Store`、`Module`、`Linker`、`wasmtime_wasi::WasiCtxBuilder` |
| `src/ext/mod.rs` | 30 | `pub use engine_wasmedge::WasmEngine`、`pub use instance_wasmedge::WasmInstance` | 更新为新模块名 |

### 4.2 间接引用 WasmEdge 的文件

| 文件 | 行数 | 引用方式 | 迁移动作 |
|------|------|----------|----------|
| `src/ext/vm_actor.rs` | 251 | 调用 `WasmInstance::init_vm()` 和 `Vm::run_func()` | 适配新 API 签名 |
| `src/infra/error.rs` | 66 | `AppError::WasmEdge(String)` 变体 | 重命名为 `AppError::Wasmtime` |
| `src/infra/config.rs` | ~10行 | `resolve_quickjs_path()` 查找 `wasmedge_quickjs.wasm`、注释文案 | 改为查找 `pi-qjs-wasi.wasm`（定制 quickjs-ng 构建产物） |
| `src/api/cli.rs` | ~20行 | `doctor` 命令中的 WasmEdge 检测文案、错误提示 | 更新文案为 Wasmtime + quickjs-ng |
| `src/ext/engine_stub.rs` | 88 | 桩实现中引用 `AppError::WasmEdge` | 更新错误变体名 |
| `src/ext/instance_stub.rs` | 71 | 桩实现 | 更新文案 |
| `src/lib.rs` | ~5行 | `pub use resolve_quickjs_path` | 更新导出名 |

### 4.3 JS 桥接层

| 文件 | 行数 | 说明 | 迁移动作 |
|------|------|------|----------|
| `assets/js/pi_bridge.js` | ~300 | `__pi_host_call` ABI 调用 | **无需修改**（ABI 签名保持一致） |
| `assets/js/pi_main_loop.js` | 40 | 事件循环 IIFE | **无需修改**（逻辑不依赖运行时） |
| `assets/js/pi_*_shim.js` | ~600 | 7 个 npm shim | **无需修改**（纯 JS 实现） |

### 4.4 wasmedge-quickjs 子模块

| 项目 | 说明 | 迁移动作 |
|------|------|----------|
| `wasmedge-quickjs/` 子模块 | WasmEdge 专用 QuickJS，已分析不可复用 | 移除子模块引用 |
| `wasmedge-quickjs/src/host_call.rs` | Wasm 侧 `__pi_host_call` 桥接 | 在定制 quickjs-ng 构建中以 C 代码重新实现（`pi_host_call.c`） |

### 4.5 测试文件

| 文件 | 行数 | 说明 | 迁移动作 |
|------|------|------|----------|
| `tests/wasmedge_e2e_tests.rs` | 2071 | 39 个 E2E 测试（含 15 个社区插件兼容测试） | 测试逻辑不变，改用 Wasmtime API 初始化 VM |
| `tests/js_api_alignment_tests.rs` | 169 | 2 个 JS API 对齐测试 | 同上 |

### 4.6 影响统计

| 类别 | 文件数 | 涉及代码行 |
|------|--------|-----------|
| 需重写的核心文件 | 2 | ~510 行 |
| 需适配的间接文件 | 7 | ~40 行改动 |
| JS 桥接层 | 9 | 0 行改动 |
| 测试文件 | 2 | ~100 行改动（初始化部分） |
| 总计 | 20 个文件 | ~650 行核心改动 |

---

## 5. 运行时适配层设计

目标：引入 trait 抽象层，让上层代码（VmActor、PluginManager、CLI）与具体 Wasm 运行时解耦。通过 Cargo feature flag 在编译期选择后端，同时支持未来扩展到其他运行时（Wasmer、WAMR 等）。

### 5.1 当前耦合分析

当前代码虽然有 `engine_stub.rs` / `instance_stub.rs` 与 `engine_wasmedge.rs` / `instance_wasmedge.rs` 的并行文件结构，但存在三个问题：

1. **没有公共 trait**：`WasmEngine` 和 `WasmInstance` 是具体类型，不是 trait impl。上层代码直接依赖具体类型。
2. **init_vm 泄露运行时内部类型**：`init_vm()` 返回 `Vm<'_, dyn SyncInst>` — 这是 wasmedge-sdk 专有类型，VmActor 需要直接调用 `vm.run_func()`。
3. **feature flag 未生效**：`Cargo.toml` 中 `wasmedge` feature 被注释掉，实际上无条件编译 wasmedge 实现。

耦合关系：

```
PluginManager ──持有──► Arc<WasmEngine>（具体类型）
     │                       │
     │                       ▼
     │               WasmInstance（具体类型）
     │                       │
     ▼                       ▼
VmActor ──持有──► WasmInstance
     │                       │
     │                       ▼
     │               Vm<'_, dyn SyncInst>（WasmEdge 专有）
     │                       │
     └─────调用──► vm.run_func("quickjs", "_start", [])
```

### 5.2 Trait 抽象设计

引入两个核心 trait，将运行时内部类型完全封装：

```rust
// src/ext/wasm_runtime.rs

use crate::infra::error::AppError;
use std::fmt;
use std::path::Path;

/// Trait 1: 全局 Wasm 运行时引擎（对应 WasmEngine）
///
/// 每种运行时后端实现一次：WasmedgeRuntime、WasmtimeRuntime 等。
pub trait WasmRuntime: Send + Sync + fmt::Debug {
    /// 运行时名称，用于 doctor 命令和日志（"wasmedge" / "wasmtime"）
    fn name(&self) -> &str;

    /// 为指定插件创建独立的 VM 实例
    fn create_vm(&self, plugin_id: &str) -> Result<Box<dyn WasmVm>, AppError>;
}

/// Trait 2: 单插件 Wasm VM 实例（对应 WasmInstance）
///
/// 封装从创建 → 注册宿主函数 → 执行脚本 → 销毁的完整生命周期。
/// 关键：不暴露任何运行时内部类型（Vm、Store、Module 等）。
pub trait WasmVm: Send + fmt::Debug {
    fn plugin_id(&self) -> &str;

    /// 注册宿主回调：JS 侧 __pi_host_call(json) -> json
    fn register_host_binding(
        &mut self,
        invoke_fn: Box<dyn Fn(&str) -> Result<String, AppError> + Send + Sync>,
    ) -> Result<(), AppError>;

    /// 短生命周期执行：运行 JS 代码片段（用于 load_plugin 校验）
    fn run_script(&mut self, code: &str) -> Result<serde_json::Value, AppError>;

    /// 短生命周期执行：运行 .js 文件
    fn run_script_file(&mut self, path: &Path) -> Result<serde_json::Value, AppError>;

    /// 长生命周期执行：初始化 VM + 注入 bridge/shim + 执行 _start 进入事件循环。
    /// 此方法会阻塞直到 JS 事件循环退出（由 VmActor 在 spawn_blocking 中调用）。
    /// 关键抽象点：将 init_vm + run_func 合并，不暴露 Vm/Store 等内部类型。
    fn init_and_run_entrypoint(&mut self, script_path: &Path) -> Result<(), AppError>;
}
```

### 5.3 解耦后的依赖关系

```
PluginManager ──持有──► Arc<dyn WasmRuntime>（trait object）
     │                       │
     │                  create_vm()
     │                       ▼
     │               Box<dyn WasmVm>（trait object）
     │                       │
     ▼                       ▼
VmActor ──持有──► Box<dyn WasmVm>
     │                       │
     └─────调用──► vm.init_and_run_entrypoint(script_path)
                            │
                     （内部实现隐藏）
                     WasmEdge: init_vm → run_func("_start")
                     Wasmtime: linker.instantiate → call("_start")
```

上层代码（VmActor、PluginManager）完全不感知具体运行时实现。

### 5.4 Feature Flag 与工厂函数

```toml
# Cargo.toml
[features]
default = ["rt-wasmedge"]
rt-wasmedge = ["dep:wasmedge-sdk"]
rt-wasmtime = ["dep:wasmtime", "dep:wasmtime-wasi"]
```

```rust
// src/ext/mod.rs

#[cfg(feature = "rt-wasmedge")]
mod engine_wasmedge;
#[cfg(feature = "rt-wasmtime")]
mod engine_wasmtime;
mod wasm_runtime;  // trait 定义

pub use wasm_runtime::{WasmRuntime, WasmVm};

/// 工厂函数：根据编译时 feature 创建对应运行时
pub fn create_runtime(
    config: WasmEngineConfig,
) -> Result<Arc<dyn WasmRuntime>, AppError> {
    // 优先 wasmtime（如果两个都开启）
    #[cfg(feature = "rt-wasmtime")]
    {
        return engine_wasmtime::WasmtimeRuntime::new(config)
            .map(|r| Arc::new(r) as Arc<dyn WasmRuntime>);
    }
    #[cfg(feature = "rt-wasmedge")]
    {
        return engine_wasmedge::WasmedgeRuntime::new(config)
            .map(|r| Arc::new(r) as Arc<dyn WasmRuntime>);
    }
    #[allow(unreachable_code)]
    Err(AppError::WasmRuntime(
        "No Wasm runtime enabled. Enable 'rt-wasmedge' or 'rt-wasmtime'.".into(),
    ))
}
```

### 5.5 上层代码改动

改动量极小，主要是类型签名替换：

**PluginManager（`src/ext/plugin.rs`）**

```rust
// Before:
wasm_engine: Option<Arc<WasmEngine>>,
pub fn set_wasm_engine(&mut self, engine: Arc<WasmEngine>)

// After:
wasm_runtime: Option<Arc<dyn WasmRuntime>>,
pub fn set_wasm_runtime(&mut self, runtime: Arc<dyn WasmRuntime>)
```

**PluginInstance（`src/ext/plugin.rs`）**

```rust
// Before:
pub wasm_instance: Option<WasmInstance>,

// After:
pub wasm_vm: Option<Box<dyn WasmVm>>,
```

**VmActor（`src/ext/vm_actor.rs`）**

```rust
// Before:
pub struct VmActor {
    instance: WasmInstance,
    ...
}

fn run_vm(&mut self) -> Result<(), AppError> {
    let (mut vm, _, _tmp_dir) = self.instance.init_vm(&self.script_path)?;
    let run_result = vm.run_func(Some("quickjs"), "_start", []);
    ...
}

// After:
pub struct VmActor {
    vm: Box<dyn WasmVm>,
    ...
}

fn run_vm(&mut self) -> Result<(), AppError> {
    self.vm.init_and_run_entrypoint(&self.script_path)
}
```

**AppError（`src/infra/error.rs`）**

```rust
// Before:
WasmEdge(String),

// After:
WasmRuntime(String),  // 运行时无关的名称
```

### 5.6 各运行时后端实现

每个后端只需实现两个 trait，自包含在独立文件中：

| 后端 | 文件 | 实现 `WasmRuntime` | 实现 `WasmVm` |
|------|------|---------------------|----------------|
| WasmEdge | `engine_wasmedge.rs` + `instance_wasmedge.rs` | `WasmedgeRuntime` | `WasmedgeVm` |
| Wasmtime | `engine_wasmtime.rs` + `instance_wasmtime.rs` | `WasmtimeRuntime` | `WasmtimeVm` |
| 桩（无运行时）| `engine_stub.rs` | 已有，所有方法返回 Error | — |
| 未来: Wasmer | `engine_wasmer.rs` | `WasmerRuntime` | `WasmerVm` |

### 5.7 迁移策略

适配层应作为迁移的**第一步**（新增阶段零），在现有 WasmEdge 实现上先完成 trait 抽象，确保所有测试通过后再开始 Wasmtime 后端开发：

```
阶段零（新增）: 引入 trait 抽象层              ← WasmEdge 仍为唯一后端，所有测试必须通过
阶段一: 添加 Wasmtime 后端                   ← 与 WasmEdge 并存，feature flag 切换
阶段二: 定制 quickjs-ng WASI 构建            ← 产出 pi-qjs-wasi.wasm
阶段三~六: 同原计划
```

这样的好处：

1. **风险隔离**：阶段零不改变任何运行时行为，仅重构接口，回归测试 = 全量通过即可
2. **并行开发**：trait 定义完成后，WasmEdge 和 Wasmtime 后端可并行开发
3. **渐进切换**：开发期间用 `--features rt-wasmedge` 确保主线不受影响，Wasmtime 就绪后切换 default feature
4. **未来扩展**：添加新运行时只需新增一个 feature + 两个 impl 文件

### 5.8 编译期运行时切换（完整方案）

改造完成后，WasmEdge 和 Wasmtime 两个后端**同时存在于代码仓库中**，通过 Cargo feature flag 在编译期选择其一。

#### Cargo.toml 配置

```toml
[features]
default = ["rt-wasmedge"]               # 当前默认 WasmEdge；迁移完成后改为 rt-wasmtime
rt-wasmedge = ["dep:wasmedge-sdk"]
rt-wasmtime = ["dep:wasmtime", "dep:wasmtime-wasi"]

[dependencies]
wasmedge-sdk = { version = "0.13.5-newapi", features = ["aot"], optional = true }
wasmtime = { version = "29", optional = true }
wasmtime-wasi = { version = "29", optional = true }
```

#### 条件编译结构

```rust
// src/ext/mod.rs

mod wasm_runtime;                       // trait 定义（始终编译）

#[cfg(feature = "rt-wasmedge")]
mod engine_wasmedge;                    // WasmEdge 后端
#[cfg(feature = "rt-wasmedge")]
mod instance_wasmedge;

#[cfg(feature = "rt-wasmtime")]
mod engine_wasmtime;                    // Wasmtime 后端
#[cfg(feature = "rt-wasmtime")]
mod instance_wasmtime;

pub use wasm_runtime::{WasmRuntime, WasmVm, create_runtime};
```

编译时只有被选中 feature 对应的后端文件参与编译，未选中的后端零开销。

#### 开发者使用方式

```bash
# 使用 WasmEdge 后端（默认，改造前后行为一致）
cargo build
cargo test

# 切换到 Wasmtime 后端
cargo build --no-default-features --features rt-wasmtime
cargo test --no-default-features --features rt-wasmtime

# 明确指定 WasmEdge
cargo build --no-default-features --features rt-wasmedge

# 查看当前运行时（通过 doctor 命令）
cargo run -- doctor
# 输出：✓ Wasm 运行时：wasmtime (quickjs-ng)
# 或者：✓ Wasm 运行时：wasmedge (wasmedge-quickjs)
```

#### doctor 命令自动感知

```rust
// src/api/cli.rs — doctor 子命令
fn run_doctor(cfg: &AppConfig) {
    match create_runtime(wasm_cfg) {
        Ok(rt) => println!("✓ Wasm 运行时：{}", rt.name()),
        Err(e) => println!("✗ Wasm 运行时：不可用 ({})", e),
    }
}
```

`rt.name()` 由各后端 `impl WasmRuntime` 返回，WasmEdge 返回 `"wasmedge"`，Wasmtime 返回 `"wasmtime"`。

#### CI 矩阵测试

两个后端在 CI 中并行测试，确保切换不引入回归：

```yaml
# .github/workflows/ci.yml
jobs:
  test:
    strategy:
      matrix:
        runtime: [rt-wasmedge, rt-wasmtime]
    steps:
      - run: cargo test --no-default-features --features ${{ matrix.runtime }}
      - run: cargo clippy --no-default-features --features ${{ matrix.runtime }} -- -D warnings
```

#### 互斥保护

两个 feature 不应同时开启（避免符号冲突和歧义）。在 `create_runtime` 中，如果两个都开启则优先 wasmtime（见 5.4），但也可加编译期检查：

```rust
#[cfg(all(feature = "rt-wasmedge", feature = "rt-wasmtime"))]
compile_error!("Feature 'rt-wasmedge' and 'rt-wasmtime' are mutually exclusive. Enable only one.");
```

#### 切换 default 的时机

| 阶段 | default feature | 说明 |
|------|----------------|------|
| 阶段零完成 | `rt-wasmedge` | Trait 抽象就绪，WasmEdge 仍为唯一可用后端 |
| 阶段一~五开发中 | `rt-wasmedge` | Wasmtime 后端在 `--features rt-wasmtime` 下开发测试 |
| 阶段五全量测试通过 | **切换为 `rt-wasmtime`** | CI 矩阵两个后端均绿，正式切换默认值 |
| 阶段六清理后 | `rt-wasmtime` | 可选：保留或移除 `rt-wasmedge` feature |

### 5.9 测试代码运行时无关化

核心原则：**所有测试代码只依赖 trait 接口和工厂函数，不 import 任何后端具体类型，切换 feature 后零改动直接 `cargo test`。**

#### 当前测试的三处耦合

| 耦合点 | 当前代码 | 问题 |
|--------|----------|------|
| import 具体类型 | `use pi_wasm::WasmEngine;` | 绑定 WasmEdge 后端具体 struct |
| 硬编码 wasm 路径 | `assets/wasm/wasmedge_quickjs.wasm` | 文件名含 "wasmedge"，Wasmtime 后端用不同 provider 文件 |
| 直接调用具体 API | `WasmEngine::global(config)` → `engine.create_instance()` | 需要知道后端具体的构造方法 |

#### 解耦方案

**1. 统一入口：测试只用 `create_runtime` + trait 方法**

```rust
// tests/e2e_tests.rs (改造后)

use pi_wasm::{create_runtime, WasmRuntime, WasmVm, WasmEngineConfig};

#[test]
fn test_e2e_engine_instance_run_script() -> Result<(), Box<dyn std::error::Error>> {
    let config = WasmEngineConfig {
        quickjs_path: Some(require_provider_wasm()),  // 运行时无关的路径
        ..Default::default()
    };
    let runtime = create_runtime(config)?;             // 工厂函数，按 feature 返回对应后端
    let mut vm = runtime.create_vm("e2e-plugin")?;     // trait 方法
    vm.register_host_binding(Box::new(|_req| { ... }))?;
    vm.run_script("")?;
    Ok(())
}
```

测试中 `runtime` 和 `vm` 都是 trait object，不知道也不关心底层是 WasmEdge 还是 Wasmtime。

**2. provider wasm 路径自动选择**

不同后端使用不同的 QuickJS provider 文件。通过一个运行时无关的辅助函数自动解析：

```rust
// tests/common/mod.rs

/// 返回当前编译后端对应的 QuickJS provider wasm 路径。
/// rt-wasmedge → assets/wasm/wasmedge_quickjs.wasm
/// rt-wasmtime → assets/wasm/pi-qjs-wasi.wasm
pub fn require_provider_wasm() -> String {
    let manifest_dir = env!("CARGO_MANIFEST_DIR");

    #[cfg(feature = "rt-wasmtime")]
    let filename = "pi-qjs-wasi.wasm";
    #[cfg(feature = "rt-wasmedge")]
    let filename = "wasmedge_quickjs.wasm";
    #[cfg(not(any(feature = "rt-wasmedge", feature = "rt-wasmtime")))]
    let filename = "quickjs_provider.wasm";

    let p = std::path::Path::new(manifest_dir)
        .join("assets/wasm")
        .join(filename);
    assert!(
        p.exists(),
        "集成测试要求 {} 存在于 {:?}",
        filename, p
    );
    p.to_string_lossy().into_owned()
}
```

测试代码调用 `require_provider_wasm()`，不需要知道具体文件名。`#[cfg]` 在 `common/mod.rs` 一处集中处理，其他测试文件完全不含条件编译。

**3. fixture 目录结构**

```
tests/
  common/
    mod.rs                        # require_provider_wasm() + setup helpers
  fixtures/
    quickjs/                      # 运行时无关的 JS 测试脚本（重命名自 wasmedge_quickjs/）
      hello.js
      js_api_async_test.js
      require_path_test.js
      ...
    pi_mono_tps/
      tps.ts
  e2e_tests.rs                   # 运行时无关的 E2E 测试（重命名自 wasmedge_e2e_tests.rs）
  js_api_alignment_tests.rs
```

fixture JS 文件本身不含运行时相关代码（都是纯 JS），只需将目录名从 `wasmedge_quickjs/` 改为 `quickjs/` 以保持中立。

**4. 不需要改动的部分**

以下内容在切换 feature 后天然不受影响：

| 项目 | 原因 |
|------|------|
| 所有 JS fixture 文件 | 纯 JS，与运行时无关 |
| `pi_bridge.js` + 所有 shim | 通过 `__pi_host_call` ABI 与宿主通信，ABI 签名两个后端一致 |
| `HostApiDispatcher` 相关测试逻辑 | 纯 Rust 层，不涉及 Wasm 运行时 |
| 测试中的断言逻辑 | 验证的是 JS 执行结果和 host call 行为，与运行时实现无关 |

**5. 验证：切换后零改动跑测试**

```bash
# WasmEdge 后端：全量测试
cargo test --no-default-features --features rt-wasmedge

# Wasmtime 后端：同一套测试，零改动
cargo test --no-default-features --features rt-wasmtime

# 两次执行使用完全相同的测试源码，仅 provider wasm 文件和运行时后端不同
```

#### 阶段零中的对应工作项

在阶段零（Trait 抽象层）中需增加一项测试改造：

| # | 工作项 | 涉及文件 | 说明 |
|---|--------|----------|------|
| 0.8 | 测试代码去耦合 | `tests/wasmedge_e2e_tests.rs` → `tests/e2e_tests.rs`、`tests/common/mod.rs`、`tests/fixtures/` | 将 `use WasmEngine` 替换为 `use create_runtime`；抽取 `require_provider_wasm()` 辅助函数；重命名 fixture 目录 |

---

## 6. 完整工作项清单

### 阶段零：引入 Trait 抽象层（预估 1-2 天）

| # | 工作项 | 涉及文件 | 风险 | 说明 |
|---|--------|----------|------|------|
| 0.1 | 定义 `WasmRuntime` + `WasmVm` trait | 新建 `src/ext/wasm_runtime.rs` | 低 | 两个 trait + `create_runtime()` 工厂函数 |
| 0.2 | 重命名 `AppError::WasmEdge` → `AppError::WasmRuntime` | `src/infra/error.rs` + 全局引用 | 低 | 运行时无关的错误变体名 |
| 0.3 | 为现有 WasmEdge 实现 trait | `src/ext/engine_wasmedge.rs`、`src/ext/instance_wasmedge.rs` | 中 | `WasmEngine` → `impl WasmRuntime`；`WasmInstance` → `impl WasmVm`；关键：将 `init_vm` + `run_func` 合并为 `init_and_run_entrypoint` |
| 0.4 | 改造 `PluginManager` 使用 trait | `src/ext/plugin.rs` | 中 | `Arc<WasmEngine>` → `Arc<dyn WasmRuntime>`；`Option<WasmInstance>` → `Option<Box<dyn WasmVm>>` |
| 0.5 | 改造 `VmActor` 使用 trait | `src/ext/vm_actor.rs` | 中 | `WasmInstance` → `Box<dyn WasmVm>`；`run_vm` 调用 `init_and_run_entrypoint` |
| 0.6 | 更新 `mod.rs` 导出 + feature flag | `src/ext/mod.rs`、`Cargo.toml` | 低 | 添加 `rt-wasmedge` feature（default），条件编译 |
| 0.7 | 测试代码去耦合 | `tests/wasmedge_e2e_tests.rs` → `tests/e2e_tests.rs`、`tests/common/mod.rs` | 中 | 移除 `use WasmEngine` 改为 `use create_runtime`；抽取 `require_provider_wasm()`；fixture 目录 `wasmedge_quickjs/` → `quickjs/` |
| 0.8 | 全量回归测试 | 所有测试 | 低 | `cargo test` + `cargo clippy` 必须全部通过 |

### 阶段一：基础运行时替换（预估 3-4 天）

| # | 工作项 | 涉及文件 | 风险 | 说明 |
|---|--------|----------|------|------|
| 1.1 | 替换 Cargo.toml 依赖 | `Cargo.toml` | 低 | 移除 `wasmedge-sdk`，添加 `wasmtime`、`wasmtime-wasi`；确认版本兼容 |
| 1.2 | 重写 WasmEngine | `src/ext/engine_wasmedge.rs` → `src/ext/engine_wasmtime.rs` | 中 | `wasmedge_sdk::Config` → `wasmtime::Config` + `wasmtime::Engine`；配置项映射：`bulk_memory_operations`、`max_memory_pages` 等 |
| 1.3 | 重写 WasmInstance 核心 | `src/ext/instance_wasmedge.rs` → `src/ext/instance_wasmtime.rs` | 高 | 最大工作量：`Vm`/`Store`/`Module`/`WasiModule`/`ImportObjectBuilder` 全部替换为 Wasmtime 对等 API |
| 1.4 | 迁移 `__pi_host_call` 宿主函数 | `src/ext/instance_wasmtime.rs` | 中 | `ImportObjectBuilder.with_func` → `Linker.func_wrap`；内存访问从 `CallingFrame::memory_mut` → `Caller::get_export("memory")` |
| 1.5 | 更新模块导出 | `src/ext/mod.rs` | 低 | `engine_wasmedge` → `engine_wasmtime`；`instance_wasmedge` → `instance_wasmtime` |
| 1.6 | 更新错误类型 | `src/infra/error.rs` | 低 | `AppError::WasmEdge` → `AppError::Wasmtime`，更新 `#[error]` 文案 |

### 阶段二：定制 quickjs-ng WASI 构建 + Node API 兼容层（预估 4-6 天）

| # | 工作项 | 涉及文件 | 风险 | 说明 |
|---|--------|----------|------|------|
| 2.1 | Fork quickjs-ng，添加 `pi_host_call.c` | 新建 `vendor/quickjs-ng/` 或 Git submodule | 中 | 实现 `__pi_host_call` 的 WASM import 声明 + JS 全局注册（等价于 `wasmedge-quickjs/src/host_call.rs`），约 80 行 C 代码 |
| 2.2 | 配置 wasi-sdk + CMake 构建 | `vendor/quickjs-ng/CMakeLists.txt` 修改 | 中 | 确保 `pi_host_call.c` 被链接到 qjs-wasi 构建；目标产物：`pi-qjs-wasi.wasm`（Command 模式） |
| 2.3 | 验证 `__pi_host_call` 端到端 | 新增测试 | 中 | Wasmtime 加载 `pi-qjs-wasi.wasm` → `linker.func_wrap("env", "__pi_host_call", ...)` → JS 调用 `__pi_host_call()` → 宿主回调成功 |
| 2.4 | 构建脚本集成 | `build.rs` 或 `Makefile` | 低 | 自动化 wasi-sdk 编译流程，CI 中产出 `pi-qjs-wasi.wasm` 并作为 artifact 分发 |
| 2.5 | （可选）Reactor 模式构建 | `vendor/quickjs-ng/` | 低 | 同时产出 `pi-qjs-wasi-reactor.wasm`，为长生命周期 VM 优化做准备 |
| 2.6 | 移植 `_node:fs` 模块 | 新建 `vendor/quickjs-ng/pi_node_fs.c` | 中-高 | 将 wasmedge-quickjs `fs.rs` 的 26 个 WASI fd/path 函数翻译为 C，注册为 `_node:fs` QuickJS 内部模块。全部使用标准 WASI 系统调用，Wasmtime 完全支持 |
| 2.7 | 移植 `process.env`/`exit` 全局函数 | `vendor/quickjs-ng/pi_globals.c` | 低 | WASI `environ_get()` → JS 全局 `env` 对象；`proc_exit()` → JS 全局 `exit()` |
| 2.8 | 移植 `_encoding` 模块 | `vendor/quickjs-ng/pi_encoding.c` | 中 | UTF-8 编解码优先（覆盖 90%+ 使用场景）；其他编码（GBK/Big5 等）按需添加 |
| 2.9 | 扩展 `pi_node_shim.js` 中 crypto 基础 | `assets/js/pi_node_shim.js` | 低 | 将 `randomBytes` 改为调用 WASI `random_get`（如需更安全的随机数）；高级 crypto 可后续通过 host call 代理 |

### 阶段三：VM Actor 层适配（预估 1-2 天）

| # | 工作项 | 涉及文件 | 风险 | 说明 |
|---|--------|----------|------|------|
| 3.1 | 适配 `WasmInstance::init_vm` | `src/ext/instance_wasmtime.rs` | 中 | 返回类型从 `Vm<'_, dyn SyncInst>` 改为 Wasmtime `Instance` + `Store<HostState>` |
| 3.2 | 适配 `VmActor::run_vm` | `src/ext/vm_actor.rs` | 中 | `vm.run_func("quickjs", "_start")` → `instance.get_typed_func::<(), ()>("_start")`；退出码判断逻辑调整 |
| 3.3 | WASI 配置迁移 | `src/ext/instance_wasmtime.rs` | 低 | `WasiModule::create(argv, env, preopens)` → `WasiCtxBuilder::new().args().preopened_dir()` |

### 阶段四：配置与 CLI 适配（预估 1 天）

| # | 工作项 | 涉及文件 | 风险 | 说明 |
|---|--------|----------|------|------|
| 4.1 | 更新 `resolve_quickjs_path` | `src/infra/config.rs` | 低 | 查找目标从 `wasmedge_quickjs.wasm` → `pi-qjs-wasi.wasm`；环境变量从 `WASMEDGE_QUICKJS_PATH` → `PI_QJS_WASM_PATH` |
| 4.2 | 更新 `doctor` 命令 | `src/api/cli.rs` | 低 | 检测文案："WasmEdge 运行时" → "Wasmtime 运行时"；安装提示更新；QuickJS 路径提示更新 |
| 4.3 | 更新错误提示 | `src/api/cli.rs` | 低 | 插件加载失败时的 "WasmEdge" 关键字替换 |
| 4.4 | 更新 `WasmEngineConfig` 文案 | `src/ext/engine_stub.rs` | 低 | 桩实现中的错误消息和注释 |
| 4.5 | 更新 lib.rs 导出 | `src/lib.rs` | 低 | `resolve_quickjs_path` 改名或保持（仅内部实现变化） |

### 阶段五：测试迁移与验证（预估 2-3 天）

| # | 工作项 | 涉及文件 | 风险 | 说明 |
|---|--------|----------|------|------|
| 5.1 | 迁移 E2E 测试初始化 | `tests/e2e_tests.rs`（已在阶段零重命名） | 中 | 确保 `require_provider_wasm()` 在 `rt-wasmtime` 下返回 `pi-qjs-wasi.wasm`；39 个测试用例断言逻辑不变 |
| 5.2 | 迁移 API 对齐测试 | `tests/js_api_alignment_tests.rs` | 低 | 同上，仅 provider 路径变化 |
| 5.3 | 全量回归测试 | 所有测试文件 | 中 | `cargo test --features rt-wasmtime` + `cargo clippy` 全部通过 |
| 5.4 | 长生命周期 VM 压力测试 | 新增测试 | 中 | 验证 quickjs-ng WASI 在长时间运行下的稳定性；特别关注内存泄漏和 Context 状态一致性 |

### 阶段六：清理与文档（预估 1 天）

| # | 工作项 | 涉及文件 | 风险 | 说明 |
|---|--------|----------|------|------|
| 6.1 | 移除 wasmedge-quickjs 子模块 | `.gitmodules`、`wasmedge-quickjs/` | 低 | `git submodule deinit wasmedge-quickjs && git rm wasmedge-quickjs` |
| 6.2 | 移除 WasmEdge 安装脚本 | `scripts/install-wasmedge.sh`（如有） | 低 | 不再需要外部 C/C++ 依赖安装 |
| 6.3 | 更新 README | `README.md` | 低 | 构建前提从 "安装 WasmEdge" 改为 "cargo build 即可" |
| 6.4 | 更新 agents 文档 | `agents/` 下相关文档 | 低 | 架构描述从 WasmEdge 更新为 Wasmtime + quickjs-ng |
| 6.5 | 更新 CI 配置 | `.github/workflows/`（如有） | 低 | 移除 WasmEdge 安装步骤 |

### 工作量汇总

| 阶段 | 预估工时 | 风险等级 |
|------|---------|---------|
| 零、Trait 抽象层 | 1-2 天 | 低-中 |
| 一、基础运行时替换 | 3-4 天 | 中-高 |
| 二、定制 quickjs-ng WASI 构建 + Node API 兼容层 | 4-6 天 | 中-高 |
| 三、VM Actor 层适配 | 1-2 天 | 中 |
| 四、配置与 CLI 适配 | 1 天 | 低 |
| 五、测试迁移与验证 | 2-3 天 | 中 |
| 六、清理与文档 | 1 天 | 低 |
| **总计** | **13-19 天** | — |

---

## 7. 推荐实施路径

### 7.1 分支策略

```
develop
  └─ feature/wasm-runtime-abstraction   (阶段零：先合并，无功能变化)
  └─ feature/wasmtime-migration
       ├─ step-1/runtime-replace     (阶段一：Wasmtime 宿主侧)
       ├─ step-2/quickjs-ng-build    (阶段二：定制 quickjs-ng WASI)
       ├─ step-3/vm-actor-adapt      (阶段三)
       ├─ step-4/config-cli          (阶段四)
       ├─ step-5/test-validation     (阶段五)
       └─ step-6/cleanup             (阶段六)
```

### 7.2 关键决策点

**决策 1：quickjs-ng WASI 模式选择**

- **方案 A（推荐）：Command 模式** — 用 `pi-qjs-wasi.wasm` 直接替代 `wasmedge_quickjs.wasm`。`_start` 入口，argv 接收脚本路径，执行后退出。与当前 `init_vm` + `run_func("_start")` 流程完全一致，迁移改动最小。
- **方案 B（后续优化）：Reactor 模式** — 用 `pi-qjs-wasi-reactor.wasm`，init 一次后反复 `JS_Eval()`。适合长生命周期 VM，消除每次 `_start` 的重初始化开销。可作为独立优化阶段引入。

**决策 2：脚本注入方式**

- **方案 A（推荐）：保持运行时拼接** — `build_combined_script` 不变，仅替换底层 Wasm 运行时。quickjs-ng Command 模式与当前 wasmedge-quickjs 行为一致（argv 接收 JS 文件），零风险。
- **方案 B（后续优化）：Reactor 模式下按需 eval** — 先 `qjs_init()` 加载 bridge + shim 到持久 Context，然后只 eval 用户脚本。启动更快，但需要改造 `VmActor` 的 init/run 分离逻辑。

**决策 3：quickjs-ng 代码管理**

- **方案 A（推荐）：Git submodule** — `vendor/quickjs-ng` 作为 submodule，本地添加 `pi_host_call.c` 和 CMake 补丁。上游更新时 `git pull && rebase` 即可。
- **方案 B：维护独立 fork** — 在 GitHub 创建 `pi-quickjs-ng` fork。修改在独立分支，通过 PR 管理。适合团队协作但维护成本稍高。

### 7.3 回滚方案

有了 trait 抽象层后，回滚变为"切换 feature flag"的零成本操作：

```toml
[features]
default = ["rt-wasmedge"]            # 迁移前：WasmEdge 为默认
# default = ["rt-wasmtime"]          # 迁移完成后：切换默认值
rt-wasmedge = ["dep:wasmedge-sdk"]
rt-wasmtime = ["dep:wasmtime", "dep:wasmtime-wasi"]
```

```bash
# 开发期间验证 Wasmtime 后端
cargo test --no-default-features --features rt-wasmtime

# 出问题时回退到 WasmEdge
cargo test --features rt-wasmedge
```

阶段零合并后，主线默认仍为 `rt-wasmedge`，完全不影响现有功能。Wasmtime 后端在独立 feature 下开发和测试，就绪后只需修改 `default` 一行即完成切换。
