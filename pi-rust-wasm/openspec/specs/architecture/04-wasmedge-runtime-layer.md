本文为 [Architecture](../Architecture.md) 中「4. WasmEdge运行时层」的详细设计，总览见主文档。

## 4. WasmEdge运行时层

项目的沙箱隔离核心，基于WasmEdge官方构建，全局单例Engine，每个插件对应独立的Store/Instance，完全隔离的线性内存与执行环境，是JS/TS插件的执行载体。
#### 4.1 核心组件
- 全局WasmEdge Engine：全局唯一初始化，负责Wasm模块编译、内存管理、实例调度，仅一次初始化开销
- 插件独立Wasm实例：每个插件对应一个独立的WasmEdge实例，专属线性内存、调用栈、QuickJS上下文，故障不扩散、数据不串扰
- QuickJS运行时：WasmEdge官方优化版QuickJS引擎，内置到Wasm实例中，原生支持JS/TS代码执行，无需手动打包嵌入
- 宿主导入绑定：将宿主API显式注册到Wasm实例的导入表，映射为QuickJS全局对象，插件可直接调用，调用请求通过WasmEdge通道转发到宿主层执行
- WASI标准支持：原生支持WASI Preview2、WASI-Socket、WASI-HTTP，网络、文件IO能力受宿主权限管控

#### 4.2 插件加载执行流程
1.  宿主读取插件JS/TS代码，校验插件清单与权限声明
2.  创建独立的WasmEdge实例，初始化QuickJS运行时与Node.js兼容层
3.  向实例导入表注册该插件授权的宿主API，未授权API不注册
4.  将插件代码注入QuickJS运行时，执行插件初始化逻辑，注册工具、监听事件
5.  插件运行时，调用宿主API时，通过WasmEdge通道转发到宿主层，经过权限校验后执行，结果原路返回
6.  插件卸载时，销毁WasmEdge实例，完全释放所有内存与资源，无残留


#### 4.3 内存安全与数据交换 (Memory & Data Exchange)
- **内存所有权**：遵循“谁申请谁释放”原则。沙箱调用 API 时，在 Wasm 线性内存中申请空间存放参数，并传递指针与长度给宿主。
- **边界校验**：宿主在访问 Wasm 内存指针前，必须进行边界检查，确保指针落在该插件实例的合法线性内存范围内，防止越界访问。
- **零拷贝优化（预留）**：对于 4 原语中涉及的大文件读写，支持通过共享内存或 WasmEdge 的 `memory.copy` 直接操作字节流，避免大规模 JSON 字符串转换。

#### 4.4 并发调度模型 (Concurrency Model)
- **多实例并行**：每个 Agent 拥有独立的 Wasm 实例，宿主在 Tokio 多线程工作池中并行调度这些实例。
- **非阻塞分发**：Hostcall 路由器采用无状态设计，支持多线程同时通过单一入口进入。
- **资源竞争隔离**：宿主资源（如文件句柄、Session 对象）通过 `Arc<RwLock<T>>` 或 `DashMap` 管理，确保不同 Agent 操作不同会话时完全零竞争。
- **异步逃生通道**：针对高延迟操作（LLM/网络），宿主侧统一使用异步 `await` 处理，不占用 Wasm 实例的执行配额，防止由于单个 Agent 挂起 LLM 调用而导致整个引擎线程池枯竭。

#### 4.5 资源与内存模式 (Resource & Memory Profile)

资源上限（Wasm 页数、QuickJS 堆、插件并发、消息缓存等）依 **MemoryProfile** 派生，不硬编码。**一期 MVP 仅落文档与任务约束，不实现 MemoryProfile 等代码**；十一期实现完整资源改造。

##### 内存模式 (MemoryProfile)

- **Low**：目标 < 10MB，嵌入式/边缘设备。单线程 Tokio、Wasm 64 Pages、QuickJS 堆典型 2–3MB，**软上限**（如 5MB）达则告警、**硬上限**（如 8MB，依 10MB 总预算）强制限制；插件并发 1、惰性加载 + 可选 LRU、Transcript 流式、消息缓存 0。文档约定：Low 模式建议插件精简、避免在 JS 层处理大文件，利用宿主 API 流式处理。
- **Standard**：目标 ~50MB，**默认**。多线程 Tokio（如 CPU/2）、Wasm 512 Pages、JS 堆 16MB、插件并发 3–5、惰性加载、最近 10 条缓存。
- **High**：无硬性上限，开发/服务器。多线程原生、Wasm 8192 Pages、JS 堆 128MB+、并发不限、可预加载、全量/索引缓存。
- **Auto**（可选）：**仅在程序启动时执行一次**系统内存检测并锁定 Profile（如 <1GB→Low，>8GB→High，否则 Standard）；不在运行时根据剩余内存自动切换，避免 Flapping；用户若需变更则手动通过 `/config` 触发。

Low 模式下宿主 API（如 readFile）强制流式/分块，禁止单次读取超过 1MB（与 4 原语「大文件分块读取」一致）。**Low 模式**与 4.4 多实例并行的关系为「按模式切换」——Low 下通过 Semaphore 等限制为逻辑单并发，Standard/High 下保持多实例并行。

##### MemorySettings 与参数表

| 参数 | Low | Standard | High |
|------|-----|----------|------|
| Tokio | current_thread，栈 256KB | multi_thread（如 CPU/2） | multi_thread 原生 |
| Wasm 最大页数 | 64 (4MB) | 512 (32MB) | 8192 (512MB) |
| QuickJS 堆 | 2–3MB 典型，软 5MB 告警，硬 8MB | 16MB | 128MB+ |
| 插件并发 | 1 | 3–5 | 不限 |
| 消息缓存条数 | 0 | 10 | 全量/索引 |
| 惰性加载 | 是 + LRU 可选 | 是 | 可预加载 |

##### 零拷贝与流式（资源侧）

- **零拷贝**：明确推荐在 **sessions.json、config.toml、单行 JSONL transcript** 解析时使用 `from_slice` + 借用（`&'a str`）；跨 await 或长期持有的数据不强制零拷贝，实现时注意生命周期。
- **mimalloc**：在 10MB/Low 场景下推荐作为可选全局分配器（防碎片、稳 RSS）；以 feature 或 Low 专用构建启用，Standard/High 可不换。

##### 运行时动态切换（十一期实现）

- **原则**：新实例应用新规，旧实例动态缩减/驱逐。
- **Config Observer**：使用 `tokio::sync::watch` 广播 `MemoryProfile` 变更；各消费方持有 `Receiver`，在 `changed().await` 后执行本组件的 `apply_profile`。
- **插件**：High→Low 时对空闲 (Idle) 插件立即卸载；正在执行 (Active) 的允许跑完，跑完后标记 Outdated，下次空闲时销毁，再调用时按新 profile 重建。
- **Hostcall**：用 Semaphore 控制并发，许可数随当前 profile 变化。
- **消息缓存**：切换至 Low 后按新 `context_window_cache` 截断内存缓存并释放。
- **CLI**：例如 `/config memory low`，切换后输出资源释放汇总（如 `Freed N idle instances, M message caches cleared`），配置可写回文件。

**约定**：插件开发者应假设 Hostcall 环境是无状态的，或能够通过事件恢复状态；内存模式切换导致的实例重建**不保证 JS 堆内存的连续性**，插件内全局变量、未持久化的缓存会丢失。
