# 插件系统总览

本文为 [Architecture](../openspec/specs/Architecture.md) 中「4. 插件系统（统一入口）」的当前入口页。**当前实现口径只认进程内 `rquickjs` 运行时**；旧版 WasmEdge 时代文档已经退出主阅读路径，但其中真正有价值的导图、决策和“说人话”解释，已经重新收口到现行文档集。

## 摘要

可以把这套插件系统想成一台“会按需放映的多厅电影院”：

- 磁盘上的 `plugin.json` 和 `main.ts/js` 像片单与拷贝源；
- `PluginCatalog` / `ToolRegistry` / `FunctionRegistry` 像售票台和排片表，先决定“谁可见、谁能被调用”；
- 真正运行中的 `VmActor` / `PluginVmInstance` 才像某一场正在放映的影片实例，按 `(session_id, plugin_id)` 单独存在。

这也是为什么本文会反复强调三件事：**发现 != 激活 != 运行**、**tools != functions**、**scope 可见性 != session 期活体实例**。

## 文首导读：先读懂这六件事

1. **运行时换芯**：插件不再依赖外部 WasmEdge 和 `.wasm` 文件，而是直接运行在 tomcat 进程内嵌的 `rquickjs` 上。好处是开箱即用；代价是少了 Wasm 的内存硬墙，隔离要靠线程、预算、超时和错误态降级来做。
2. **崩溃隔离必须显式设计**：同进程跑第三方代码，最怕一个插件把整个宿主拖死。当前策略是“每个运行中的插件实例单独一条专属线程 + panic 捕获 + 超时 / budget 中断 + Error 态降级”，尽量把故障关在当前 `(session_id, plugin_id)` 里。
3. **多会话并发靠多实例，不靠共享活体**：同一插件被会话 A 和会话 B 同时使用时，当前模型不是共用一个 VM，而是给每个 `(session_id, plugin_id)` 各发一份独立活体，避免状态串台。
4. **插件是能力容器，不是“某一个工具”**：同一个插件可以同时贡献 `tools[]`、`functions[]`、事件订阅和命令。`tools[]` 给 LLM 看；`functions[]` 给宿主子系统看；两条注册面正交，不应该混成一套。
5. **作用域先分账，再谈运行**：`scope > agent > global` 决定的是“当前项目看得见什么”；`VmActor` 决定的是“当前会话里什么东西真的跑起来了”。不要把磁盘分层、scope 可见性和 session 期活体当成一层概念。
6. **异步模型分两种职责**：`submit/poll` 解决的是“插件里发起耗时 hostcall 怎么拿结果”；`waitForEvent` 解决的是“长生命周期 VM 怎么阻塞等下一条事件”。它们互补，不是二选一。

> 一句话串起来：**换成 `rquickjs` 之后，插件系统的重点不再是“怎么塞进 Wasm”，而是“怎么在单进程里把发现、注册、执行、隔离、并发和宿主扩展点这几件事分清楚”。**

## A.0 Mermaid 时序图（插件注册：从磁盘到共享注册面）

