本文为 [Architecture](../../Architecture.md) 中「4. 插件系统（统一入口）」的补充设计，聚焦“插件来源扫描 -> 注册 -> 加载 -> 运行时管理”闭环。

---

## 插件来源扫描注册加载技术方案

本文目标：

1. 对照 `openclaw` 与 `pi-mono` 的来源扫描/注册/加载实践。
2. 给出 `pi-rust-wasm` 的备选架构与推荐定版方案。
3. 提供可执行的模块边界、状态模型、测试矩阵与演进路径。

> 本文为架构优先设计，不以当前实现细节为前提；现有代码仅作为迁移输入，不作为架构约束。

### 实现状态（待实现）

- 本文所有架构与流程目前为目标设计，默认状态为**待实现**。
- 若后续落地，请按章节更新为：`已实现` / `部分实现` / `待实现` 并附实现文件链接。

---

## 术语表

| 术语 | 含义 |
|------|------|
| Source Scan | 扫描候选插件来源（目录/配置/内置/宿主注入）。 |
| Manifest | 插件声明文件（ID、版本、配置 schema、能力声明等）。 |
| Catalog | 可发现插件清单（元数据层，不含运行时实例）。 |
| Registry | 运行时注册表（已激活实例、状态、句柄）。 |
| Activation | 从 Catalog 命中后按需加载到 Registry。 |
| Enabled State | 策略层状态（是否允许启用）。 |
| Load State | 运行时状态（是否加载、是否失败）。 |

---

## 一、对标设计：openclaw

### 1.1 设计摘要

`openclaw` 的主线是“两阶段”：

- 阶段 1（发现/校验）：`discover + manifest-registry`
- 阶段 2（激活/注册）：`loader + plugin-registry`

核心特点：

- 多来源扫描（`bundled/workspace/global/config`）
- Manifest 先行（先校验清单，再加载代码）
- 诊断内建（重复 ID、路径逃逸、权限可疑、schema 缺失）
- 运行时注册内容丰富（tools/hooks/commands/providers/channels/httpRoutes/...）

### 1.2 ASCII 核心四图（openclaw）

#### 1) 结构图

```text
┌──────────────────────────────────────────────────────────────────────┐
│                        OpenClaw Plugin Pipeline                     │
├──────────────────────────────────────────────────────────────────────┤
│ Source Discovery                                                     │
│   bundled / workspace / global / config                             │
│         │                                                            │
│         ▼                                                            │
│ Manifest Registry                                                    │
│   id, origin, schema, diagnostics, precedence                        │
│         │                                                            │
│         ▼                                                            │
│ Loader                                                               │
│   allow/deny/entries/slota -> resolve enabled set                    │
│         │                                                            │
│         ▼                                                            │
│ Runtime Plugin Registry                                              │
│   plugins/tools/hooks/commands/providers/channels/httpRoutes/...     │
└──────────────────────────────────────────────────────────────────────┘
```

#### 2) 调用流图

```text
discoverOpenClawPlugins()
    -> loadPluginManifestRegistry()
        -> resolve duplicate/precedence/security diagnostics
            -> createPluginRegistry()
                -> load module (jiti)
                    -> plugin register(api)
                        -> registerTool/registerHook/registerCommand/...
                            -> setActivePluginRegistry()
```

#### 3) 时序图

```text
Startup
  │
  ├─ scan sources ---------------------------> candidates
  ├─ read manifests -------------------------> manifest records + diagnostics
  ├─ apply config policy --------------------> enabled/disabled decision
  ├─ load selected modules ------------------> plugin register(api)
  └─ commit active registry -----------------> runtime available
```

#### 4) 数据闭环图

```text
manifest + source metadata
        │
        ▼
catalog decision (enabled?)
        │ yes
        ▼
runtime registration (tools/hooks/commands/...)
        │
        ▼
agent/runtime invocation
        │
        └─ diagnostics/events/metrics -> feedback to operations
```

---

## 二、对标设计：pi-mono

### 2.1 设计摘要

`pi-mono` 的核心是“扩展运行时注册”：

- 发现侧：通过 `package.json` 的 `pi` manifest（如 `pi.extensions`）与目录约定收集资源。
- 加载侧：extension module 加载后，将声明写入内存结构。
- 运行侧：runner 按事件/工具/命令三类统一调度。

