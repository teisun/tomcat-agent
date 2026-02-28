# Plugins

**设计思想**：插件通过 loader 加载，slots、config-state 管理配置与启用状态；plugin-sdk 提供扩展 API；extensions 注册 channels、skills 等。

---

## 一、入口与加载

- **loader.ts**：`openclaw/src/plugins/loader.ts`，`loadPlugins`、`discoverClawdbotPlugins`、`createPluginRegistry`。
- **discovery**：发现插件清单（clawdbot.plugin.json）。
- **registry**：PluginRecord、PluginRegistry。

---

## 二、Slots 与 Config

- **config-state**：`normalizePluginsConfig`、`resolveEnableState`、`resolveMemorySlotDecision`。
- **slots**：插件槽位与扩展点。

---

## 三、Plugin SDK

- **plugin-sdk/**：`openclaw/src/plugin-sdk/`，扩展 API、类型定义。
- **extensions**：channels、skills、hooks 等注册。
