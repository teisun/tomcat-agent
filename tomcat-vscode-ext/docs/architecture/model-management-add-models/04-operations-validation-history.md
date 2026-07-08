# 模型管理与 Add Models · 04 配置、错误、验收与历史

> 总览见 [`../model-management-add-models.md`](../model-management-add-models.md)（含定位、阅读顺序与文首导图集）。
> 本文对应 [`ARCHITECTURE_SPEC.md`](../../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) 的 **§6 配置与环境变量**、**§7 错误模型 / 截断 / 警告**、**§8 测试矩阵（验收）**、**§9 风险与应对**、**§10 历史决策 / 跨文档修订**。
> 协议与文件职责见 [`03-protocol-and-runtime.md`](03-protocol-and-runtime.md)；已定稿选型与实施见 [`02-decisions-and-delivery.md`](02-decisions-and-delivery.md)。

---

## 6. 配置与环境变量（SHOULD）

内置预置模型的 provider → 凭证变量（缺省推断 `<PROVIDER>_API_KEY`，可被 `api_key_env` 覆盖）：

| 变量                  | 归属 provider | 内置模型                        | 端点/备注                                          | 说人话           |
| ------------------- | ----------- | --------------------------- | ---------------------------------------------- | ------------- |
| `OPENAI_API_KEY`    | openai      | gpt-5.4 / 5.5 / 5.6         | `api.openai.com` + `/v1/responses`             | OpenAI 的钥匙。   |
| `MIMO_API_KEY`      | mimo        | mimo-v2.5-pro               | `token-plan-cn.xiaomimimo.com`，thinking=doubao | 小米 MiMo 的钥匙。  |
| `DEEPSEEK_API_KEY`  | deepseek    | deepseek-v4-pro / -flash    | `api.deepseek.com`，thinking=deepseek           | DeepSeek 的钥匙。 |
| `ZHIPU_API_KEY`     | zhipu       | glm-5.2                     | `open.bigmodel.cn/api/paas/v4`（path-aware）     | 智谱 GLM 的钥匙。   |
| `MOONSHOT_API_KEY`  | moonshot    | kimi-k2.7-code              | 默认 `api.moonshot.cn` + `/v1/chat/completions`；全球站改 `api.moonshot.ai`     | Kimi 的钥匙。     |
| `ANTHROPIC_API_KEY` | anthropic   | claude-opus-4-8 / 4-7 / 4-6 | `api.anthropic.com` + `/v1/messages`           | Claude 的钥匙。   |

- 存储：`~/.tomcat/assets/.env`（chmod 0600），启动时 `[dotenvy::from_path](../../../../tomcat/src/api/cli/mod.rs)` 载入；`.env` 不覆盖已存在的进程环境变量。
- 优先级：**env > .env 文件 > 推断**（`api_key_env` 显式 > provider 推断）。
- serve 能力门控：`initialize.capabilities` 决定 GUI 是否显示模型管理入口（旧 serve 无则隐藏）。

### 6.1 测试凭证与两个 `.env`（真机验收怎么填）

两个 `.env` 用途不同、内容填一样即可（均已 gitignore）：`~/.tomcat/assets/.env` 是运行时真相（CLI/扩展调用读）；`tomcat/.env` 是 `cargo test` real-llm 夹具（[`tests/common/mod.rs`](../../../../tomcat/tests/common/mod.rs) 的 `load_openai_test_env`）。

凭证来源：OpenAI 走兼容网关、DeepSeek/MiMo 已有、Kimi 官方 key（按平台选择 `.ai` / `.cn`）、**Claude 走中转站；GLM 暂不真机调试（官方 openai 口占位预置）**。

| # | 变量 | 填到哪 | 值 | 覆盖 |
| --- | --- | --- | --- | --- |
| 1 | `OPENAI_GATEWAY_API_KEY` | 两个 | 已有 | OpenAI wire（兼容网关）|
| 2 | `DEEPSEEK_API_KEY` / `MIMO_API_KEY` | 两个 | 已有 | `deepseek-v4-*` / `mimo-v2.5-pro` |
| 3 | `MOONSHOT_API_KEY` | 两个 | Kimi 官方 key | `kimi-k2.7-code` |
| 4 | `ANTHROPIC_API_KEY` | 两个 | 中转站 Claude key | `claude-opus-4-8/4-7/4-6` |
| 5 | `TOMCAT_E2E_ANTHROPIC_BASE_URL` | `tomcat/.env` | `https://fcodex.top` | Claude 测试覆盖 |
| 6 | `TOMCAT_E2E_ANTHROPIC_MODEL` | `tomcat/.env` | `claude-opus-4-6` | 真机测试上游模型名 |