```mermaid
sequenceDiagram
    participant Disk as 磁盘(三层根:项目>agent>全局)
    participant LLM as LLM
    participant Host as 宿主调用者(WebSearchRuntime)
    participant Sess as 会话入口(SessionRuntime)
    participant AL as AgentLoop(execute_tool 总闸)
    participant PM as PluginManager(PluginToolExecutor)
    participant Cat as PluginCatalog(进程级发现+静态元信息,不可变)
    participant VM as 短命PluginVmInstance(懒,首次激活)
    participant RM as RuntimeManager
    participant LVM as 长命VmActor(session期)
    participant Disp as HostApiDispatcher
    participant Reg as ToolRegistry(scope 共享)
    participant FnReg as 宿主扩展点注册表(scope 共享,宿主专用)
    participant Map as plugins表(per-scope 管理态,可变)

    Note over Disk,Reg: 阶段一 编目/预填(触发分两拍: 进程启动先扫 global/agent；进入某 project scope 时再补扫该 project overlay；全程只读 manifest 不跑码)
    Disk->>PM: 进程启动先扫 global/agent roots + 同名 first-wins
    PM->>PM: 读 plugin.json -> PluginManifest{id,name,version,main,permissions, tools[], functions[], events[]/activation}
    PM->>Cat: pre-seed PluginCatalog 条目(immutable: id/version/root/manifest + declared tools[]/functions[]/events[])
    Note over Cat: 这就是「扫盘即填满」的那张表(不可变元信息层); ≠内置 BUILTIN_TOOL_CATALOG(那是编译期 const, 内置工具规格单一事实源)
    alt manifest 静态声明 tools[](目标态,首选)
        PM->>Cat: 记下 declared tool meta(待某 scope 命中时零跑码 materialize 到该 scope 的 ToolRegistry)
        Note over Cat: 冷启动最省; events[] 也可静态读到, 但它只是事件名声明, 不决定是否 session 入口预启动
    else 无静态 tools[](legacy)
        Note over Cat: 标记 needs_activation; 工具留到 scope 激活期跑码补登
    end
    opt manifest 静态声明 functions[](本版新增; 宿主专用)
        PM->>Cat: 记下 declared host-function contract(待某 scope 命中时按 point 分发到宿主扩展点注册表)
        Note over Cat: functions[] 只声明宿主可见契约; 不进 LLM, 不保存插件内部后端顺序/默认参数
    end

    Note over Sess,Map: 会话进入 / scope 首次激活(从 Catalog 命中集 seed per-scope 管理态; 「阶段二」与事件预启动也在此; 但 seed 表行≠跑码)
    Sess->>PM: 首次进入该 project scope 时补扫 project_root/.tomcat/plugins overlay(只读 manifest, 不跑码)
    Sess->>Cat: 合并出该 project scope 命中插件集(global/agent 共享 + project overlay)
    Sess->>Map: seed per-scope plugins表条目(轻量: 当前直接记为 Enabled catalog stub, 引用 Catalog 元信息, 账本字段空; 不读 main 不跑码)
    Note over Map: plugins表=per-scope「管理态层」, 记本 scope 的 status/config/registered_tools/registered_functions/event_listener_ids/VM句柄; 其中 registered_tools 镜像 manifest.tools[]（给 LLM 的契约面）, registered_functions 镜像 manifest.functions[]（给宿主的最小契约面）。Catalog(底座) + 本表(overlay) = 该 scope 下插件完整视图
    Note over Sess,LVM: 下面是两条【正交】决策(非三选一): A=工具可见性(看有无静态 tools[]); B=VM 生命周期(看 activation)。activation 与 tools[] 互不蕴含, 同一插件各走 A、B 一次
    alt A 有静态 tools[](目标态首选)
        Sess->>Reg: 零跑码 materialize manifest.tools[] -> 对该 scope 的 LLM 可见
    else A 无静态 tools[](legacy)
        opt 且 activation=lazy(不会被 B 预启动, 否则没人跑码登记工具)
            Sess->>VM: 「阶段二」短命 create_instance + run_script(manifest.main)
            VM->>Disp: hostCall('tools','registerTool', {name,...})
            Disp->>Reg: register_tool(跑码后才可见; execute 留 JS)
            VM-->>Sess: run_script ok(短命校验完即弃)
        end
        Note over Reg: 若 activation=session, 工具登记交给 B 预启动的长 VM 顺带完成, 不另起短命 VM
    end
    opt manifest 静态声明 functions[](宿主面, 本版新增)
        Sess->>FnReg: 零跑码按 point 分发 manifest.functions[] -> 对宿主可按扩展点契约调用(不进 LLM)
    end
    alt B activation="session"(生命周期型: 要接 session_start/定时器/订阅事件)
        Sess->>PM: 预启动 start_session_vm(session_id, plugin_id) (长跑 VM)
        PM->>RM: 建 actor + Init -> LVM 进 waitForEvent
        Sess->>Disp: deliver_event(session_start)
        Disp-->>LVM: waitForEvent() 取到 session_start
        PM->>Map: 置 status=Enabled(无静态 tools[] 时, 长 VM 启动代码里的 registerTool 在此一并登记; event_listener_ids 待运行期挂 on/once 再回填)
        Note over LVM: 必须此刻在场; 否则永久错过 session_start(不能拖到 tool_call)
    else B activation=lazy(默认: 纯按需)
        Note over RM: 不预启动长 VM; 等 LLM 首次 tool_call 进阶段三
    end

    Note over LLM,LVM: 阶段三 运行(这里懒的是“长跑 VM 起机”; 触发=LLM 真发 tool_call。前提:该工具已对 LLM 可见——要么阶段一静态 tools[]，要么会话进入时已补跑过阶段二)
    Note over LLM,Reg: 进入本块前, ToolRegistry 中已必须有该工具(来源=阶段一静态 tools[] 或会话进入期补跑的阶段二); LLM 看不见的工具不可能被 tool_call
    LLM-->>AL: 返回 tool_calls(toolName, args)
    Note over AL: execute_tool 总闸统一分发: 内置工具→PrimitiveExecutor; 插件工具→走下面这条插件分支(PluginToolExecutor)
    AL->>Reg: call_tool(toolName, params)
    Reg->>Reg: get_tool(toolName) -> Tool{plugin_id}（按名反查归属插件）
    Reg->>PM: PluginToolExecutor.execute(tool{plugin_id}, params, session_id)
    PM->>RM: ensure/start_session_vm(session_id, plugin_id)
    RM->>RM: get(key=session_id/plugin_id)? 命中复用; 未命中才新建
    RM->>LVM: spawn actor + send Init (actor 内部再 init_vm + 注入 bridge/shim + _start)
    LVM-->>RM: ready(已进入 waitForEvent loop；目标态建议显式握手)
    RM-->>PM: vm ready / handle
    alt 工具调用(执行 LLM 选中的那个插件工具)
        PM->>LVM: invoke __pi_execute_tool({toolName, params})
        Note over LVM: VM 内按裸 toolName 查 __pi_tools[toolName] 执行(单插件内不会撞名)
        LVM->>Disp: hostCall(...)
        Disp-->>LVM: tool result
        LVM-->>PM: tool result
        PM-->>Reg: result
        Reg-->>AL: 封装为 {content, details}
        AL-->>LLM: tool result 回传(进入下一轮)
    else 宿主函数调用(非 LLM; 例: web_search backend)
        Host->>FnReg: dispatch point="web_search.backend" + {backend,query,...}
        FnReg->>PM: PluginFunctionInvoker.execute(targetFn{plugin_id,function="webSearchBackend"}, params, session_id)
        PM->>LVM: invoke __pi_execute_function({functionName, params})
        Note over LVM: VM 内按 functionName 查 __pi_functions[functionName] 执行(不会进 ToolRegistry)
        LVM->>Disp: hostCall(...)
        Disp-->>LVM: function result
        LVM-->>PM: function result
        PM-->>FnReg: result / unsupported_backend
        FnReg-->>Host: {hits,warnings,backend_used} 或返回 incompatible；auto/fallback 仅在当前赢家插件内部继续
    else 事件投递(非 LLM tool_call 路径)
        PM->>Disp: deliver_event(instance_id, envelope)
        Disp-->>LVM: waitForEvent() 取到 event payload
    end
    Note over RM,LVM: session_end 或 idle TTL 时回收; 同 key 重复使用直接复用
    Note over PM,Map: unload_plugin(id) 时: 删该 scope 的 plugins表条 + 据 registered_tools / registered_functions 清理 ToolRegistry / 宿主扩展点注册表；事件监听现状主要按 plugin_id 批量 remove_plugin_listeners；RM.evict VM
```

## A.1 一图看懂：插件系统怎么转起来

如果你前面那张 Mermaid 时序图看得头大，先看下面这张。它不追求把所有分支都画全，而是先把这套系统最重要的主路径钉死：**插件先被发现，再决定谁可见，再按需起 VM，最后通过事件和 hostcall 跑起来。**

```text
[磁盘上的插件]
project/.tomcat/plugins        agent plugins        global plugins
           \                        |                    /
            \_______________________|___________________/
                                    |
                                    v
[阶段 1: 发现 / 编目]
PluginManager.scan()
  -> 只读 plugin.json / main 路径
  -> PluginCatalog
       |- manifest.tools[]       -> 以后给 LLM 看
       |- manifest.functions[]   -> 以后给宿主子系统调
       |- events[] / activation
       `- 此时还没有真正跑 JS

                                    |
                                    v
[阶段 2: scope 物化]
当前 project / agent / global 叠层决定 "谁可见"
  -> ToolRegistry         (LLM 只看这里)
  -> FunctionRegistry     (web_search 等宿主子系统只看这里)
  -> PluginRuntimeManager (准备管理活体 VM, 但此时可以还是空的)

                                    |
                                    v
[阶段 3: 起活体 VM]
当 activation="session" 或首次真正调用时:
  key = (session_id, plugin_id)
      -> VmActor (专属线程)
      -> PluginVmInstance (rquickjs)
      -> 注入 pi_bridge.js / pi_main_loop.js / __pi_* host bindings
      -> 进入空闲循环: await __pi_wait_for_event(50)

                                    |
                    +---------------+----------------+
                    |                                |
                    v                                v
[入口 A: LLM 工具调用]                        [入口 B: 宿主函数调用]
LLM                                        WebSearchRuntime / other host subsystem
 -> AgentLoop.execute_tool                  -> FunctionRegistry dispatch(point)
 -> ToolRegistry.get_tool(name)             -> PluginFunctionInvoker.execute
 -> PluginToolExecutor.execute              -> ensure/start_session_vm(...)
 -> ensure/start_session_vm(...)            -> dispatcher.deliver_event(command_invoke)
 -> dispatcher.deliver_event(command_invoke)-> VM 收到 command_invoke
 -> VM 收到 command_invoke                  -> __pi_execute_function(functionName, params)
 -> __pi_execute_tool(toolName, params)     -> 插件 JS 运行
 -> 插件 JS 运行                            -> pi.* / __pi_host_call(...)
 -> pi.* / __pi_host_call(...)              -> HostApiDispatcher
 -> HostApiDispatcher                       -> result -> commandCompleted(call_id, result)
 -> result -> commandCompleted(...)         -> 宿主收到 function result
 -> 宿主收到 tool result
 -> 返回给 LLM

                                    |
                                    v
