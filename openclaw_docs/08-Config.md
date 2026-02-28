# Config 配置模块

**设计思想**：OpenClaw 采用单一配置文件 `clawdbot.json` 承载全部运行时配置，通过加载、校验、合并、路径解析与热更机制，为 Gateway、Channels、Agents 等模块提供统一的配置来源。配置模块不持有运行时状态，仅负责 I/O 与转换。

---

## 一、职责与设计目标

- **单一真相源**：所有配置从 `~/.clawdbot/clawdbot.json`（或 `CONFIG_PATH_CLAWDBOT` 指定路径）加载。
- **校验与合并**：加载后经 `validateConfigObjectWithPlugins` 校验，再应用 defaults、merge-config、runtime-overrides。
- **路径规范化**：`normalize-paths` 将相对路径解析为绝对路径；`paths` 提供 `resolveConfigPath`、`resolveStateDir` 等。
- **与 Gateway 热更配合**：`config-reload` 监听文件变化，触发 Gateway 重载配置，Config 模块本身无状态，每次 `loadConfig` 返回最新快照。

---

## 二、ClawdbotConfig 结构

**定义位置**：`openclaw/src/config/types.clawdbot.ts`

**顶层字段**（节选）：

| 字段 | 类型 | 说明 |
|------|------|------|
| meta | object | lastTouchedVersion、lastTouchedAt |
| auth | AuthConfig | 鉴权配置 |
| env | object | shellEnv、vars、环境变量注入 |
| wizard | object | 上次 wizard 运行信息 |
| agents | AgentsConfig | Agent 列表、defaults、workspace |
| models | ModelsConfig | 模型、别名、fallback、auth profiles |
| channels | ChannelsConfig | 各 channel 配置（whatsapp、telegram 等） |
| session | SessionConfig | mainKey、scope、groupPolicy |
| gateway | GatewayConfig | bind、port、auth、tailscale、controlUi |
| plugins | PluginsConfig | 启用/禁用的插件 |
| skills | SkillsConfig | 技能目录、ClawdHub |
| tools | ToolsConfig | browser、media、sandbox 等 |
| cron | CronConfig | 定时任务 |
| hooks | HooksConfig | 钩子启用状态 |
| bindings | AgentBinding[] | Agent 路由绑定 |

**子类型**：`types.agents`、`types.gateway`、`types.channels`、`types.models` 等定义各区块的详细结构。

---

## 三、加载与写入

### 3.1 入口

- **loadConfig**：`openclaw/src/config/io.ts`，主入口，返回 `ClawdbotConfig`。
- **readConfigFileSnapshot**：读取文件并返回 `ConfigFileSnapshot`（raw、parsed、valid、issues、warnings、legacyIssues）。
- **writeConfigFile**：写入配置，支持备份轮转（`CONFIG_BACKUP_COUNT`）。

### 3.2 加载流程

```
resolveConfigPath() → 确定配置文件路径
  → fs.readFile (或 readConfigFileSnapshot)
  → parseConfigJson5 (JSON5 解析)
  → resolveConfigIncludes (处理 $include)
  → resolveConfigEnvVars (环境变量替换)
  → coerceConfig → ClawdbotConfig
  → validateConfigObjectWithPlugins
  → applyDefaults (defaults.ts 中各 apply*)
  → mergeConfig (若有 include 合并)
  → applyConfigOverrides (runtime-overrides)
  → normalizeConfigPaths
  → 返回
```

### 3.3 校验

- **validateConfigObjectWithPlugins**：`openclaw/src/config/validation.ts`，基于 schema 与插件提供的 schema 扩展校验。
- **findLegacyConfigIssues**：检测废弃字段，提示迁移。

---

## 四、路径与运行时覆盖

- **paths.ts**：`resolveConfigPath`、`resolveStateDir`、`resolveAgentDir` 等。
- **runtime-overrides.ts**：`applyConfigOverrides`，支持 `CLAWDBOT_*` 环境变量覆盖部分配置。
- **normalize-paths.ts**：将配置中的相对路径（如 workspace、agent dir）解析为绝对路径。

---

## 五、与 Gateway config-reload 的配合

- Gateway 启动时调用 `startGatewayConfigReloader`（`openclaw/src/gateway/config-reload.ts`）。
- 监听配置文件变化（chokidar），变化时重新 `loadConfig`，并触发 Gateway 内部重载（channel-manager、cron、plugins 等）。
- Config 模块无缓存，每次 `loadConfig` 都从磁盘读取并重新解析。

---

## 六、关键文件索引

| 文件 | 职责 |
|------|------|
| openclaw/src/config/io.ts | loadConfig、readConfigFileSnapshot、writeConfigFile |
| openclaw/src/config/types.clawdbot.ts | ClawdbotConfig 类型定义 |
| openclaw/src/config/validation.ts | 校验 |
| openclaw/src/config/defaults.ts | 各区块默认值 |
| openclaw/src/config/merge-config.ts | include 合并 |
| openclaw/src/config/paths.ts | 路径解析 |
| openclaw/src/config/runtime-overrides.ts | 运行时覆盖 |
| openclaw/src/config/plugin-auto-enable.ts | 插件自动启用 |
| openclaw/src/config/legacy-migrate.ts | 遗留配置迁移 |
