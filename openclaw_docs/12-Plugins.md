# Plugins

## 零、先用大白话

插件像 **可插拔插座**。  
核心只认 **插头形状**：`openclaw.plugin.json` + **Plugin SDK 里公开的那几样**。  
具体渠道、工具、钩子，以 **独立包** 拧上去。  
**别**从扩展里乱 `import` 核心私有文件——边界写在上游 **`AGENTS.md`**。

**这一节你会学到**：`discoverOpenClawPlugins` 从哪扫；registry 给谁用。

---

## ASCII 核心四图

### 1) 结构图

```text
┌──────────────────────────────────────────────────────────────┐
│              OpenClaw Plugin Pipeline（缩写）                 │
├──────────────────────────────────────────────────────────────┤
│ Source Discovery：bundled / workspace / global / config       │
│        -> Manifest Registry（openclaw.plugin.json 聚合）      │
│        -> Loader（allow/deny/slots -> enabled set）           │
│        -> Runtime Plugin Registry（tools/hooks/channels/...） │
└──────────────────────────────────────────────────────────────┘
```

### 2) 调用流图

```text
discoverOpenClawPlugins()
  -> loadPluginManifestRegistry()
      -> diagnostics / precedence
          -> createPluginRegistry()
              -> jiti load -> plugin.register(api)
                  -> registerTool/registerHook/registerCommand/...
                      -> setActivePluginRegistry()
```

### 3) 时序图

```text
Startup
  |
  ├─ scan sources ------------------------> candidates
  ├─ read manifests ----------------------> records + diagnostics
  ├─ apply config policy -----------------> enabled/disabled
  ├─ load modules ------------------------> register(api)
  └─ commit registry ---------------------> runtime ready
```

### 4) 数据闭环图

```text
manifest + source metadata
        |
        v
enabled? -> load -> runtime surfaces（tools/hooks/...）
        |
        v
agent / gateway 调用插件能力
        |
        └─ diagnostics / logs -> 运维调整配置与清单
```

---

## 一、入口与加载

**设计思想**：manifest 优先；loader 负责发现、校验、注册与运行时衔接。

- **loader.ts**：`src/plugins/loader.ts`，包含 `loadPlugins`、`createPluginRegistry` 等；发现函数为 **`discoverOpenClawPlugins`**（`src/plugins/discovery.ts`）。
- **discovery**：扫描 workspace 与安装路径，读取插件清单 **`openclaw.plugin.json`**（bundled 扩展与第三方插件均以此声明 id、入口、channel 等）。
- **registry**：`PluginRecord`、`PluginRegistry`，供 Gateway 与渠道装配使用。

---

## 二、Slots 与 Config

- **config-state**：`normalizePluginsConfig`、`resolveEffectiveEnableState`、`resolveMemorySlotDecision` 等。
- **slots**：插件槽位与扩展点（与 memory、gateway 等协作）。

---

## 三、Plugin SDK

- **`src/plugin-sdk/`**：扩展可依赖的 **公开** API 与类型（对外文档见上游 `docs/plugins/`）。
- **extensions**：各渠道/能力以 workspace 包形式放在 `extensions/*`，通过 manifest 注册。

---

## 延伸阅读

- [03-Channels.md](03-Channels.md)  
- [README.md](README.md)（`openclaw.plugin.json` 命名约定）

---

## 常见误会

- **误会**：禁用插件只要删目录。**正解**：配置里 **`plugins`** 状态、缓存、manifest 诊断都要看；删目录不等于干净。  
- **误会**：插件和内置代码权限一样。**正解**：插件跑在 **受控 API** 后；越权会加载失败或被拒。  
- **误会**：改 `extensions/` 立刻影响运行中 Gateway。**正解**：看 **reload 计划**；有的要重启。
