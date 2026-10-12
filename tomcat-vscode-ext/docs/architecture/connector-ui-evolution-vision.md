# Tomcat 连接器模块：产品与技术演进设计

> 本文记录连接器从「可配置的 MCP Server」向「简单易用的连接器中心」演进的长期方向。当前版本只实现 MCP；CLI/A2A 与插件连接器沿用同一产品模型，但不在本期实现。
>
> 参考：豆包“技能·连接器”界面、Cursor MCP 管理体验，以及 Tomcat 的 v2 渐进式披露设计。

## 1. 产品目标：用户是来用能力，不是来研究配置

一个连接器的核心任务只有一件事：把一个外部能力安全地接入 Tomcat，让用户授权后即可使用。

```text
用户想使用“GitHub / 飞书 / Notion”能力
          │
          ▼
   找到连接器 → 点击“+”
          │
          ▼
   一句话说明权限 → 点击“立即连接”
          │
          ▼
   浏览器 OAuth / CLI 扫码授权
          │
          ▼
   返回 Tomcat → 显示“已连接”与可用能力
```

产品原则：

1. **一键优先**：预置连接器不要求用户填写 URL、命令或 OAuth 参数。
2. **授权即完成**：Add/Connect 的主按钮直接进入授权，不制造“保存后再登录”的第二个任务。
3. **解释清楚再授权**：授权前说明连接器能做什么、会申请哪些权限。
4. **配置与状态分离**：配置来自文件或目录；连接状态、授权状态和能力列表由后端实时返回。
5. **MCP 与 CLI 并列**：它们都是 Connector，但连接方式和授权体验不同。
6. **安全默认值**：不在 UI、日志或普通配置中显示密码、PAT、OAuth secret 或 access token。

## 2. 连接器的统一产品模型

```text
Connector
├── identity      id / name / icon / description / category
├── type          MCP | CLI | A2A | Plugin
├── transport     MCP: stdio | Streamable HTTP
│                 CLI: local executable / managed process
├── auth          none | bearer | OAuth | device-code | QR
├── capabilities  tools / resources / actions
├── scope         Global（默认）| Workspace（显式选择）
└── runtime       installed / connecting / connected / failed / revoked
```

`type` 是“它是什么”；`transport` 是“数据怎么走”；`auth` 是“怎么证明身份”。三者不能混成一个字段：

```text
MCP + stdio + 无认证
MCP + stdio + server 自带 OAuth
MCP + HTTP + 无认证
MCP + HTTP + Bearer
MCP + HTTP + OAuth
CLI + 本地进程 + 二维码授权
```

这使未来 CLI 连接器可以复用列表、详情、授权状态和错误处理，而不必把 MCP 特有字段散落在整个 UI 中。

## 3. 当前版本：Global 默认、Workspace 显式选择的 MCP 基座

### 3.1 列表页

```text
┌──────────────────────────────────────────────────────────────┐
│ Tomcat Settings                         Connectors   [ + ]   │
├────────────────┬─────────────────────────────────────────────┤
│ Models         │ Connected                                   │
│ Sessions       │ ┌─────────────────────────────────────────┐ │
│ Tools          │ │ ● GitHub       [MCP] [Workspace]  ›     │ │
│ Connectors     │ │   Connected · 8 tools · HTTP · OAuth    │ │
│                │ └─────────────────────────────────────────┘ │
│                │ ┌─────────────────────────────────────────┐ │
│                │ │ ● Playwright   [MCP] [Workspace]  ›     │ │
│                │ │   Connected · 21 tools · stdio           │ │
│                │ └─────────────────────────────────────────┘ │
│                │ Needs confirmation / Failed                 │
│                │ ┌─────────────────────────────────────────┐ │
│                │ │ ○ repo-tools   [MCP] [Workspace] [Trust]│ │
│                │ │   Awaiting project trust               │ │
│                │ └─────────────────────────────────────────┘ │
└────────────────┴─────────────────────────────────────────────┘
```