[阶段 4: VM 空闲等待下一件事]
JS 回到 __pi_start_event_loop:
  await __pi_wait_for_event(50)
     |- 宿主在 50ms 内塞来事件 -> 返回真实事件
     |- 50ms 内没事件         -> 返回 { type: "__tick" }
     `- channel 关闭          -> 返回 { type: "__shutdown" }

  关键点:
  - 宿主不是去 "查看 VM 里有没有事件"
  - 而是自己维护一条 event channel:
      deliver_event()  -> tx.try_send(...)
      waitForEvent()   -> rx.recv_timeout(...)
  - timeout 的意思只是 "这 50ms 没人投递", 所以给 VM 一个 __tick
  - __pi_wait_for_event() 每次返回后, Rust 侧会 guard.reset()
    所以空闲等待不计入 call_timeout_ms

                                    |
                                    v
[阶段 5: 回收]
session_end / idle_ttl / VM Error
  -> cleanup_instance
  -> shutdown / evict runtime key
  -> 下次再需要时可重建
```

读这张图时，脑子里只要先分清三句话就够了：

1. **Catalog / Registry 决定谁可见，VM 决定谁真的活着。**
2. **tool 调用给 LLM 用，function 调用给宿主自己用，但最后都落到同一台 session VM。**
3. **宿主和 VM 之间不是共享内存式直接互看，而是靠 event channel + hostcall 双向通信。**

## A.2 抽象 ASCII 总图

```text
                         ┌──────────────────────── tomcat 进程 ────────────────────────┐
磁盘三层根               │                                                               │
scope/agent/global ─────►│ 发现/编目 ──► PluginCatalog                                   │
plugin.json + main       │               │                                               │
                         │               ├─ manifest.tools[]      ──► ToolRegistry      │
                         │               │                            (给 LLM)           │
                         │               ├─ manifest.functions[]  ──► FunctionRegistry  │
                         │               │                            (给宿主)           │
                         │               └─ events[] / activation                         │
                         │                                   │                            │
                         │                                   ▼                            │
                         │                        PluginRuntimeManager                     │
                         │                                   │                            │
                         │                    (session_id, plugin_id)                     │
                         │                                   ▼                            │
                         │                         VmActor / PluginVmInstance             │
                         │                                   │                            │
                         │                     pi.* / __pi_host_call(json)               │
                         │                                   │                            │
                         │                            HostApiDispatcher                   │
                         └───────────────────────────────────────────────────────────────┘
```

## A.3 最简组件图

```text
磁盘 plugin/                            tomcat 进程内
├─ plugin.json                          ┌──────────────────────────────────────────────┐
└─ main.ts/js ── load/scan ────────────►│ PluginManager                                │
                                        │   ├─ PluginCatalog                           │
                                        │   ├─ ToolRegistry (给 LLM)                   │
                                        │   ├─ FunctionRegistry (给宿主)               │
                                        │   └─ PluginRuntimeManager                    │
                                        │            │                                 │
                                        │            └─ key = session_id + plugin_id   │
                                        │                    │                          │
                                        │                    ▼                          │
                                        │               VmActor                         │
                                        │                    │                          │
                                        │               PluginVmInstance                │
                                        │                    │                          │
                                        │   globalThis.pi ── __pi_host_call(json)      │
                                        │                    │                          │
                                        │               HostApiDispatcher               │
                                        └──────────────────────────────────────────────┘
```

## 4.1 落地选型决策表

