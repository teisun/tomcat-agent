# 运行时、VM 生命周期与沙箱隔离

本文为 [Architecture](../../openspec/specs/Architecture.md) 中「4. 插件系统（统一入口）」的专题页，补充 [`../plugin-system-overview.md`](../plugin-system-overview.md) 的运行时物化、`VmActor` 生命周期与隔离策略。

## 背景与动机

短生命周期 VM 可以解决“插件脚本能不能跑”这类问题，但一旦插件需要**跨调用持久状态**，事情就变了：

| 插件场景 | 为什么短命 VM 不够 | 说人话 |
|----------|-------------------|--------|
| `todo` / `plan` / `git-checkpoint` | 需要跨多次工具调用保留内存态 | 每次都重建 VM，就像每次开会都把白板擦干净。 |
| `session_start` 初始化数据 | 初始化结果要供后续事件 / 工具调用复用 | 不能每次来事件都重跑整套初始化。 |
| `setInterval` / 会话级轮询 | 定时器要跨多个事件持续存活 | 短命 VM 根本留不住“还在转”的定时器。 |

所以当前运行时必须同时支持两类实例：**短命校验 VM** 和 **长生命周期 session VM**。

## 当前运行时对象

| 对象 | 职责 | 说人话 |
|------|------|--------|
| `PluginEngine` | 全局 QuickJS 引擎与实例创建入口 | 这是总引擎，不是具体某一场放映。 |
| `PluginVmInstance` | 单次脚本拼装、注入宿主全局、短命执行或 session VM 启动 | 真正承载一份 JS 运行实例。 |
| `PluginRuntimeManager` | 按 `(session_id, plugin_id)` 管理长生命周期实例 | 这是“会话 + 插件”活体的总账本。 |
| `VmActor` | 每个长跑 VM 的专属线程、命令通道与事件循环 | 这是某一场真正长期运行的放映厅。 |

## 两类运行时实例

### 1. 短生命周期实例

用于：

- 加载期校验脚本可否运行
- 必要时完成轻量初始化
- legacy 场景下补做一次工具登记
- 不进入长期事件循环

它的特点是：

- 生命周期短
- 不承载 session 期状态
- 适合做“脚本能不能起、manifest 与实现是否一致”的快速收口

### 2. 长生命周期 session VM

用于：

- 接收 `session_start` / `session_end` 等生命周期事件
- 处理真正的插件工具调用与宿主函数调用
- 维持会话期内的插件状态
- 持续运行事件循环

它的特点是：

- 按 `(session_id, plugin_id)` 隔离
- 命中已有健康实例时直接复用
- 空闲时由 `idle_ttl_ms` 机会式回收，而不是后台 sweeper 常驻扫描

## 为什么当前选择 `waitForEvent` + Actor 模型

当前文档保留的定稿不是“宿主每次主动注一段 JS 进去跑”，而是：

1. VM 内部长期阻塞等待下一条事件；
2. 宿主把事件投进 channel；
3. `VmActor` 线程内独占 `Vm`；
4. 耗时 hostcall 继续复用 `submit/poll`。

| 候选方案 | 定稿 | 说人话 |
|----------|------|--------|
| 宿主主动 `eval` / 注入 JS | 不采用 | 宿主每次往 VM 里塞代码，边界太糊，也更难把生命周期和线程模型讲清楚。 |
| `waitForEvent` + `VmActor` | **采用** | 让 VM 自己常驻等事件，宿主只投递命令，职责更清楚。 |

## 运行时闭环

```text
install / discover
  -> 只更新 catalog / layered registry
  -> 不运行插件代码

scope materialize
  -> 形成当前 scope 的可见插件集
  -> 静态 tools[] / functions[] 可进入可见面

runtime activation
  -> session_start 或首次使用
  -> ensure/start_session_vm(session_id, plugin_id)
  -> spawn VmActor if needed

session end / idle ttl
  -> shutdown / evict
```

## 长生命周期 VM 的核心结构

```text
RuntimeManager
  └─ key = (session_id, plugin_id)
       └─ VmActorHandle
            ├─ cmd_tx
            ├─ state
            └─ dedicated blocking thread
                 └─ owns PluginVmInstance
```

这意味着：

- 外部不直接拿着可变 `Vm` 到处跑；
- 对 VM 的初始化、投递事件、执行命令、关停都走 actor 命令通道；
- 真正执行 JS 的线程边界是稳定的。

## 生命周期状态机

```text
Created -> Running -> Idle -> Running
   |         |         |        |
   |         |         |        -> Error
   |         |         -> ShuttingDown -> Stopped
   |         -> ShuttingDown -> Stopped
   -> Error
```

- `Created`：runtime key 建立，actor 未完成初始化。
- `Running`：正在处理事件或 handler 执行中。
- `Idle`：阻塞在 `waitForEvent`。
- `ShuttingDown`：收到 `Shutdown`，执行收尾。
- `Stopped`：线程退出，资源释放完成。
- `Error`：初始化或执行失败，等待恢复 / 重建。

