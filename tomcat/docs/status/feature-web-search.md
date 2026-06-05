| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Jerry | 2026-06-05 07:12 +0800 | PENDING_INTEGRATION | feature/web-search | — |

### ✅ DONE
- [✓] **[P1]** 认领 `T2-P1-012`，任务卡与看板索引已切到 `DOING / Jerry`
- [✓] **[P1]** 完成 `PR-WS-A`：注册 `web_search`、落 6 字段 schema、接入 `tool_exec/branches/web_search.rs`，并保持 reviewer / verifier 不开放联网检索
- [✓] **[P1]** 完成 `PR-WS-S`：实现 Tavily / Brave / Serper adapters、`ToolsWebSearchConfig`、per-provider `.env` / env key、`moka` LRU + TTL cache、`auto` HTTP fallback
- [✓] **[P1]** 完成 `PR-WS-O`：实现 project-level hosted 候选模型发现、显式 `openai` / `auto` hosted 路径、`openai_server.rs` 归一化与 `Capabilities.web_search`
- [✓] **[P1]** 完成 `PR-WS-W`：在 `normalize_hits` 落 SSRF / 私网 / loopback / 单段 host 拦截，以及 `allowed_domains` / `blocked_domains` 过滤
- [✓] **[P1]** 已补 `tests/web_search_tool_tests.rs` 并登记 `scripts/test-groups.sh`
- [✓] **[P1]** 本地验证通过：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`./scripts/run-integration-tests.sh lib`、`./scripts/run-integration-tests.sh integration`、`PI_LIVE_WEB_SEARCH=1 cargo test --test web_search_tool_tests live_tavily_search_smoke -- --nocapture`
- [✓] **[P1]** 方案复核整改：修正 catalog / tool-catalog 过期占位说明；`web_search.md` 对齐 hosted sidecar 落点；Brave/Serper 域名改写 warning、Tavily warning 命名统一；相关单测 / 集成测复绿
- [✓] **[P1]** 已完成 `T2-P1-013` 当前批次：`web_fetch` 已注册 `url / prompt / format` schema，接入 catalog / `tool_exec` / chat runtime 注入链，并补 `missing runtime` 友好错误
- [✓] **[P1]** 已完成 `web_fetch` 主链：`validate.rs` 的 URL 校验 / SSRF 守卫、`http -> https` 首跳升级、受控重定向、`html2md` 转换、超大正文 `.md` 落盘、PDF/图片/二进制落盘、magic 覆盖错误 `content-type`、moka 缓存；`PR-WF-D / PR-WF-P` 继续后置
- [✓] **[P1]** 本地验证通过：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test web_fetch --lib`、`cargo test --test web_fetch_tool_tests`、`cargo test --test tool_catalog_doc`
- [✓] **[P1]** 已完成 `web_fetch` 收口整改：`markdownify.rs` 正则改 `LazyLock`，`validate.rs` 的 secret-prefix 误报收紧为边界匹配，`application/json` / `*xml` 改为 verbatim 返回，`prompt_ignored_mvp` 改为按本次请求现算且不再污染缓存，off-host redirect 结果不进缓存
- [✓] **[P1]** 文档 / 验收已对齐：`web_fetch.md` 改为显式说明工具描述来自 `catalog.description`、`validate.rs` 当前拒绝所有 IP literal，并登记 DNS 解析型 SSRF 残留；补 `tool_exec` 成功路由到 `web_fetch` 的回归测试并复绿
- [✓] **[P2]** `PLAN_SPEC.md` 收紧决策表与 todos 映射：已拍板决策须逐项对应 todo，不再允许多项决策合并为一个 todo 而不落点

### 🔌 INTERFACE
- `web_search` 现为真实可执行工具，schema 固定为 `query / count / freshness / country / language / domain_filter`
- 输出统一为 `{ hits, query, backend, stats, truncated, warnings }`
- `backend=auto` 现按 `openai(hosted project candidate) -> tavily -> brave -> serper` 选择，并在缺 key / 401 / 403 / 429 / 5xx / timeout / transport fail 时自动降级
- `web_fetch` 已落地为真实可执行工具，schema 为 `url / prompt / format`
- 输出统一为 `{ url, code, code_text, content_type, bytes, result, total_chars, duration_ms, cached, persisted_output_path, redirect, truncated, warnings }`
- 本批次安全边界仅包含 URL 校验 / SSRF 守卫、受控重定向与正文/二进制分流；`PermissionScope::Domain` / `check_domain` / host 会话授权仍后置

### ⚠️ BLOCKED
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