| 维度                     | 关切                                              | 决策                                                                                                                                                                                              | 取自                                                                                                                                                                                                                                                                                                                                                                                           | 入选理由                                                                                                                                                                                                                                                                 | 未入选 + 拒因                                                                                                                     | 说人话                                                                                                                                          |
| ---------------------- | ----------------------------------------------- | ----------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------------------------------------------------------------------------------------------------------------------- | -------------------------------------------------------------------------------------------------------------------------------------------- |
| 运行时                    | 进程内 JS 引擎选谁                                     | **采用 rquickjs；拒绝 WasmEdge / Wasmtime**                                                                                                                                                          | tomcat `src/ext/engine_wasmedge.rs`（现状）；`pi_agent_rust/Cargo.toml`(`rquickjs`)、`src/extensions_js.rs`                                                                                                                                                                                                                                                                                        | 设计：静态编入二进制的嵌入式 QJS；理由：去外部依赖、有 pi_agent_rust 同款先例、迁移风险最低                                                                                                                                                                                                              | WasmEdge：强制 C 库 + 定制 wasm 构建，与运行时深耦（`docs/reports/wasm_runtime_migration_analysis.md`）；Wasmtime：`wasmedge_quickjs.wasm` 不可移植 | 换成焊进肚子里的 QJS，不再依赖外部放映机。                                                                                                                      |
| 隔离（防作恶/跑飞）             | 同进程怎么防插件作恶/死循环                                  | **采用软隔离（超时+中断预算+重建）；拒绝强制硬隔离**（崩溃隔离见下一行）                                                                                                                                                         | tomcat `src/ext/vm_actor.rs`(panic catch)；`pi_agent_rust/src/extensions_js.rs`(interrupt budget)                                                                                                                                                                                                                                                                                             | 设计：interrupt budget+单次超时+fail-open+运行时重建；理由：QJS 无 Wasm 内存墙，软隔离成本低且够用                                                                                                                                                                                                 | 每插件独立进程 / OS 沙箱：IPC 与启动成本高，本期过重（预留扩展点）                                                                                       | 没物理墙，靠超时和重建兜底。                                                                                                                               |
| 崩溃隔离                   | 插件崩了/panic 了怎么不拖垮宿主                             | **采用 每个运行中插件实例专属线程 + `catch_unwind` + 错误态降级；拒绝 让插件错误冒泡到主线程**                                                                                                                                        | tomcat `src/ext/vm_actor.rs:129,165,195`(spawn_blocking 专属线程 + catch_unwind→`VmActorState::Error`)；codex `codex-rs/hooks/src/engine/command_runner.rs`(子进程隔离 hook)                                                                                                                                                                                                                           | 设计：每个运行中插件实例单 `VmActor` 跑在专属 `spawn_blocking` 线程，`run_vm` 用 `catch_unwind` 兜 panic 进 `Error` 态，channel 断开→`__shutdown`，不影响其它插件实例与 Agent Loop；理由：rquickjs 同进程必须靠「线程+panic 捕获+超时」三层兜                                                                                           | codex 全程 OS 沙箱/子进程：IPC 与开发体验重，本期插件量级不需要；裸进程内无 catch：一个 panic 拖垮整进程                                                           | 每个运行中的插件实例单独开间房跑，崩了就标红关掉，不连累隔壁和主程序。                                                                                                      |
| 并发 / 实例模型              | 支持多插件同时跑吗？同插件被多 session 用怎么办                    | **采用 面向对象多实例：运行期 VM 按 `(session_id, plugin_id)` 一键一实例；工具面 / 命令面仍可按 project scope 组织；host-facing function 也复用 scope 视图，但注册面按 `point` 只保留当前赢家；拒绝 全局单 VM 跑所有插件/所有会话**                         | tomcat `src/ext/runtime_manager.rs`(`VmRuntimeKey{session_id,plugin_id}`→`VmActorHandle`，`DashMap`)、`plugin/manager.rs:451`(`start_session_vm` 命中复用/未命中新建)、`:140`(`load_plugin` 现状证据)、`:536`(`end_session` 批量清理)；`src/core/session/scope.rs:49`(Code 模式已有 project scope key)；pi_agent_rust `src/extensions.rs`(per-extension 实例)                                                 | 设计：① 同插件被 N 个会话用 = N 个独立 `VmActor` 实例（JS 堆/状态/事件信箱互隔离）；② 多个不同插件各自独立实例并发；③ LLM 工具面继续按 scope 组织；④ host-facing function 与普通 plugin 共用三层发现，但在 `FunctionRegistry` 里按 `point` 收敛赢家。理由：既保留多会话并发，也让宿主函数视图与项目 scope 对齐且可预测 | 全局单 VM 跑所有会话/插件：状态互染、并发串台、一崩全崩；per-session 各存一份 Registry：重复注册、内存翻倍、列表漂移                                                  | 同一个插件被几个会话用，就给每个会话发一份「独立的它」；宿主函数也跟着当前 scope 看，但同一点位只认一位赢家。                                                                             |
| 多 session 事件路由         | 多会话同时跑，事件/会话调用怎么不串台                             | **采用 `instance_id = session_id/plugin_id` 作 VM 与事件通道键，并把 session_id 透传到 session 类 hostcall；拒绝 全局 current session**                                                                              | tomcat `src/ext/runtime_manager.rs`(`VmRuntimeKey{session_id,plugin_id}`)、`dispatcher/dispatch.rs:197,294`(按 instance_id 注册/投递事件)、`dispatcher/session_ops.rs:16`(`current_session_key()` — 现状缺陷)；pi_agent_rust `src/extensions.rs`(per-extension 隔离)                                                                                                                                         | 设计：VM 与事件通道已按 `session_id+plugin_id` 双键隔离；新增从 `instance_id` 解析 `session_id` 注入 `session_ops`，按会话取数；理由：架构本就多 session 并发，事件已隔离但会话读写仍走全局 current，必须修正                                                                                                                   | 全局单 current session：多 session 并发会读串/写错会话（现状 `session_ops` 缺陷）；每 session 独立 dispatcher：重复基础设施、浪费                              | 每个会话+插件有独立放映厅和事件信箱；但「读当前会话」现在用全局变量，并发会读串，得改成按信箱上的会话号取。                                                                                       |
| 插件 vs 工具               | 插件本身就是一个工具吗                                     | **采用 插件=能力容器（可同时贡献 tools/functions/events/commands）；拒绝 插件=单个工具或单个 function**                                                                                                                             | tomcat `docs/architecture/plugin-system/plugin-source-scan-register-load.md:343-347`（插件注册表与工具系统分层）、`src/ext/dispatcher/ops.rs:164`(`do_register_tool`)、现有架构里 tools 与事件/命令已分层；本版在其上新增 host-facing function 面                                                                                                                                                | 设计：插件加载后既可通过 `pi.registerTool` 贡献 0..N 个工具，也可通过 `pi.registerFunction` 暴露 0..N 个宿主函数，并继续挂事件/命令；理由：一个插件天然可能同时有“给 LLM 的能力”和“给宿主的内线接口”                                                                                           | 「一插件=一工具/一函数」：无法表达多工具、多函数、事件钩子、命令，且会把不同受众（LLM/宿主）混成一类                                                                   | 插件是个「能力包」，进去能摆工具、留内线函数、挂事件和命令，不是某一个具体能力。                                                                                                           |
| 能力注入 LLM               | 插件清单怎么给 LLM、LLM 怎么用，跟工具一样吗                      | **采用 只有插件注册的 tools 走共享 `ToolRegistry`，与内置工具同一 tool-calling 通道注入 LLM；functions 不进 LLM；拒绝 像 skill 那样把插件清单做渐进式披露，也拒绝把 host-facing function 暴露成 tool**                                                                                      | tomcat `src/ext/dispatcher/ops.rs:174`(`tools.register_tool`)、`src/api/chat/context.rs:286`(`DefaultToolRegistry`)、`docs/architecture/skill-system.md`(skill 走 `<available_skills>` 渐进披露 — 对比)；`registerTool` 若复用来承载宿主函数，会误进 `ToolRegistry` 形成污染                                                                                                                           | 设计：插件 manifest 本身不进 LLM；其注册的工具以 tool spec 形式进 LLM 工具列表，LLM 按需 function-call，与内置工具完全同路；host-facing function 则留给宿主内部调用。理由：复用既有 tool-calling，同时避免把系统内线接口暴露给模型                                                                 | 把 manifest name+desc 注入系统提示（skill 路线）：插件能力是「可执行工具」而非「惰性指令正文」；把 function 伪装成 tool：运行期会进 ToolRegistry 污染 LLM 工具面 | 给模型看的只有工具；系统自己用的函数绝不摆上工具架。 |
| 宿主函数面（本版新增）          | 不给 LLM、只给宿主自己调用的能力怎么建模                           | **采用 `manifest.functions[]` + `pi.registerFunction(name, handler)`；manifest 只承载宿主可见的最小契约，启动时按 `point` 分发到宿主扩展点注册表；发现 / 安装路径复用 `project > agent > managed` 三层，但注册面按 `point` 做 override；拒绝 复用 `tools[]` / `pi.registerTool` / 每种能力各开一个顶层 manifest 字段**                                                                                                      | tomcat 现状 `ToolRegistry` 只服务 LLM tool-calling（`context.rs`、`ops.rs`）；`registerTool` 运行期会进 `ToolRegistry`，若复用将污染 LLM 工具面；本版新增 host-facing function 面与 point-dispatch 机制                                                                                                                                     | 设计：宿主函数与 LLM 工具是两类受众，必须分账；`functions[]` 让宿主在编目期静态知道“这插件提供了哪类 host-facing capability、入口函数叫什么”，`pi.registerFunction` 让运行期把实现绑定进 VM；进入 `FunctionRegistry` 前再按 `point` 选高层赢家。                                                                                                      | 复用 `registerTool`：运行期会把 host-facing 能力错误暴露给 LLM；按能力种类新增 `webSearchBackends[]`/`rerankers[]` 顶层字段：扩展一多 manifest 会碎裂                                               | 给模型看的叫工具；给系统自己用的叫函数，而且这些函数会先按点位选出当前 scope 的赢家。 |
| 工具调用事件 / 生命周期观察      | 第三阶段插件工具调用要发哪些事件？只复用 start/end 够不够？                    | **采用 复用现有两套事件语义，不新造 `tool_call_start` / `tool_call_end` 字面量；阶段三最小完整集为 `tool_execution_start`(AgentEvent, UI/观察) → `tool_call`(ExtensionEvent, 执行前钩子) → `[可选] tool_execution_update`(长耗时/分段进度) → `tool_result`(ExtensionEvent, 执行后结果，含 `isError`) → `tool_execution_end`(AgentEvent, 生命周期收口，含 `isError`)；`tool_call_streaming` 仅作 LLM 参数流式到齐前的预告，非阶段三必需；拒绝 只发 start/end 两个事件 或 另造 `tool_call_error` / `tool_call_cancelled` / `tool_call_start` / `tool_call_end` 并行命名体系** | tomcat `docs/architecture/plugin-system/events.md:120-147`（观察向 `tool_execution_*` 与钩子向 `tool_call` / `tool_result` 分离、顺序明示）、`src/infra/events/mod.rs:85-95`(`WIRE_TOOL_EXECUTION_START/END/UPDATE`、`WIRE_TOOL_CALL`、`WIRE_TOOL_RESULT`、`WIRE_TOOL_CALL_STREAMING`)、`src/core/agent_loop/tool_dispatcher.rs:40-54,170-180,217-229,238-255`（现状事件时序与 cancel 时至少发 `tool_execution_end` 配平 UI） | 设计：阶段三插件工具调用与内置工具调用应共享同一套事件口径，便于 UI/日志/审计/插件 hook 一起复用；`tool_execution_start/end` 负责**观察**生命周期，`tool_call`/`tool_result` 负责**业务钩子**语义，长任务再按需补 `tool_execution_update`；失败不另起事件，用 `tool_result.isError` 与 `tool_execution_end.isError` 表达；中断场景至少保证 `tool_execution_end` 收口配对，必要时结果文本写 `[interrupted]` | 只发 start/end：看得到开闭但拿不到前置钩子和结果 payload；另造 `tool_call_start/end`：与现有 `tool_execution_*` / `tool_call` / `tool_result` 语义重复，割裂消费者；再拆 `tool_call_error` / `tool_call_cancelled`：状态面膨胀、与现有 `isError` / interrupted 收口重复 | 要发，但别另造一套方言。沿用系统现成的五步语义就行：开始、开跑前通知、过程更新（可选）、结果、结束。失败塞进 `isError`，中断至少补一个 `end` 把 UI 和日志配平。 |
| 工具调用执行路径               | LLM 返回 tool_call 后插件工具怎么执行？要不要单独 `execute_plugin_tool`，阶段三步骤是否都串在一个函数里 | **采用 统一 tool-calling 入口（Agent Loop `execute_tool` 单点分发）+ 插件分支专属 `PluginToolExecutor`（即 `ToolExecutor` 的插件实现）；插件工具的「ensure VM → `__pi_execute_tool` → hostcall → 封装结果」全链路收敛在 `PluginToolExecutor.execute` 一处；拒绝 让插件工具旁路 Agent Loop 自起并行执行通道 / 把阶段三步骤散在 `PluginManager` 各处** | tomcat `src/core/tools/contract/registry.rs:24-34,43-48,127-160`(`ToolExecutor::execute` / `ToolRegistry::call_tool` 注入式执行)、`src/core/agent_loop/`(`execute_tool` 统一入口)、`src/ext/runtime_manager.rs`(`get_or_start` VM)、`assets/.../pi_bridge.js`(`__pi_execute_tool`)；pi_agent_rust `src/extensions.rs`(per-extension execute) | 设计：LLM 的 tool_call 一律先进 Agent Loop `execute_tool` 总闸；内置工具→`PrimitiveExecutor`，插件工具→经 `ToolRegistry.call_tool` 落到注入的 `PluginToolExecutor.execute`，阶段三 VM 生命周期全在这一处串联；理由：统一入口才能让内置/插件工具共享同一套中断(abort)、引导(steering)、审计、展示与结果封装，插件特有的 VM 编排对 Agent Loop 透明 | 旁路 Agent Loop 自起执行通道：丢失统一中断/审计/展示，且要重复实现工具结果协议；把阶段三步骤散在 `PluginManager` 各处串：职责漂移、难测 | LLM 喊工具名，统一从 Agent Loop 那个「执行工具」总闸进；是插件工具就转给「插件执行器」一把梭——确保 VM 在 → 调 `__pi_execute_tool` → 收结果，全在这一个函数里串完，不另开野路子。 |
| 宿主函数调用路径 / 命名           | 宿主自己要调用插件能力时，走哪条路、名字怎么定                         | **采用 与 tool 平行的一条宿主函数调用链：point-dispatch / 当前赢家函数 → `PluginFunctionInvoker.execute` → `__pi_execute_function({functionName, params})`；函数名用宿主稳定契约名（如 `webSearchBackend`），拒绝 `webSearchBackend.mimo` 这种按厂商拆成多个函数**                                                                                               | `ToolRegistry`/`PluginToolExecutor` 现有工具链可复用其 VM 生命周期与等待结果机制；`web_search` backend 需求要求宿主“只知道这是 web search 后端能力”，不应感知厂商枚举；point override 已保证同一 scope 下只消费赢家视图                                                                                                                                          | 设计：function 面的消费者是宿主而不是 LLM，所以函数名应代表**宿主契约**而不是 vendor；宿主把 `backend="mimo"|..."auto"` 作为参数传入，插件内部自己路由显式后端并维护 auto/fallback 策略；当前 scope 若该赢家返回 `unsupported_backend`，宿主直接判定不兼容，不再跨插件兜底                                                                 | 每 vendor 一个函数名：宿主必须知道所有厂商、注册面膨胀、auto 排序难以下沉到插件；继续借用 `ToolRegistry`：函数会暴露给 LLM；把候选函数也硬塞进 `FunctionRegistry` 的 name→plugin 唯一映射：多候选函数并存时不自然 | 宿主只会说“给我做 web search backend 这件事”，不需要知道里头到底是 MiMo 还是以后别的厂商；它只认当前 scope 的赢家函数。 |
| 工具→插件路由 / 命名           | 一个插件可注册多个工具、多插件可能重名，调用时怎么定位到「某 plugin 的某 tool」并落到对的 VM | **采用 注册期工具名在 scope 内全局唯一（撞名即拒/告警）；调用期路由链 `toolName → ToolRegistry.get_tool → Tool.plugin_id →（当前回合 session_id + plugin_id）→ RuntimeManager 取/起 VM → __pi_execute_tool({toolName, params})`；VM 内部按裸 `toolName` 查 `__pi_tools`（单插件内不撞）；拒绝 让 LLM 直面 `plugin_id::tool` 复合名 / 靠 `get_tool` 取首个匹配蒙** | tomcat `src/core/tools/contract/registry.rs:13-22`(`Tool.plugin_id`)、`:52-54`(`plugin_id::name` 键)、`:84-88`(`get_tool` 现状按裸名取首个=撞名隐患)、`:110-114,127-160`(`get_tool`/`call_tool`)、`src/ext/runtime_manager.rs`(`VmRuntimeKey{session_id,plugin_id}`)、`assets/.../pi_bridge.js`(`__pi_execute_tool` 用 `__pi_tools[toolName]`) | 设计：`Tool` 自带 `plugin_id` 且按 `plugin_id::name` 存储，天然可由 `toolName` 反查归属插件；只要暴露给 LLM 的名字在 scope 内唯一，host 即可用 name→plugin_id 精确定位，再以当前回合 `session_id` 合成 `VmRuntimeKey` 投到正确会话的 VM；`ensure/start_session_vm` 只保证 VM 就绪、**不**注册工具（注册在阶段二） | 让 LLM 面对 `plugin::tool` 复合名：污染 tool spec、对模型不友好；`get_tool` 现状「按裸名取首个启用项」：跨插件重名会静默路由到错误插件（须改为唯一性约束） | 一个插件能注册好几个工具；调用时靠「工具名」反查它属于哪个插件，再用「会话号+插件号」找到对应那间放映厅，把工具名递进去执行。前提是同项目里工具名别重，真撞了注册时就报错，而不是闷头调第一个。 |
| 插件加载路径                 | 是否参考 skill 三层加载                                 | **通用插件发现层复用 skill 三层磁盘根（P0 project > P1 agent > P2 managed）+ `PluginCatalog`/ScopeRegistry 分层；host-facing `functions[]` 不再搞单源例外，但会在注册面按 `point` override 收敛赢家**                                                                                                  | tomcat `docs/architecture/skill-system.md`(P0→P2 三层 first-wins)、`plugin-source-scan-register-load.md:237-263`(GlobalCatalog+AgentRegistry+SessionContext 推荐，待实现)、`src/ext/plugin/manager.rs:140`(现状目录直载)；openclaw 同文档:47(bundled/workspace/global/config 多源)                                                                                                                                 | 设计：tool/event/function 共享同一套三层发现根，避免安装面与发现面分裂；宿主函数的差异只放在 `FunctionRegistry` 物化阶段，而不是再造一套例外目录规则。                                                                                  | 方案 A 目录直载（现状）：缺 catalog/策略层，多项目 scope 隔离弱；把三层发现做成 candidate-union：高层覆盖不清晰、调用方语义飘移                                                          | 发现路径统一三层找；真正的特殊规则发生在“同一点位谁赢”这一步。                                                                               |
| 注册面作用域 / 多 session 可见性 | ToolRegistry、宿主扩展点注册表 和 plugins 表是每个 session 各看各的，还是共享一份；Catalog 呢；能不能预算一份 base Registry | **采用 分账作用域：`ToolRegistry` / `plugins表` / `FunctionRegistry` 都按 project scope 组织；运行实例 `VmActor` 才 per-`(session,plugin)` 隔离。其中 `FunctionRegistry` 复用三层发现结果，但按 `point` 只保留当前赢家。**拒绝 每 session 一份独立 registry / 拿 candidate-union 直接暴露给宿主调用方** | `src/core/session/scope.rs:49`(Code 模式已有 project scope key)、`src/api/chat/session_runtime.rs:64`(`SessionRuntimeRegistry` 已有 registry 形态占位)、`src/api/chat/context.rs:286`(现状 `ToolRegistry` 由 chat 侧持有)                                                                                         | 设计：tool 面和 function 面受众不同，但两者都应该随 project scope 复用；区别只在函数面先做 `point` 级收敛。运行期实例继续按 `session_id + plugin_id` 隔离，互不串状态。                                                                                                            | per-session 各存一份 Registry：重复注册、内存翻倍、列表会话间漂移；把 candidate-union 直接交给宿主：调用语义不可预测                                                          | 不是所有注册面都得一模一样。工具那边按项目看；函数这边也按项目看，但同一点位先选赢家。                                                                             |
| 四张表分层 / plugins表 写入时机          | `plugins表` 是干嘛的？跟 Catalog/ToolRegistry/宿主扩展点注册表/RuntimeManager 啥区别？什么时候写？manifest 静态声明后能不能扫盘就填满？ | **采用 分层四表，但把 tool 面与 function 面分账：① 通用 `PluginCatalog` 仍可承载插件静态元信息；② `plugins表` 继续承担管理态；③ `ToolRegistry` 负责 LLM 可见的工具面；④ `FunctionRegistry` 负责宿主函数的当前赢家视图。`functions[]` 与 `tools[]` 共享发现根，但进入 `FunctionRegistry` 前先按 `point` override 收敛。**拒绝 把 `events[]` 与 `event_listener_ids` 混为一谈 / 把 `registered_tools`、`registered_functions` 再拆双账 / 把 candidate-union 直接暴露给宿主调用方** | tomcat `src/ext/plugin/manager.rs:90`(`plugins: RwLock<HashMap<String,PluginInstance>>` 现状单层全局——待拆 Catalog/per-scope)、`:246-258`(`load_plugin` 现状即造实例+`register_plugin`)、`:358-366`、`:380-408`(`enable`/`disable` 翻 status)、`:595-597`(`unload` 现状按 plugin_id 批量 `remove_plugin_listeners` + `unregister_plugin_tools`)、`types.rs:37-49`(`PluginInstance` 字段)、`infra/event_bus/mod.rs:297-332,397-416`(`add_listener` 返回 `EventListenerId`; `remove_plugin_listeners(plugin_id)` 批量清)、`src/core/tools/contract/catalog.rs:91`(内置 `BUILTIN_TOOL_CATALOG`=编译期 const, 与本表无关) | 设计：把「不可变(manifest 派生, 可共享, 扫盘即知)」与「可变(状态/配置/清理账本)」分层后，`tools[]` 和 `functions[]` 都可静态声明且共享三层发现根；真正的差异只放在注册面：工具面保留完整可见集，函数面按 `point` 收敛赢家。`event_listener_ids` 只在插件运行期真挂上宿主 EventBus 时才有值；现状卸载主要靠 `plugin_id` 批量 remove，不靠逐个 ID 清理 | 单层全局 plugins表(现状)：职责混杂；把 `events[]` 当成 `event_listener_ids`：把“声明想吃什么事件”与“运行时真正挂上的监听句柄”混成一层；把 candidate-union 直接暴露给宿主：调用语义不稳定 | 要分表，也要分账。工具和函数都能静态声明，但宿主函数进入表前先按点位选赢家。 |
| 加载策略 / 资源占用（插件很多）      | 插件很多时启动就全量加载进内存是否太重、怎么省                         | **采用 三段式「发现编目(只读 manifest,不跑码) → 工具元信息(manifest 静态 `tools[]` 直接作为 LLM 契约面) → 运行实例懒加载(VmActor 首次使用才建)+idle 回收」；legacy `registerTool` 仅作兼容迁移/实现自报，不再作为第二套工具来源；拒绝 启动对所有插件 eager 跑 `run_script` 全量加载**                                | openclaw `src/plugins/discovery.ts`+`manifest-registry.ts`(只读 manifest 建 catalog)、`loader.ts:1348`(`loadModules:false`)、`tools.ts:671`(optional tool 首次调用懒加载单插件)；pi_agent_rust `src/resources.rs:348`(发现只收路径)；pi-mono `resource-loader.ts:348`(启用集 eager 全量——反例)；tomcat `plugin-source-scan-register-load.md:237-263`(GlobalCatalog 已规划)、`runtime_manager.rs`(VmActor 已按需建/`end_session` 回收) | 设计：① 编目只读 JSON 建轻量 Catalog（不 import、不跑 JS）；② 工具元信息以 manifest 静态声明为准（不跑码即可见，对齐 openclaw `contracts.tools`）；③ 运行实例（VmActor）维持「首次使用才建、命中复用、`end_session` 回收」懒加载并加 idle TTL。常驻内存只有轻量 Catalog + 少数在用 VM，不随插件总数线性膨胀；legacy `registerTool` 只是迁移兜底，不再和 manifest 并列成两套真相                               | 启动 eager 全量跑 `run_script`（pi-mono/pi_agent_rust 启用集做法）：插件多时启动慢、内存高                                                           | 别一开机就把所有插件代码都拉起来跑。开机只读每个插件「身份证(manifest)」建一张轻量目录；工具信息就以 manifest 里静态报的那份为准（不跑码就知道有啥）；真正的「活体 VM」等到某会话用到才开、闲了就回收。这样装一百个插件，内存里也只有目录 + 正在用的那几个。 |
| 加载/激活的触发时机（启动 vs 会话进入 vs 首次使用） | 插件到底什么时候加载？「阶段二」到底什么时候触发？ | **采用 三个加载点（仅 1 个不跑码 + 2 个跑码），但把 tool 面与 function 面分开描述：① 程序启动＝阶段一编目，只读 manifest，`functions[]` 与 `tools[]` 一样进入三层 catalog；② 会话进入 / scope 首次激活时，工具面 materialize 到 `ToolRegistry`，函数面则按 `point` override 物化到宿主扩展点注册表；③ 首次 `tool_call` / 宿主 function call＝执行期，已可见的能力被点名时起/复用长跑 VM 执行。host-facing `functions[]` 本版要求静态声明，不靠阶段二补发现。** | tomcat `src/ext/plugin/manager.rs:140`(现状 load 即 `run_script`——待改懒激活)、`:451`(`start_session_vm` 命中复用/未命中新建)、`:509`(`dispatch_session_event`→`deliver_event`)、`src/ext/plugin/types.rs:11-21`(manifest 现状无 `tools[]`/`functions[]`/`events[]`/`activation`，待加)、`vm_actor.rs:138-204`(`_start`→`waitForEvent` 长阻塞)、`docs/architecture/plugin-system/phase2-long-lived-vm.md`(session_start lazy create / session_end shutdown)、`infra/event_bus/mod.rs:297-332,397-416`(`EventListenerId` 运行时分配；按 plugin_id 批量 remove)、openclaw `contracts.tools`(静态声明工具不跑码即可见)、`src/core/session/scope.rs:49`(scope key) | 设计：把“加载点”按是否跑码拆清后，tool 面与 function 面的差异也顺手钉死：工具可见性仍可按 scope materialize；宿主函数同样先进入 scope 视图，但真正暴露给宿主前要先做 `point` 级收敛。静态 `tools[]` 的插件**完全不经过阶段二**；host-facing function 也天然靠 `functions[]` 静态可见；生命周期型插件是否预启动只看 `activation:"session"`，无论 `events[]` 列了几个事件名，都不改变这个开关 | 把阶段二画成"一→二→三"必经线性阶段：误导；把 candidate-union 直接暴露给宿主：调用语义难以解释；把阶段二拖到 `tool_call`：鸡生蛋，时序不成立 | 工具和函数别混着讲：工具那边仍有会话进入时的可见性物化；函数这边也会物化，但先按点位选赢家。 |
| 插件“类型”判别 / `activation` 与 `tools[]` 正交 | `activation:"session"` 也可能没有静态 `tools[]`，纯工具型 vs 生命周期型怎么区分？要不要加一个“类型”标识？ | **采用 不引入 plugin `type` 单一枚举；用两个**正交** manifest 字段判别：`tools[]`(有无→工具可见性 + 是否要跑阶段二) 与 `activation`(`"session"` vs 默认 `"lazy"`→长跑 VM 是否在 scope 进入时预启动)。`functions[]` 只是另一条**静态宿主契约面**，不参与这个判别。二者互不蕴含，组合出 4 种行为；其中只有 `(activation=lazy ∧ 无静态 tools[])` 需单独起「阶段二」短命 VM，`(activation=session ∧ 无静态 tools[])` 的工具登记由预启动的长 VM 顺带完成。`events[]` 只是“订阅哪些事件名”的声明，不参与这两个判别；拒绝 用 `{tool｜lifecycle}` 单一类型枚举（会和“既给工具又订阅事件”的插件冲突）/ 拒绝 用 `events[]` 是否非空当预启动开关** | tomcat `src/ext/plugin/types.rs:11-21`(manifest 待加 `tools[]`/`functions[]`/`activation`)、`src/ext/plugin/manager.rs:451`(`start_session_vm` 预启动)、`docs/architecture/plugin-system/phase2-long-lived-vm.md`(session_start lazy create)、`infra/event_bus/mod.rs:81-83`(`EventListenerId`)、openclaw `contracts.tools`/`activation` | 设计：插件本就可能**同时**贡献工具、宿主函数与订阅生命周期事件，单一类型枚举无法表达这种叠加；改用两条独立开关后，4 种组合(static×{session,lazy} / legacy×{session,lazy})都自洽，且把“阶段二短命 VM”精确收敛到唯一一格(lazy∧legacy)。`activation` 是“何时必须让常驻 VM 在场”的权威开关，`tools[]` 是“对 LLM 暴露面”的权威契约，`functions[]` 是“对宿主暴露面”的静态契约，三者互不替代 | 单一 `type:{tool,lifecycle}` 枚举：无法表达“既给工具又听事件”，且与 `tools[]`/`activation` 语义重叠；用 `events[]` 非空当预启动开关：把“想吃什么事件”误当“何时起 VM”，且无法表达“只想首个 tool_call 才起、但运行期也会 on 事件”的插件 | 别给插件贴“它是工具还是生命周期”这种单一标签——一个插件完全可能既给工具、又留宿主函数、又要听事件。改用独立开关：`tools[]` 管 LLM 可见性，`functions[]` 管宿主可见性，`activation` 管何时预启动常驻 VM。 |
| 能力暴露                   | 敏感能力怎么给插件                                       | **采用 `pi.*` 单入口 hostcall + 权限闸；加载期 `requiredPermissions` 当前默认放行，但保留 `confirm_permissions` 扩展点；拒绝 ambient Node 模块与本期交互式授权弹窗**                                                                                                                         | tomcat `src/ext/dispatcher/dispatch.rs`、`host_binding.rs`、`src/ext/plugin/manager.rs`、`src/api/chat/context.rs`、`src/api/cli/plugin_cmd.rs`；codex `codex-rs/core/src/mcp_tool_call.rs:702`(sandbox 收口)                                                                                                                                                              | 设计：真正敏感的 fs/net/exec/会话能力仍统一经 dispatcher 过闸审计；加载期先不加一层额外弹窗，避免当前插件系统在 CLI/chat 两条路径行为分叉；后续若建设更细的插件权限系统，直接复用 `confirm_permissions` 注入链                                                                                                                                              | 直接给 `node:fs`/`node:http`：ambient 授权破坏沙箱；加载期立即强制交互弹窗：当前体验重且与 chat 运行时不一致                                                                 | 敏感活还是只能敲安检窗口；清单上的权限声明本期先记账、默认放行，后面再把真正的插件授权系统接上。                                                                                      |
| 工具垫片                   | 纯计算工具放哪                                         | **采用 `pi_runtime_prelude.js` 内联纯 JS 基线 + `pi_node_shim.js` fail-closed alias + 少量工具 shim（`@sinclair/typebox` / `ms`）+ crypto 同步原生（不进 dispatcher）**                                                                                                                  | tomcat `assets/js/pi_runtime_prelude.js`、`assets/js/pi_node_shim.js`、`assets/js/pi_typebox_shim.js`、`src/ext/crypto_native.rs`                                                                                                                                                                                                 | 设计：path/util/events/Buffer/编码/console/timers 全在 prelude；`node:*` import 只做 alias 或 fail-closed 拒绝；`typebox`/`ms` 仅保留为轻量工具级兼容；crypto 用 `Func::from` 同步原生。理由：纯算/轻工具不该为每次调用绕 dispatcher 一圈                                                                                                                                 | 全部走异步 hostcall：拼字符串也要往返 Rust；继续无条件注入 pi-mono UI/AI/sandbox 大 shim：维护成本高且无运行时证据支持                                                                      | 小算盘自己打，别每次都跑去柜台；但也别再把整套 pi-mono 道具箱跟着塞进 VM。                                                                                                      |
| 运行时全局能力分层注入          | B.1 那批"宿主注入函数"（console/timers/TextEncoder/Buffer…）算哪一类、由谁保证、写进决策了吗 | **采用 五层注入分类，运行实例初始化时一次性装好：① `pi_runtime_prelude.js` 保证 `console` / `timers` / `TextEncoder` / `TextDecoder` / `path` / `util` / `events` / `Buffer`；② `pi_node_shim.js` 提供 `node:*` alias 与 fail-closed `fs`/`child_process`/`os`；③ 轻量工具 shim：`__pi_typebox` / `__pi_ms`；④ 同步原生函数（`Func::from` 挂全局、**不进 dispatcher**）：`__pi_crypto_*_native`(hash/hmac/random/aes-gcm/ed25519)；⑤ 敏感/异步能力（`globalThis.pi.*`→`__pi_host_call` 过权限闸）。拒绝 把①②写成"rquickjs 天然自带" / 让 crypto 走 dispatcher / 把敏感能力做成 ambient 全局** | tomcat 本文 B.1(291-307)、§4.2 P2/P4、`assets/js/pi_runtime_prelude.js`、`assets/js/pi_node_shim.js`、`assets/js/pi_crypto_shim.js`、`src/ext/crypto_native.rs` | 设计：把"宿主注入函数"按"谁来保证 + 走不走 dispatcher"分五档——prelude 基线、node alias、轻工具 shim、crypto 同步原生、敏感能力 hostcall。理由：与「工具垫片」「能力暴露」两行的开销/安全分流对齐，并消除“以为 rquickjs 裸运行时天然就有 console/编码 API”的隐性依赖                                                                                                                                   | 把 console/TextEncoder 当 QJS 天然自带：裸运行时不提供；crypto 走 dispatcher：纯算往返浪费；敏感能力做 ambient：破坏沙箱                                                                        | 插件里能直接用的全局不是一坨黑箱，而是分层注入：prelude/alias/小工具/同步原生/hostcall 各有各的边界。                                                                                          |
| Node 兼容                | 要不要兼容层                                          | **采用 Tier-A 轻量能力 + fail-closed `node:*` alias（`path`/`util`/`events`/`buffer`/`crypto` 映射到宿主注入对象，`fs`/`child_process`/`os` 明确拒绝）+ 少量工具 shim（`typebox`/`ms`）；拒绝整套 Node 兼容层**                                                                                     | tomcat `assets/js/pi_runtime_prelude.js`、`assets/js/pi_node_shim.js`；`pi_agent_rust/src/extensions_js.rs`(105 模块，反例)                                                                                                                                                                                                                                                                  | 设计：保留最常见的轻能力与 import 习惯，但对真正敏感的 Node 模块 fail-closed。理由：既照顾零构建 TS 插件的基础 ergonomics，又不回到旧 `assets/modules/` 那套整箱兼容层                                                                                                                                                           | 搬整套 105 模块：与「放弃 pi-mono」自相矛盾、维护重                                                                                             | 给常用小工具留门，把危险大模块堵死，不再背整箱 Node 道具。                                                                                                              |
| pi-mono 对齐             | 是否硬兼容其插件                                        | **拒绝硬兼容；自有 manifest + 裁剪 `pi.*`，仅保留少量与当前运行时直接相关的轻工具 shim；`@mariozechner/pi-tui` / `pi-ai` / `pi-coding-agent` / `@anthropic-ai/sandbox-runtime` 不再做运行时注入**                                                                                               | tomcat `src/ext/instance_rquickjs.rs`、`src/ext/ts_compiler.rs`、`docs/architecture/plugin-system/js-api-alignment.md`、`pi-mono-compat-strategy.md`                                                                                                                                                                                                                                  | 设计：保留 `pi.*` 命名习惯但裁剪子集；纯 pi-mono UI/AI/agent/sandbox 兼容层既无仓内运行时测试，也与当前用户文档宣称的“只保留少量轻能力”不一致，因此直接退出默认运行时注入面                                                                                                                                                             | 全量对齐 `ExtensionAPI`：30+ 事件 + UI 渲染面，超出 tomcat 需要                                                                             | 借个顺手的名字，但不再承诺把人家整套插件生态原样搬进来。                                                                                                                |

## 先看什么

推荐按下面顺序阅读：

1. [`plugin-system/plugin-source-scan-register-load.md`](./plugin-system/plugin-source-scan-register-load.md)：先把“发现 / 激活 / 运行”三层分开。
2. [`plugin-system/js-bridge-and-host-api.md`](./plugin-system/js-bridge-and-host-api.md)：再看 `pi.*`、事件推送、ctx 代理和宿主 API 分层。
3. [`plugin-system/host-call-protocol.md`](./plugin-system/host-call-protocol.md)：接着看 `HostRequest` / `HostResponse`、manifest、`tools[]` / `functions[]`。
4. [`plugin-system/runtime-and-sandbox.md`](./plugin-system/runtime-and-sandbox.md)：最后看 `VmActor`、长生命周期 VM、隔离和回收。
5. [`plugin-system/events.md`](./plugin-system/events.md)：如果你关心事件线格式和 hook 语义，再单独下钻。
6. [`package-manager.md`](./package-manager.md)：如果你关心安装、账本和三层路径，再看这份。
7. [`../../src/ext/README.md`](../../src/ext/README.md)：需要从文档跳到代码时，再看实现侧地图。

## 边界约束

- **插件是能力容器，不是宿主捷径**：敏感能力必须走 `pi.*` → `__pi_host_call` → `HostApiDispatcher`。
- **静态声明优先**：`tools[]` / `functions[]` 先决定“谁可见”；运行时注册负责把“具体哪段 JS 实现可被执行”绑到当前 VM。
- **安装与运行分离**：`tomcat install` / `/install` 负责写三层目录和账本；真正执行插件代码发生在显式加载、`session_start` 或首次使用时。
- **父页负责导航、专题页负责单一事实源**：本页只收口入口、导图和关键决策；细节分别下钻到子文档。

## 历史决策

- ~~WasmEdge + `wasmedge_quickjs.wasm` 作为默认插件沙箱~~ → 已放弃；当前实现统一为进程内 `rquickjs`。
- ~~维护完整 pi-mono / npm 兼容层~~ → 已放弃；当前只保留运行时真正需要的轻量 shim 与 fail-closed alias。
- ~~为历史方案保留单独归档目录~~ → 已放弃；必要背景只保留在现行文档的历史决策小节，避免第二套阅读路径。