- 凭证已就绪并写入两个 `.env`；`fcodex.top` 已加入 `NO_PROXY`（与兼容网关同，免走代理）。
- Claude 已确认走原生 Anthropic 口（`/v1/messages`），新 `anthropic-messages` provider 可真机验；真机只验 `claude-opus-4-6`（三个 Opus 同 wire，4-6 通即认 4-7/4-8 通）。
- GLM `glm-5.2` 以官方 OpenAI Chat 口（`/api/paas/v4`）占位预置，本期不填 `ZHIPU_API_KEY`、不真机调试，保持「Needs API key」状态（正好当 path-aware `/api/paas/v4` 构造用例 + 门控样本）。GLM 官方另支持 Anthropic 口 `/api/anthropic`，本期不启用。
- 内置预置默认 base_url 仍指官方；Kimi 默认走中国站 `api.moonshot.cn`，若 key 属全球站则用 `api.moonshot.ai` 覆盖；中转站仅本机测试覆盖；集成测试 base_url 仿 `MIMO_TEST_BASE_URL_ENV`（[`tests/common/mod.rs`](../../../../tomcat/tests/common/mod.rs)）。
- OpenAI 官方口不测，用兼容网关覆盖；未填 Key 的官方内置（OpenAI/GLM）保持「Needs API key」，当门控样本。

---

## 7. 错误模型 / 归一化结局（SHOULD）

```text
models.toml 解析失败        → Err(AppError::Config)（带文件路径与行号）
upsert 缺 api/provider      → Err(AppError::Config)（提示必填字段）
remove 目标是 Builtin       → Err（内置不可删；提示用同 id 覆盖）
remove 目标不存在           → Err（unknown_model）
set_provider_key value 空   → Err（拒绝写空 Key）
写盘并发冲突                → 文件锁串行化；超时 → Err（提示重试）
模型 api 未注册             → Err（列出已注册 api：openai/openai-responses/anthropic-messages）
选中模型但 keyPresent=false → 非致命：Ready 列表不含它；set_model 前置校验给出「先填 Key」提示
Anthropic HTTP 4xx/5xx      → 归一 AppError（401→Key 无效；429→退避重试，复用现有 backoff）
```

> 说人话：能拦在写盘前的都返回明确 `Err`（缺字段、删内置、写空 Key）；「没 Key」不是错误，只是模型停在「待填」状态、不进下拉；Anthropic 的网络错误并入既有 LLM 错误/退避通道。

---

## 8. 测试矩阵（MUST）

| 维度           | 用例 / 编号                                                                                                                                                 | 状态               | 说人话                |
| ------------ | ------------------------------------------------------------------------------------------------------------------------------------------------------- | ---------------- | ------------------ |
| 单元(Rust)     | `core/llm/tests/admin_test.rs`：upsert/remove 往返、set_key 读写、保留其它键、并发锁、同 resolver Key 轮换重建 provider                                                       | ✅ 2026-07-07    | 写盘中枢的细节锁死。         |
| 单元(Rust)     | `catalog::tests` + `core/llm/endpoint.rs::tests`：builtin 预置齐全、用户 id 合并不丢、path-aware endpoint(bare vs 带路径)                                                     | ✅ 2026-07-07    | 内置齐全 + 地址拼接对。      |
| 单元(Rust)     | `thinking_policy_test` / `replay_policy_test`：Anthropic `thinking` 映射、签名保留、同 profile replay 语义                                                               | ✅ 2026-07-07    | Claude wire 与 continuity 口径一致。 |
| 单元(Rust)     | `resolver::tests`：catalog reload 后新模型可解析                                                                                                                | ✅ 2026-07-07    | 热重载即时生效。           |
| 单元(TS/React) | `Composer.test.tsx`：分隔符/Add Models 页脚/能力缺失回退原生 `<select>`                                                                                              | ✅ 2026-07-07    | 下拉排版与门控。           |
| 单元(TS/React) | `SettingsApp.test.tsx`：表单校验、Ready/NeedsKey 分组、password 输入、两次调用、不回显密钥                                                                                     | ✅ 2026-07-07    | 设置页交互与保护。          |
| 集成           | `tomcat-vscode-ext/tests/*`：upsert→list_models/source/keyPresent 往返；`webview_provider_flow.test.ts` 锁定 capability/state 流转与 `currentState()` 不含明文 Key | ✅ 2026-07-07    | serve 往返正确，前端状态也不带钥匙。 |
| 集成           | `api/cli/tests`：`tomcat model add/list/remove/key/default`（临时 HOME）                                                                                        | ✅ 2026-07-07    | CLI 端到端。           |
| real-LLM 冒烟  | env 门控可跳过：GLM/Kimi 用官方 Key、Anthropic 用中转站 Key + `TOMCAT_E2E_ANTHROPIC_BASE_URL` 覆盖、OpenAI 走兼容网关（凭证见 §6.1）                                          | 环境门控（按需冒烟） | 真机连通冒烟。            |
| 安全(关键承诺)     | `commands_test.rs` 锁定 set_provider_key 响应 / list_models / list_provider_keys 无明文；`webview_provider_flow.test.ts` 锁定 webview 快照无明文；`admin_test.rs` 锁定 `.env` 0600；日志脱敏 | ✅ 2026-07-07    | 钥匙不泄漏（见 [`02-decisions-and-delivery.md`](02-decisions-and-delivery.md) 的 §3.1 D8）。 |
| E2E          | `E2E-MODEL-001`（`[E2E_SCENARIO_LIBRARY.md](../../../../tomcat/docs/openspec/specs/guides/testing/E2E_SCENARIO_LIBRARY.md)`）：下拉→设置页→加模型/填 Key→回下拉→选中 prompt | 已实现（默认不在本轮执行） | 用户真链路跑通。           |
| 回归           | `tomcat doctor` 不回归；旧 models.toml 不被破坏；无 Key 内置不误判可用                                                                                                    | ✅ 2026-07-07    | 不弄坏现有的。            |
| 文档           | 本文定稿 + `[tomcat-vscode-extension-phase2.md](../tomcat-vscode-extension-phase2.md)` 同步 list_models 出参扩展                                                     | ✅ 2026-07-06（本文） | 字和代码别两张皮。          |

---

## 9. 风险与应对（MUST）

| 风险                       | 影响  | 应对（具体动作）                                                                                             | 说人话                                |
| ------------------------ | --- | ---------------------------------------------------------------------------------------------------- | ---------------------------------- |
| catalog 热重载并发            | 中   | `GlobalServices.model_catalog` 用 `RwLock<Arc<_>>`/arc-swap；写盘后整体替换 Arc（读写不撕裂）                        | 换目录时整块替换，读的人不会看到半截。                |
| Key 轮换（改已存在值）            | 中   | `dotenvy` 不覆盖已存在变量、Rust 2024 禁 unsafe `set_var`；AuthStore 改读「可刷新 .env 快照」或提示新会话生效                    | 换钥匙是边界情况，必要时开新会话生效。                |
| GLM thinking_format 不确定  | 低   | 先 `zai`，用契约测试对真实响应校验；不对再切 deepseek/doubao 系                                                          | 智谱思考参数先猜后验，错了就换。                   |
| base_url path-aware 误判   | 低   | 启发式「是否含非根路径」；保留可选显式 `wire_path` 覆盖作安全阀                                                               | 极少数带路径又要 /v1 的网关能手动指定。             |
| Anthropic 签名/thinking 回放 | 中   | 复用 `StreamEvent::Thinking.signature`；`replay_policy.rs` 加「签名保留」规则(区别 deepseek strip)；多轮保留 thinking 块 | Claude 多轮要保住思考签名，不能像 deepseek 那样删。 |
| 协议漂移(TS/Rust 不一致)        | 中   | 构建期 `tomcat serve --print-schema` 刷 `wire.d.ts`；capability 门控旧 serve                                 | 后端一改类型自动同步，前端编译期发现。                |
| Key 明文泄漏面                | 高   | value 只入不出、响应仅 `keyPresent`、日志/审计脱敏、input type=password、`.env` 0600                                  | 钥匙全程不回显、不落日志。                      |
| 预置 seed 收敛回归                  | 中   | 删掉手写 `MANAGED_MODELS` 字符串表，改为内嵌 `builtin_models.toml` 单源释放；保留兼容：用户既有 models.toml 同 id 覆盖；加预置完整性 / 零漂移测试                                   | 收敛单源别把用户已有配置搞丢。                      |

---

## 10. 历史决策 / 跨文档修订（SHOULD）

- ~~内置模型分「builtin_models() + init 追加 MANAGED_MODELS」双表~~ → 否：两处真相、init 未跑则缺模型；本文收敛为**运行时单源内嵌 `builtin_models.toml` + init 从其原样释放 seed 到 models.toml**（见 [`02-decisions-and-delivery.md`](02-decisions-and-delivery.md) 的 §3.1 D4）。
- ~~模型清单只能手改 models.toml，无 UI/CLI~~ → 否：本文新增设置中心 + `tomcat model` CLI + serve 命令。
- ~~OpenAI wire 硬编码~~ `{base}/v1/chat/completions` → 否：改为 path-aware，兼容 GLM 的 `/api/paas/v4`（见 [`02-decisions-and-delivery.md`](02-decisions-and-delivery.md) 的 §3.1 D5）。
- 跨文档修订：`[tomcat-vscode-extension-phase2.md](../tomcat-vscode-extension-phase2.md)` 中「webview 只拿 id 字符串、`list_models` 出参较薄」的描述，被本文扩展为「`list_models` 出参附 `source/apiKeyEnv/keyPresent`」；相邻 `04-protocol-runtime.md` 的 serve 命令表在实现 IP1d 时须同步登记新命令。