## 隔离与资源约束

当前隔离模型不是“多进程硬隔离”，而是**单进程内的受控 VM 隔离**。风险控制依赖以下几层：

| 层 | 机制 | 说人话 |
|----|------|--------|
| 线程隔离 | `VmActor` 把长跑 VM 放到专属线程 | 一个插件卡死时，不直接把主循环一起拖住。 |
| 时间预算 | `call_timeout_ms` | 单次执行跑太久就掐。 |
| 指令预算 | `interrupt_budget` | 死循环不能无限跑。 |
| 堆预算 | `js_heap_mb` | 不让单实例把 JS 堆吃穿。 |
| 错误降级 | `catch_unwind` + `Error` 态 | 真出事时先把当前实例标红关掉，别拖垮别的实例。 |

这套模型的结论是：

- **普通脚本错误、超时、预算耗尽** 应当只让当前插件实例出错；
- **底层 C 级崩溃** 不是这套架构能完全兜住的对象，因此文档与实现都不再假装有“Wasm 级硬墙”。

## Session 维度隔离

一个插件被两个会话同时使用时，当前模型默认不是共享一个 VM，而是：

```text
(session_a, plugin_x) -> vm_a
(session_b, plugin_x) -> vm_b
```

这样做的原因：

- 不把 session 状态混在一个 JS 堆里；
- 事件投递天然按实例路由；
- `session_end` 可以按会话批量回收。

## 配置与环境变量

总则：**env > config > 默认**。

| 项 | 默认值 | `0` 值语义 | 说明 | 说人话 |
|----|--------|------------|------|--------|
| `[plugin] js_heap_mb` | `16` | 不设置 QuickJS 堆上限 | 单实例 JS 堆预算 | 别让某个插件把内存吃穿。 |
| `[plugin] call_timeout_ms` | `30000` | 禁用单次执行软超时 | 单段 JS 执行墙钟时限 | 一次执行跑太久就当异常处理。 |
| `[plugin] interrupt_budget` | `5000000` | 禁用 budget 中断 | 单次执行预算 | 死循环和超大计算不能无限跑。 |
| `[plugin] event_channel_capacity` | `64` | 退化为同步交接 | 宿主向长生命周期 VM 投递事件的队列深度 | 排队能排多深。 |
| `[plugin] idle_ttl_ms` | `300000` | 禁用 idle TTL 回收 | 机会式空闲回收阈值 | 闲太久再关厅，但不是后台每秒巡逻。 |
| `PI_PLUGIN_DISABLE` | false | `1/true/yes/on` = 全局禁用 | 跳过整套插件运行时初始化 | 一把总闸，彻底关停插件系统。 |

> 机会式回收说明：本期**没有**后台 sweeper。`idle_ttl_ms` 只会在新的插件活动发生时顺手检查，或在 `end_session()` 时显式清理；所以 TTL 到了不代表立刻回收。

## 错误模型 / 隔离结局

```text
manifest 解析失败 / main 越界        -> 加载即失败，写审计
JS 运行时构建失败                    -> create_instance 失败
插件初始化脚本抛错                   -> Err + destroy，不留半初始化实例
_start 运行期 panic                  -> catch_unwind -> VmActorState::Error
单次执行超时 / budget 耗尽           -> 中断该次执行 + 必要时重建
hostcall 参数 / 权限错误             -> HostResponse{ok:false,error}
异步任务超时                        -> poll 侧收到 Error
```

> 说人话：发现 / 激活期错误是“直接拒绝并记账”；运行期错误是“就地兜住，尽量别让火烧出当前插件实例”；业务层 hostcall 失败则作为 `ok:false` 回给插件，让插件自己决定怎么处理。

## 可靠性策略

- 队列上限：`DispatchEvent` 使用有界 channel，超过上限时回压或拒绝。
- 超时策略：事件处理超时、hostcall 超时分离，避免互相吞错。
- 清理策略：`session_end` 触发 `Shutdown`，并清理 `RuntimeKey` 下的 pending call。
- 恢复策略：`Error` 态支持按需重建 actor（重建前记录诊断事件）。

## 代码入口

实现侧代码地图见 [`../../src/ext/README.md`](../../src/ext/README.md)。如果要从代码下钻，推荐顺序是：

1. `src/ext/runtime/engine_config.rs`
2. `src/ext/runtime/engine.rs`
3. `src/ext/runtime/instance.rs`
4. `src/ext/runtime_manager.rs`
5. `src/ext/vm_actor.rs`
6. `src/ext/plugin/manager.rs`

## 与其他文档的关系

- 发现、scope 与激活时机：[`plugin-source-scan-register-load.md`](./plugin-source-scan-register-load.md)
- JS bridge 与宿主 API 边界：[`js-bridge-and-host-api.md`](./js-bridge-and-host-api.md)
- Hostcall / manifest / `tools[]` / `functions[]` 契约：[`host-call-protocol.md`](./host-call-protocol.md)
- 事件语义：[`events.md`](./events.md)
