| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Jerry | 2026-06-04 20:22 +0800 | PENDING_INTEGRATION | feature/web-search | — |

### ✅ DONE
- [✓] **[P1]** 认领 `T2-P1-012`，任务卡与看板索引已切到 `DOING / Jerry`
- [✓] **[P1]** 完成 `PR-WS-A`：注册 `web_search`、落 6 字段 schema、接入 `tool_exec/branches/web_search.rs`，并保持 reviewer / verifier 不开放联网检索
- [✓] **[P1]** 完成 `PR-WS-S`：实现 Tavily / Brave / Serper adapters、`ToolsWebSearchConfig`、per-provider `.env` / env key、`moka` LRU + TTL cache、`auto` HTTP fallback
- [✓] **[P1]** 完成 `PR-WS-O`：实现 project-level hosted 候选模型发现、显式 `openai` / `auto` hosted 路径、`openai_server.rs` 归一化与 `Capabilities.web_search`
- [✓] **[P1]** 完成 `PR-WS-W`：在 `normalize_hits` 落 SSRF / 私网 / loopback / 单段 host 拦截，以及 `allowed_domains` / `blocked_domains` 过滤
- [✓] **[P1]** 已补 `tests/web_search_tool_tests.rs` 并登记 `scripts/test-groups.sh`
- [✓] **[P1]** 本地验证通过：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`./scripts/run-integration-tests.sh lib`、`./scripts/run-integration-tests.sh integration`、`PI_LIVE_WEB_SEARCH=1 cargo test --test web_search_tool_tests live_tavily_search_smoke -- --nocapture`

### 🔌 INTERFACE
- `web_search` 现为真实可执行工具，schema 固定为 `query / count / freshness / country / language / domain_filter`
- 输出统一为 `{ hits, query, backend, stats, truncated, warnings }`
- `backend=auto` 现按 `openai(hosted project candidate) -> tavily -> brave -> serper` 选择，并在缺 key / 401 / 403 / 429 / 5xx / timeout / transport fail 时自动降级

### ⚠️ BLOCKED
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | - | - |
