| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tomcat Agent | 2026-11-19 05:21 +0800 | ACTIVE | feat/web-search-backends | - |

### ✅ DONE (已完成/进行中)
- CLI 和共享 Serve 启动时补齐内置网页搜索插件，VS Code 用户不必先执行 `tomcat init`。
- 新增 DeepSeek 官方 Search API 适配器。默认顺序为 OpenAI 托管 → Tavily → Brave → Serper → MiMo → DeepSeek；缺依赖则跳过，不覆盖用户已保存的名单。
- 在 VS Code「设置 → 工具 → 网页搜索」管理后端顺序、参与开关和 Key。将 Key 状态与操作合为一个入口；专用 Key 移除需确认，共用 Key 可跳到模型页管理。
- 已有验证：定向 Rust 回归、扩展单测、类型检查和 GUI 构建；最新界面用示例数据在桌面深色与窄屏浅色下验证了键盘操作、移除确认和模型页跳转。按用户要求，本次提交不重跑测试或测覆盖率，也不安装新版、不重启在用服务。

### 🔌 INTERFACE (接口变更)
- `tools.web_search.backends` 接受有序后端标识名单；单项为指定后端，多项按序回退。兼容旧 `backend`，旧 `*_base_url` 忽略并告警。
- 插件 `web_search.backend` 的 `metadata.backends` 声明可用后端、依赖和默认顺序。已保存的 Key 仅对受信任来源开放，占位符仅在请求头替换；项目插件和显式路径插件不能借用已保存的 Key。
- Serve 新增 `web_search_settings`、`set_web_search_routing`、`clear_provider_key`，保存 Key 复用 `set_provider_key`。设置读取按窗口工作区解析生效插件，路由和凭据更新对共享 Serve 生效。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| DeepSeek 真账号未验证 | 未验证开放平台 Search API 的授权、额度与计费 | 获得明确授权后单独验证 |
| 最新 VSIX 安装验收未执行 | 本轮以构建、定向回归和开发页验证为限 | 发布前在隔离环境验证 |