三个关键注册面：

- `handlers`：事件回调（on）
- `tools`：LLM/运行时可调用工具（registerTool）
- `commands`：命令入口（registerCommand）

### 2.2 ASCII 核心四图（pi-mono）

#### 1) 结构图

```text
┌────────────────────────────────────────────────────────────┐
│                    pi-mono Extension System                │
├────────────────────────────────────────────────────────────┤
│ Resource Discovery                                          │
│   package.json(pi manifest) + conventions + filters         │
│         │                                                    │
│         ▼                                                    │
│ Extension Loader                                             │
│   load module -> createExtensionAPI                          │
│         │                                                    │
│         ▼                                                    │
│ In-Memory Extension Store                                    │
│   handlers Map / tools Map / commands Map                    │
│         │                                                    │
│         ▼                                                    │
│ Extension Runner                                             │
│   dispatch events / resolve tools / resolve commands         │
└────────────────────────────────────────────────────────────┘
```

#### 2) 调用流图

```text
discover extension paths
    -> loadExtensionModule(path)
        -> factory(api)
            -> api.on(...)
            -> api.registerTool(...)
            -> api.registerCommand(...)
                -> store in extension maps
                    -> runner dispatch at runtime
```

#### 3) 时序图

```text
Session/Startup
  │
  ├─ collect resources from manifest/filter
  ├─ load extension modules
  ├─ execute factory registration
  ├─ build effective tool/command set
  └─ runtime emits events -> handlers execute
```

#### 4) 数据闭环图

```text
manifest/resource config
      │
      ▼
extension load
      │
      ▼
handlers/tools/commands maps
      │
      ├─ event path ----> handlers
      ├─ tool path -----> tools
      └─ command path --> commands
```

---

## 三、本项目备选设计（pi-rust-wasm）

> 实现状态：**待实现**

### 3.1 方案 A：目录直载（最小改动）

- 通过路径直接 `load_plugin(path)`，立即实例化并执行初始化。
- 优点：实现简单，改动小。
- 缺点：缺乏 catalog 层、策略层弱、难支持多 agent 独立策略。

### 3.2 方案 B：GlobalCatalog + AgentRegistry（推荐）

- GlobalCatalog：聚合可发现插件（全局/agent/宿主注入），统一清单与诊断。
- AgentRegistry：按 agent 按需激活，维护运行时实例与状态。
- Session 仅携带上下文，不复制插件实例。

### 3.3 方案 C：进程级单 Registry

- 类似“全局唯一 active registry”。
- 优点：模型简单。
- 缺点：多 agent 隔离弱，状态污染风险高，不利于后续精细化调度。

### 3.4 选择建议

推荐方案 B。理由：

1. 保留全局可发现能力，同时保证 agent 执行隔离。
2. 最适配后续多 agent 并发与策略差异化。
3. 兼顾懒加载效率与状态可观测性。

---

## 四、推荐定版：GlobalCatalog + AgentRegistry + SessionContext

> 实现状态：**待实现（推荐目标架构）**

### 4.1 分层定义

- `GlobalCatalog`（可发现层）  
  保存插件元数据与来源信息，不保存 VM 实例。
- `AgentRegistry`（执行层）  
  每个 agent 一张已激活实例表，保存 VM/绑定状态/句柄。
- `SessionContext`（调用层）  
  只提供 `session_id`、权限与上下文，不承载插件实例。

### 4.2 状态模型

每个插件条目最少包含：

- `enabled_state`: `enabled | disabled`
- `load_state`: `unloaded | loading | loaded | error`
- `source_origin`: `global | agent | host_injected`
- `diagnostics[]`: 扫描/校验/加载诊断

### 4.3 生命周期约束

- 懒加载：首次命中才激活实例。
- 常驻：本期不自动卸载。
- 卸载接口：预留 API 与状态机转移（后续实现 GC/hot-reload）。

### 4.4 ASCII 核心四图（本项目推荐架构）

#### 1) 结构图