默认添加范围是 **Global**，Workspace 用于显式选择的项目共享配置，未打开项目时不可选。Workspace 来源共用项目根级信任入口；不提供“每次命令/URL 改动重新审批”的保证，Global 不受另一个项目的信任状态阻塞。源码事实源是 `gui/src/settings/ConnectorsSettingsView.tsx::{openAdd,requestProjectTrust}`，路径由后端 `connectorConfigPaths` 提供。

### 3.2 添加流程

```text
点击 + Add Connector
        │
        ├─ Type: MCP（CLI/A2A 预留）
        ├─ Scope: Global（默认）/ Workspace
        ├─ Transport: stdio / HTTP
        └─ HTTP 时才出现 Auth: none / Bearer / OAuth
        │
        ▼
点击 Add
        │
        ├─ 写入所选范围的 mcp.json（UI 展示实际路径）
        ├─ HTTP + OAuth → 立即启动 login_connector
        ├─ 打开浏览器或返回受控授权 URL
        └─ Modal 保持打开，显示授权中 → 连接中 → 已连接/失败
```

失败时必须保留“配置已保存”和“授权失败”两个事实：

```text
配置保存成功，但尚未授权
[重新授权] [编辑配置] [移除]
```

### 3.3 详情页

```text
┌──────────────────────────────────────────────┐
│ GitHub                         [Workspace]  ×  │
│ ● Connected · MCP · HTTP                      │
├──────────────────────────────────────────────┤
│ SOURCE                                        │
│ <project>/.agents/mcp.json（默认目录）       │
│                                              │
│ TRANSPORT                                     │
│ HTTP  https://api.githubcopilot.com/mcp/     │
│                                              │
│ AUTHENTICATION                                │
│ OAuth  ✓ Authorized     [Re-login] [Logout]  │
│                                              │
│ TOOLS                                         │
│ 控制哪些工具可以被 tool_search 发现并调用     │
│ search_repositories                         ● │
│ get_file_contents                           ● │
│ create_issue                                ○ │
├──────────────────────────────────────────────┤
│ [Remove]                         [Reload] [Done]│
└──────────────────────────────────────────────┘
```

Tomcat v2 中具体 MCP schema 不进入 LLM 缓存前缀；工具开关控制的是后端目录中可被 `tool_search`/`tool_call` 使用的集合。

## 4. MCP 与 CLI 的产品差异

### 4.1 MCP Server

```text
MCP 连接器卡片
  +
  ▼
连接说明 / 权限说明
  ▼
HTTP MCP → OAuth 登录页或授权配置框
stdio MCP → 启动本地进程；必要时执行其授权流程
  ▼
已连接 → 显示工具、资源、授权状态
```

MCP 的配置通常是 URL 或命令；MCP 工具清单由 server 的 `tools/list` 返回，真实清单永远以后端运行时结果为准。

### 4.2 CLI Connector

```text
CLI 连接器卡片
  +
  ▼
直接下载已签名的 CLI
  ▼
显示“已安装”，按钮变为“连接”
  ▼
点击连接 → 打开浏览器或显示二维码
  ▼
扫码/授权完成 → token 写入安全存储
  ▼
显示“已绑定账号”和关联能力
```

CLI 与 MCP 共用 `ConnectorView`、状态机和授权状态，但 CLI 需要额外的下载、校验、平台适配和 device-code/QR 流程。本期只留接口和 UI 类型，不实现下载器。

## 5. 技术分层

```text
┌──────────────────────────────────────────────────────────┐
│ Connector Catalog / Curated Registry（未来）             │
│ 预置名称、图标、描述、权限、配置模板、下载校验信息       │
└──────────────────────┬───────────────────────────────────┘
                       ▼
┌──────────────────────────────────────────────────────────┐
│ Connector Service                                        │
│ 配置读写、scope 合并、生命周期、授权任务、状态快照       │
└──────────────┬───────────────────────┬───────────────────┘
               ▼                       ▼
       MCP Manager                 CLI Manager（未来）
       stdio / HTTP                下载 / 进程 / QR
               │                       │
               └──────────┬────────────┘
                          ▼
                  Generic Connector API
                  list / describe / call
                          │
                          ▼
                  tool_search / describe / call
```

### 5.1 配置层

