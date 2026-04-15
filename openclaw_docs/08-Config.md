# Config 配置模块

## 零、先用大白话

配置模块像 **图书馆员管规则手册**。  
它不聊天。它只做几件事：把 **`openclaw.json`** 从磁盘读出来、查有没有写错、补上默认值、把路径变成「绝对地址」，再递给 Gateway 和渠道。  
手册改了几个字，Gateway 常常能 **热更新**，不用整台电脑重启。

**这一节你会学到**：配置从哪读；类型散在哪几个文件；环境变量能盖什么。

---

## ASCII 核心四图

### 1) 结构图

```text
paths.ts（~/.openclaw/、openclaw.json）
        |
        v
loadConfig + validate + merge defaults
        |
        v
normalize-paths（绝对化）
        |
        v
Gateway / Channels / Agents 消费 OpenClawConfig
```

### 2) 调用流图

```text
读磁盘 JSON
  -> validateConfigObjectWithPlugins
      -> merge runtime overrides
          -> normalize paths
              -> 返回内存快照给调用方
```

### 3) 时序图

```text
CLI          loadConfig         plugins schema     Disk
  |                |                  |             |
  | doctor/config  |                  |             |
  |--------------->|----------------->|------------>|
  |                | validated        |             |
  |<---------------|                  |             |
```

### 4) 数据闭环图

```text
用户编辑 openclaw.json
        |
        v
config-reload 监听 -> 新快照
        |
        v
WS 客户端重新拉 config.get
        |
        v
doctor 校验通过 -> 长期运行稳定
```

---

## 一、职责与设计目标

**设计思想**：采用单一配置文件（默认 **`openclaw.json`**）承载运行时配置；模块无长期状态，只做 I/O 与转换。

- **单一真相源**：默认路径由 **`src/config/paths.ts`** 解析：状态目录默认 **`~/.openclaw/`**，主配置默认 **`~/.openclaw/openclaw.json`**。可用 **`OPENCLAW_CONFIG_PATH`** 或 **`OPENCLAW_STATE_DIR`** 覆盖；仍可能识别旧目录 **`~/.clawdbot`** 与旧文件名 **`clawdbot.json`**（兼容逻辑见 `paths.ts`）。
- **校验与合并**：`validateConfigObjectWithPlugins`；defaults、`merge-config`、内存中的 **runtime overrides**（见下）。
- **路径规范化**：`normalize-paths`；`paths` 提供 `resolveConfigPath`、`resolveStateDir` 等。
- **与 Gateway 热更配合**：`config-reload` 监听文件变化；每次 `loadConfig` 从磁盘读新快照。

---

## 二、OpenClawConfig 结构

**定义位置**：类型从 **`src/config/types.ts`** 再导出；根对象形状主要在 **`src/config/types.openclaw.ts`** 及 `types.*.ts` 各子模块。

**顶层字段**（节选）：

| 字段 | 类型 | 说明 |
|------|------|------|
| meta | object | lastTouchedVersion、lastTouchedAt |
| auth | AuthConfig | 鉴权配置 |
| env | object | shellEnv、vars、环境变量注入 |
| wizard | object | 上次 wizard 运行信息 |
| agents | AgentsConfig | Agent 列表、defaults、workspace |
| models | ModelsConfig | 模型、别名、fallback、auth profiles |
| channels | ChannelsConfig | 各 channel 配置 |
| session | SessionConfig | mainKey、scope、groupPolicy |
| gateway | GatewayConfig | bind、port、auth、tailscale、controlUi |
| plugins | PluginsConfig | 启用/禁用的插件 |
| skills | SkillsConfig | 技能目录、ClawdHub |
| tools | ToolsConfig | browser、media、sandbox 等 |
| cron | CronConfig | 定时任务 |
| hooks | HooksConfig | 钩子启用状态 |
| bindings | AgentBinding[] | Agent 路由绑定 |

---

## 三、加载与写入

### 3.1 入口

- **loadConfig**：`src/config/io.ts`，主入口，返回 **`OpenClawConfig`**。
- **readConfigFileSnapshot**：`ConfigFileSnapshot`（raw、parsed、valid、issues、warnings、legacyIssues）。
- **writeConfigFile**：写入配置，支持备份轮转（`CONFIG_BACKUP_COUNT`）。

### 3.2 加载流程（简化）

```text
resolveConfigPath() → 读文件
  → parseConfigJson5
  → resolveConfigIncludes（$include）
  → resolveConfigEnvVars
  → coerceConfig → OpenClawConfig
  → validateConfigObjectWithPlugins
  → applyDefaults
  → mergeConfig（若有 include）
  → applyConfigOverrides（内存中的测试/注入覆盖，见 runtime-overrides）
  → normalizeConfigPaths
  → 返回
```

### 3.3 校验

- **validateConfigObjectWithPlugins**：`src/config/validation.ts`。
- **findLegacyConfigIssues**：废弃字段与迁移提示。

---

## 四、路径与运行时覆盖

- **paths.ts**：`resolveConfigPath`、`resolveStateDir`、`resolveAgentDir`；网关端口还可看环境变量 **`OPENCLAW_GATEWAY_PORT`**（见 `resolveGatewayPort`）。
- **runtime-overrides.ts**：测试或运行时通过 **`setConfigOverride`** 写入内存树，**`applyConfigOverrides`** 在 `loadConfig` 末尾合并；**不是**「`CLAWDBOT_*` 环境变量自动映射整棵配置树」的旧描述。
- **normalize-paths.ts**：workspace、agent dir 等相对路径 → 绝对路径。

---

## 五、与 Gateway config-reload 的配合

- **`src/gateway/config-reload.ts`**：`startGatewayConfigReloader` 监听配置文件变化，重新 `loadConfig` 并驱动 Gateway 内部重载（channel-manager、cron、plugins 等）。

---

## 六、关键文件索引

| 文件 | 职责 |
|------|------|
| `src/config/io.ts` | loadConfig、readConfigFileSnapshot、writeConfigFile |
| `src/config/types.ts` | 类型再导出入口 |
| `src/config/types.openclaw.ts` | 根配置类型主模块 |
| `src/config/validation.ts` | 校验 |
| `src/config/defaults.ts` | 默认值 |
| `src/config/merge-config.ts` | include 合并 |
| `src/config/paths.ts` | 路径解析、旧目录兼容 |
| `src/config/runtime-overrides.ts` | 内存覆盖合并 |
| `src/config/plugin-auto-enable.ts` | 插件自动启用 |
| `src/config/legacy-migrate.ts` | 遗留迁移 |

---

## 延伸阅读

- [01-技术设计总览.md](01-技术设计总览.md)  
- [README.md](README.md)（历史名称与 `openclaw doctor`）

---

## 常见误会

- **误会**：我改了环境变量就等于改了整份配置。**正解**：只有少数键会进 **`runtime-overrides`**；大头仍是 **`openclaw.json`**。  
- **误会**：JSON 里多一个未知字段没关系。**正解**：校验会 **报错或警告**；看 `doctor` 输出最省事。  
- **误会**：`merge-config` 和 `$include` 是一回事。**正解**：`$include` 是 **拆文件**；merge 是 **合并规则**；都在 `loadConfig` 链路里分步发生。