```text
┌──────────────────────────────────────────────────────────────────────┐
│                        Plugin Runtime Architecture                  │
├──────────────────────────────────────────────────────────────────────┤
│ GlobalCatalog                                                        │
│   scan(global dir, agent dir, host injected)                         │
│   validate manifest/schema/security                                  │
│   build discoverable records + diagnostics                           │
├──────────────────────────────────────────────────────────────────────┤
│ AgentRegistry (per agent)                                            │
│   activated plugin instances                                          │
│   vm handle / tool bindings / event handlers / status                │
├──────────────────────────────────────────────────────────────────────┤
│ SessionContext (per session call)                                    │
│   session_id / permissions / runtime context                          │
└──────────────────────────────────────────────────────────────────────┘
```

#### 2) 调用流图

```text
runtime needs plugin X
    -> query GlobalCatalog(X)
        -> check enabled_state + policy
            -> if unloaded: activate into AgentRegistry
                -> bind tools/handlers/commands
                    -> execute with SessionContext
```

#### 3) 时序图

```text
Process Start
  │
  ├─ build GlobalCatalog (scan + validate + diagnostics)
  │
Agent First Use(plugin X)
  │
  ├─ AgentRegistry miss
  ├─ activate plugin X from GlobalCatalog
  └─ AgentRegistry hit (subsequent calls)
```

#### 4) 数据闭环图

```text
Source Scan -> Catalog Record -> Agent Activation -> Runtime Invocation
      │               │                  │                   │
      └---------------┴------------------┴-------------------┘
                      diagnostics/metrics/state feedback
```

---

## 五、模块边界（目标设计）

建议目标模块：

- `plugin_source_scanner`：来源扫描（global/agent/host injected）
- `plugin_manifest_registry`：manifest 解析、校验、去重、优先级
- `plugin_catalog`：可发现插件集合查询接口
- `agent_plugin_registry`：agent 运行时实例表与状态机
- `plugin_activator`：按需激活流程（load/bind/register）
- `plugin_lifecycle_service`：启用/禁用/预留卸载与观测

### 模块落地清单（待实现）

- [ ] `plugin_source_scanner`：完成来源扫描（全局/agent/宿主注入）
- [ ] `plugin_manifest_registry`：完成 manifest/schema 校验、去重与优先级
- [ ] `plugin_catalog`：完成可发现插件查询接口
- [ ] `agent_plugin_registry`：完成 per-agent 运行时表与状态机
- [ ] `plugin_activator`：完成按需激活与绑定流程
- [ ] `plugin_lifecycle_service`：完成启用/禁用与卸载接口占位

注意：

- 插件注册表与工具系统分层。
- 4 原语/LLM 属于核心能力，不是插件条目。
- 插件“贡献工具”才进入插件运行时注册域。

---

## 六、测试矩阵与演进路线

> 实现状态：**待实现**

### 6.1 测试矩阵

1. 来源扫描：
   - 全局目录、agent 目录、宿主注入来源识别
   - 非法路径、重复 ID、manifest/schema 错误诊断
2. 激活流程：
   - 懒加载首次激活
   - 二次命中复用（不重复初始化）
3. 隔离性：
   - 不同 agent 相同插件独立实例
   - 同 agent 多会话共享实例但上下文隔离
4. 状态机：
   - `enabled_state`/`load_state` 的转移正确性
   - `error` 态恢复路径

### 6.2 演进路线

- Phase 1：Catalog + AgentRegistry + 懒加载常驻
- Phase 2：显式卸载 + 生命周期回收（idle TTL/手动卸载）
- Phase 3：热更新与版本并行（灰度、回滚）

### 测试任务清单（待实现）

- [ ] 来源扫描测试（来源识别、路径安全、重复 ID）
- [ ] 激活流程测试（懒加载首命中、二次命中复用）
- [ ] 隔离性测试（跨 agent 隔离、同 agent 多会话上下文隔离）
- [ ] 状态机测试（`enabled_state` / `load_state` 转移、error 恢复）

---

## 与其他文档关系

| 主题 | 文档 |
|------|------|
| 插件系统总览 | [插件系统全貌](../plugin-system-overview.md) |
| Host API 边界 | [宿主API层](host-api-layer.md) |
| Hostcall 协议 | [Hostcall JSON 协议](host-call-protocol.md) |
| 异步执行模型 | [异步 Hostcall 与事件循环](async-hostcall-event-loop.md) |
| 长生命周期 VM | [Phase 2 长生命周期 VM](phase2-long-lived-vm.md) |

---

**导航**：返回 [插件系统全貌](../plugin-system-overview.md) | 相关： [Architecture](../../Architecture.md)