- Workspace：`<project>/<workspace.project_resource_dir>/mcp.json`，项目资源目录默认 `.agents`。
- Global：`get_work_dir(cfg)/mcp.json`；两个路径均以 Settings 展示的实际配置路径为准。
- 同名 Workspace 配置覆盖 Global 配置。
- `mcp.json` 保存声明；运行时连接、工具目录和 OAuth 状态不混写进配置。

### 5.2 授权层

```text
AuthProvider
├── None
├── StaticBearer（来自安全输入，不回显）
├── OAuthAuthorizationCode + PKCE
├── DeviceCode（未来）
└── QrCode（未来 CLI）
```

OAuth 核心组件应拥有清晰边界：

- Discovery：从 MCP 401 challenge 和标准 metadata 找授权地址。
- Callback：`127.0.0.1` 动态端口，只校验 `state` 和一次性 code。
- TokenStore：本期安全本地文件，未来接 OS Keychain。
- BrowserLauncher：只负责打开授权 URL，不处理密码。

### 5.3 运行时工具层

Tomcat v2 采用渐进式披露：

```text
前缀固定：tool_search / tool_describe / tool_call / tool_run_code
              │
              ▼
tool_search()              列连接器 source
tool_search(source=...)    列工具名和短描述
tool_describe([...])       按需取得 schema
tool_call(name, args)      调用真实工具
```

连接器数量变化只改变运行时目录和工具结果，不改变 LLM 缓存前缀。未来 CLI/A2A 只需把自己的 manager 挂到相同的 generic connector API。

## 6. 三阶段路线图

### Phase 1：当前交付

- Settings → Connectors。
- Global 默认范围；Workspace 显式选择并遵守项目级信任。
- 手工添加 MCP stdio/HTTP。
- HTTP 认证：无认证、Bearer、OAuth/PKCE。
- Add HTTP+OAuth 直接启动授权。
- fake HTTP/OAuth server 无人值守主验收。
- GitHub 官方远程 MCP 仅作环境允许时的补充 smoke test。

### Phase 2：Curated Connector Registry

```text
Connectors
├── Installed
└── Marketplace
      ├── Search
      ├── Category
      ├── ConnectorCard
      └── Install & Authorize
```

预置连接器提供 URL、图标、说明、权限和 OAuth client identity。用户不填写底层参数：

```text
点 Install
  → 读取模板
  → 写 Workspace 配置
  → 显示权限说明
  → Install & Authorize
  → 已安装/已连接
```

首批可选：GitHub、Notion、Slack、Playwright。目录初期优先使用随扩展发布的本地 JSON，稳定后再评估远程 registry；远程 registry 必须有签名/版本和回滚机制。

### Phase 3：CLI、主界面与智能推荐

- CLI 下载器：平台选择、签名/checksum 校验、版本升级和回滚。
- device-code/QR 授权：扫码完成后显示已绑定账号。
- 主界面 Connectors 面板：显示已连接连接器和高频能力。
- 连接/断开事件推送，而不是固定轮询。
- 根据项目依赖和对话意图推荐连接器，但推荐不能偷偷安装或授权。

## 7. 必须守住的产品与安全不变量

1. 用户点击授权前能看到连接器身份与权限说明。
2. 未授权或授权失败的 connector 不伪装成已连接。
3. 配置已保存与连接已成功分别展示。
4. Global 是添加默认范围；Workspace 必须显式选择，信任以项目根为单位而非逐命令审批。
5. UI 不回显密码、PAT、refresh token、access token 或 client secret。
6. 外部 URL 只在用户确认的授权流程中打开；未知域名、异常权限和非标准回调必须停止。
7. MCP/CLI/A2A 共用通用 Connector API，不把 MCP 字段硬编码到通用列表和状态组件。
8. fake server 是自动化主验收环境；真实外部服务只能作为兼容性补充，不能承担稳定回归职责。

## 8. 变更记录

- 2026-09：建立文档；纳入 Workspace 默认、Add 直接授权、MCP/CLI 分层、fake server 主验收和未来 Marketplace 方向。
- 2026-10 审查纠错：默认添加范围、项目资源目录和项目信任边界对齐现行实现；后续路线图仍是愿景，不作为当前已交付能力。
