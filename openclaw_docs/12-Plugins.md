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
- **npm 包**：工作区内与 `src/plugin-sdk` 对齐的发布边界为 **`@openclaw/plugin-sdk`**（`packages/plugin-sdk/package.json` 的 **`exports` 子路径**）；类型与实现以 **OpenClaw 上游仓库**为准。

### `@openclaw/plugin-sdk` 子路径：类型 / 模块 / 描述

下表按 **类别** 归纳各导出子路径的职责；说明依据上游 `main` 分支 `src/plugin-sdk/<name>.ts` 的注释与导出（若上游改名或增减子路径，以对仓库为准）。

| 类别 | 模块（import 子路径） | 描述 |
|------|-------------------------|------|
| 标识 | `@openclaw/plugin-sdk/account-id` | 账户 ID 归一化（路由侧 `normalizeAccountId` 等）。 |
| ACP | `@openclaw/plugin-sdk/acp-runtime` | ACP 控制面/会话运行期：`getAcpSessionManager`、`AcpRuntime*` 类型与后端注册。 |
| 插件入口 | `@openclaw/plugin-sdk/plugin-entry` | **`OpenClawPluginApi`** 等插件/API **类型**、`definePluginEntry`、配置 schema（`buildPluginConfigSchema`）聚合导出。 |
| 插件运行期 | `@openclaw/plugin-sdk/plugin-runtime` | 共享 **命令 / 钩子 / HTTP 路径与注册表 / 交互绑定 / 懒加载服务模块**；导出 `PluginRuntime`、`RuntimeLogger` 等。 |
| Provider 样板 | `@openclaw/plugin-sdk/provider-entry` | **单 Provider（常见为 API Key）** 插件样板：`SingleProviderPlugin*`、`buildSingleProviderApiKeyCatalog`，依赖 `definePluginEntry`。 |
| Provider 鉴权（配置侧） | `@openclaw/plugin-sdk/provider-auth` | Auth profile store、CLI/OAuth 凭据读取、API key marker、profile upsert 等（偏配置/onboarding）。 |
| Provider 鉴权（执行侧） | `@openclaw/plugin-sdk/provider-auth-runtime` | 运行时 API key 轮换、`requireApiKey`、`getRuntimeAuthForModel` 等动态装载。 |
| Provider 环境变量 | `@openclaw/plugin-sdk/provider-env-vars` | Provider 鉴权相关的 env 列举与过滤（`getProviderEnvVars`、`omitEnvKeysCaseInsensitive`）。 |
| Provider HTTP | `@openclaw/plugin-sdk/provider-http` | Provider 侧通用 HTTP：`fetchWithTimeout`、`postJsonRequest`、超时与请求能力类型。 |
| Provider 模型类型 | `@openclaw/plugin-sdk/provider-model-types` | 模型配置类型再导出（`ModelApi`、`ModelDefinitionConfig`、`BedrockDiscoveryConfig` 等）。 |
| Provider 模型辅助 | `@openclaw/plugin-sdk/provider-model-shared` | Catalog/replay 策略、模型 ID 归一等 provider 共用逻辑（减轻循环依赖）。 |
| Provider 引导 | `@openclaw/plugin-sdk/provider-onboard` | Onboarding 用轻量 helper（默认 Provider、fallback、静态 allowlist 模型键等）。 |
| Provider 流式 | `@openclaw/plugin-sdk/provider-stream-shared` | 基于 Pi `streamSimple` 的 **流包装器组合**、`tool_stream` 额外参数合成。 |
| Provider 工具 schema | `@openclaw/plugin-sdk/provider-tools` | 工具 schema 兼容性改写（如 Gemini 不支持关键字剔除、XAI profile）。 |
| Provider 用量 | `@openclaw/plugin-sdk/provider-usage` | 各供应商用量抓取（Claude/Codex/Gemini/Minimax/Z.ai）与快照类型。 |
| Provider 搜索 | `@openclaw/plugin-sdk/provider-web-search` | Web Search Provider 注册辅助、缓存键与过滤器 helper。 |
| Provider 搜索（契约） | `@openclaw/plugin-sdk/provider-web-search-contract` | **契约安全**的 web search 注册 + 配置启用（`enablePluginInConfig`）。 |
| Provider 搜索（仅配置契约） | `@openclaw/plugin-sdk/provider-web-search-config-contract` | 仅凭证/合并配置契约，不参与插件 enable 全套接线。 |
| Provider 视频 | `@openclaw/plugin-sdk/video-generation` | 视频生成 Provider 的请求/结果/资产配置类型门面。 |
| 渠道 | `@openclaw/plugin-sdk/core` | **Channel 插件 SDK**：`ChannelPlugin`、出站/配对/安全适配器、`createChannelPluginBase` 等大 surface。 |
| 渠道密钥 UI | `@openclaw/plugin-sdk/channel-secret-runtime` | 渠道密钥与 Secret 输入在运行期的收集、归一与默认值。 |
| 渠道流式配置 | `@openclaw/plugin-sdk/channel-streaming` | 渠道侧 **streaming / chunk / blockStreaming** 等配置类型与兼容迁移辅助。 |
| 浏览器配置 | `@openclaw/plugin-sdk/browser-config-runtime` | 浏览器相关配置快照、`OpenClawConfig`、`normalizePluginsConfig`、端口默认值等 **较轻**边界。 |
| 浏览器节点 / Gateway | `@openclaw/plugin-sdk/browser-node-runtime` | **`callGatewayFromCli`**、Gateway RPC、懒加载插件服务、`runExec`、节点命令白名单等。 |
| 浏览器工具 | `@openclaw/plugin-sdk/browser-setup-tools` | 自动化/浏览器工具共用：`callGatewayTool`、节点列表、媒体读写、CLI 装饰与测试桩。 |
| 浏览器安全 | `@openclaw/plugin-sdk/browser-security-runtime` | SSRF 策略、安全文件访问、代理检测、随机 token、敏感日志脱敏等 **窄边界**安全工具。 |
| 配置运行时 | `@openclaw/plugin-sdk/config-runtime` | 配置读写、运行时快照、频道模型覆盖、群策略、上下文可见性等 **完整配置边界**。 |
| 运行时通用 | `@openclaw/plugin-sdk/runtime-env` | 默认 runtime、`danger`/`info` 日志、verbose、退避与 `sleep` 等进程级辅助。 |
| 诊断 | `@openclaw/plugin-sdk/runtime-doctor` | 危险命名匹配、遗留 streaming 别名、插件安装路径问题、从配置卸载插件等 **doctor** 辅助。 |
| 安全聚合 | `@openclaw/plugin-sdk/security-runtime` | 密钥收集、DM 策略、上下文可见性、`external-content` 等 **安全策略**聚合导出。 |
| Secret 引用 | `@openclaw/plugin-sdk/secret-ref-runtime` | `SecretRef` / `coerceSecretRef` 窄导出（配置契约路径）。 |
| Secret 输入 schema | `@openclaw/plugin-sdk/secret-input` | 基于 **zod** 构建 `SecretInput` schema、字符串归一解析。 |
| SSRF | `@openclaw/plugin-sdk/ssrf-runtime` | **pinned dispatcher**、`fetchWithSsrFGuard`、私网访问策略迁移与校验。 |
| CLI 终端 | `@openclaw/plugin-sdk/cli-runtime` | CLI 共用：时长解析、`wait`、`version`、提示样式等。 |
| 错误 | `@openclaw/plugin-sdk/error-runtime` | 请求域子 Agent 错误类、错误图提取与展示格式化。 |
| 文本 / Markdown | `@openclaw/plugin-sdk/text-runtime` | Markdown IR、表格、渲染分块、日志脱敏、终端安全文本等大集合 **文本工具**。 |
| 测试 | `@openclaw/plugin-sdk/testing` | 渠道契约测试 helpers、CLI capture、插件 registry mock 等 **窄测试面**。 |
| 依赖再导出 | `@openclaw/plugin-sdk/zod` | 直接再导出 **`zod`**（供插件侧 schema 一致）。 |

**说明**：Memory 宿主另有独立包 **`@openclaw/memory-host-sdk`**（与 `plugin-sdk/memory-core` 类路径协作），不在上表重复列出。

---

## 延伸阅读

- [03-Channels.md](03-Channels.md)  
- [README.md](README.md)（`openclaw.plugin.json` 命名约定）

---

## 常见误会

- **误会**：禁用插件只要删目录。**正解**：配置里 **`plugins`** 状态、缓存、manifest 诊断都要看；删目录不等于干净。  
- **误会**：插件和内置代码权限一样。**正解**：插件跑在 **受控 API** 后；越权会加载失败或被拒。  
- **误会**：改 `extensions/` 立刻影响运行中 Gateway。**正解**：看 **reload 计划**；有的要重启。
