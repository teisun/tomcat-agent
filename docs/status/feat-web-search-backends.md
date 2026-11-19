| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tomcat Agent | 2026-11-19 21:23 +0800 | ACTIVE | feat/web-search-backends | - |

### ✅ DONE (已完成/进行中)
- CLI 和共享 Serve 启动时补齐内置网页搜索插件，VS Code 用户不必先执行 `tomcat init`。
- 内置插件负责 DeepSeek 官方 Search API 和 OpenAI 官方 Responses 搜索。默认顺序为 DeepSeek → Tavily → Brave → Serper → MiMo → OpenAI；缺依赖则跳过，不覆盖用户已保存的名单。
- 在 VS Code「设置 → 工具 → 网页搜索」管理后端顺序、参与开关和 Key。默认选中「网页搜索」二级标签；「去配置」行内填写，「已配置 ▾」查看来源或管理 Key。
- OpenAI 搜索固定使用 `gpt-6.2-sol`，只复用地址为官方域名的模型 Key，不改变聊天模型或网关。旧模型 `capabilities.web_search` 会被忽略。
- CLI 升至 `0.1.100`，VS Code 扩展升至 `0.2.18`，内置 CLI 同步为 `0.1.100`。本次提交与本地 `darwin-x64` 整包构建不重跑测试或测量覆盖率，不安装或发布。

### 🔌 INTERFACE (接口变更)
- `tools.web_search.backends` 接受有序后端标识名单；单项为指定后端，多项按序回退。兼容旧 `backend`，旧 `*_base_url` 忽略并告警。
- 插件 `web_search.backend` 的 `metadata.backends` 声明可用后端、依赖和默认顺序。新占位符 `{{host_key}}` 按 HTTPS 目的主机查 Key，仅在请求头替换；项目插件和显式路径插件不能借用已保存的 Key。
- Serve 新增 `web_search_settings`、`set_web_search_routing`、`clear_provider_key`，保存 Key 复用 `set_provider_key`。设置读取按窗口工作区解析生效插件，路由和凭据更新对共享 Serve 生效。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| DeepSeek 真账号未验证 | 未验证开放平台 Search API 的授权、额度与计费 | 获得明确授权后单独验证 |
| 最新 VSIX 安装验收未执行 | 本轮只构建整包，不运行安装验收 | 发布前在隔离环境验证 |
