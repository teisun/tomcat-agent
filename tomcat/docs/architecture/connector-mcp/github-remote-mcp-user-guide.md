# 使用 GitHub 官方远程 MCP

Tomcat 支持 GitHub 官方的 Streamable HTTP MCP 服务。最小配置只需要 URL；Tomcat 会在需要时通过浏览器完成 OAuth 授权。

## 最小配置

在 Tomcat Connectors 设置中选择：

- **Type**：MCP
- **Scope**：Global（默认）；需要项目共享时显式选择 Workspace
- **Transport**：HTTP
- **URL**：`https://api.githubcopilot.com/mcp/`
- **Authentication**：OAuth 2.0

点击 **Add** 后，Tomcat 会：

```text
保存所选范围的 mcp.json（以 UI 显示路径为准）
          │
          ▼
请求 GitHub MCP → 读取 401 WWW-Authenticate
          │
          ▼
发现 protected-resource 与 OAuth authorization metadata
          │
          ▼
生成 PKCE/state → 打开系统浏览器
          │
          ▼
GitHub 授权 → 127.0.0.1 临时回调
          │
          ▼
保存 token 元数据 → 重连 MCP → tools/list
```

不需要手工创建 OAuth App，不需要把 GitHub 密码、PAT 或 client secret 填进 Tomcat。只有授权页面由 GitHub 处理登录和权限确认。

Global 配置路径由 `get_work_dir(cfg)/mcp.json` 决定；Workspace 对应 `<项目根>/<workspace.project_resource_dir>/mcp.json`，项目资源目录默认 `.agents`。不要硬编码为 `.tomcat`；未打开项目时 Workspace 不可选。Workspace 来源还须通过下述项目级信任，OAuth 同意不能替代项目信任。

## 权限边界

1. Tomcat 只请求 MCP authorization server 宣布的 scope；不会自行扩大权限。
2. 连接器工具目录通过 `tool_search` 按需发现，不会把完整 GitHub 工具 schema 放进每轮前缀。
3. 工具开关控制哪些工具可以被 `tool_search` 发现并由 `tool_call` 调用。
4. GitHub 账号本身决定授权范围；Tomcat 不会保存你的 GitHub 密码。
5. `connector-oauth.json` 只保存连接器 token 元数据和刷新所需凭证，并设置为用户私有文件权限；UI 和日志不回显 token。
6. Workspace 配置描述“连接到哪里”，OAuth token 属于本机，不会写入项目配置，也不应提交到 Git。

## 重新授权与退出

在 Connector 详情中：

- **Login / Re-login**：重新打开 GitHub 授权流程。
- **Logout**：删除本机该连接器的 OAuth token，并退休同一后端内共享该凭据的缓存连接；不承诺撤销已发送的远端操作、其他后端进程缓存或 GitHub 账号上的授权。
- **Reload**：重新连接并刷新 `tools/list`。
- **Remove**：删除所选来源配置并清理对应本机凭据；如需撤销服务商授权，请同时在 GitHub 的授权设置中撤销。

## 常见问题

### 显示“配置已保存，但尚未授权”

这是正常的分阶段状态：配置已经写入，但 OAuth 还没有完成。点击 **Login / Re-login**，完成浏览器授权后再 Reload。

### 浏览器授权后仍显示失败

检查：

- 回调窗口是否访问了 `http://127.0.0.1:<动态端口>/callback`。
- 是否在授权完成前关闭了 Tomcat。
- 网络是否允许访问 GitHub MCP 与 authorization server。
- 系统代理是否只代理了浏览器而没有代理 Tomcat 进程。
- 是否修改了 Workspace 配置中的 URL；授权服务器变化后需要重新登录。

### 401 / token 过期

Tomcat 会优先使用本机保存的 access token，并在存在 refresh token 时尝试一次刷新。刷新被拒绝时，连接器会回到需要授权状态；不要把 PAT 粘贴到 `mcp.json`，直接使用 Login / Re-login。

### 显示“等待项目信任”

Workspace 连接器受项目级信任控制：用户确认的是当前项目根，不是逐条命令或 URL 的审批清单。同一未信任项目下的服务共用一个 Trust 入口；信任前不启动，Global 来源不因此被阻塞。项目一旦受信任，配置变更不会自动新增逐命令确认；不要把项目来源的普通配置和 OAuth 授权混为一谈。

### GitHub 官方远程服务暂时不可用

真实 GitHub 服务属于网络 smoke test，不是回归测试依赖。Tomcat 的稳定验收使用本地 fake Streamable HTTP/OAuth server；服务商临时故障不会影响本地连接器功能验证。
