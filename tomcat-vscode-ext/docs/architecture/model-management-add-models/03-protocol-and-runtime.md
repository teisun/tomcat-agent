# 模型管理与 Add Models · 03 协议与运行时参考

> 总览见 [`../model-management-add-models.md`](../model-management-add-models.md)（含定位、阅读顺序与文首导图集）。
> 本文对应 [`ARCHITECTURE_SPEC.md`](../../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) 的 **§4 协议** 与 **§5 One-Glance Map**。
> 上游设计见 [`02-decisions-and-delivery.md`](02-decisions-and-delivery.md)（已定稿选型与实施）。
> 单一事实源：serve 命令与出参最终定义在 `tomcat/src/api/serve/types.rs`；`--print-schema` 同步到 `tomcat-vscode-ext/src/serveClient/wire.d.ts`；webview↔host intent 定义在 `src/ui/webview/protocol.ts` 与 `gui/src/types.ts`。

---

## 4. 协议（MUST）

单一事实源：serve 命令与出参最终定义在 `[tomcat/src/api/serve/types.rs](../../../../tomcat/src/api/serve/types.rs)`；`--print-schema` 同步到 `[serveClient/wire.d.ts](../../../src/serveClient/wire.d.ts)`。webview↔host intent 定义在 `[ui/webview/protocol.ts](../../../src/ui/webview/protocol.ts)` 与镜像 `[gui/src/types.ts](../../../gui/src/types.ts)`。

### 4.1 新增 serve 命令（stdin，NDJSON）

| 命令 `type`            | 字段                                 | 必填  | 语义                                       | 说人话             |
| -------------------- | ---------------------------------- | --- | ---------------------------------------- | --------------- |
| `upsert_model`       | `model: ModelEntryInput`           | 是   | 新增/覆盖用户模型 → `models.toml`（同 id 覆盖）       | 存一条模型配置。        |
| `remove_model`       | `id: string`                       | 是   | 删除 User 源模型；Builtin 拒绝                   | 删掉自己加的模型。       |
| `set_provider_key`   | `envName: string`, `value: string` | 是   | 写 `.env`(0600)；`value` 只入不出              | 存一把钥匙（永不回显）。    |
| `list_provider_keys` | —                                  | 否   | 每次重读 `.env`，仅列名称合法且非空的 `*_API_KEY`；只回名称/布尔，不按模型引用补名称 | 热刷新有哪些已保存钥匙。       |
| `list_models`（扩展）    | `id?`                              | 否   | 出参每项**新增** `source/apiKeyEnv/keyPresent` | 列模型时带上来源和有没有钥匙。 |

### 4.2 出参：`ModelView`（`list_models.payload.models[]`）

| 字段               | JSON 类型              | 必填  | 默认      | 说明                                               | 说人话         |
| ---------------- | -------------------- | --- | ------- | ------------------------------------------------ | ----------- |
| `id`             | string               | 是   | —       | 本地模型 id                                          | 选它用的名字。     |
| `modelName`      | string\|null         | 否   | =id     | 上游真实模型名                                          | 实际发给上游的名字。  |
| `api`            | string               | 是   | —       | `openai`\|`openai-responses`\|`anthropic-messages` | 走哪套协议。      |
| `provider`       | string               | 是   | —       | 逻辑厂商                                             | 归哪个厂商/哪把钥匙。 |
| `baseUrl`        | string\|null         | 否   | 推断      | 主机(可含路径)                                         | 连哪个地址。      |
| `capabilities`   | object               | 是   | 全 false | vision/files/tools/reasoning/web_search          | 能干什么。       |
| `thinkingFormat` | string\|null         | 否   | auto    | 推理格式                                             | 思考参数怎么发。    |
| `contextWindow`  | number\|null         | 否   | 配置默认    | 上下文窗口                                            | 能记多长。       |
| `source`         | `"builtin"`\|`"user"` | 是   | —       | 出厂 vs 用户                                         | 官方内置还是你加的。  |
| `apiKeyEnv`      | string               | 是   | 推断      | 凭证**变量名**（非密钥值）                                  | 钥匙放哪个抽屉。    |
| `keyPresent`     | bool                 | 是   | —       | 对应变量是否非空                                         | 有没有钥匙。      |

> **安全三态**：协议中**永远没有** Key 明文或部分字符字段；`set_provider_key` 响应只回 `{ envName, keyPresent:true }`；`ModelView` 与 webview 状态快照均不含密钥。`list_provider_keys` 以 `~/.tomcat/assets/.env` 为唯一清单来源，每次调用重读文件，因此手工新增/删除合法的非空 `*_API_KEY` 后无需重启 serve。

### 4.3 webview↔host intent（新增）

- 侧栏 Agent Box → host：`openModelSettings`（无参）→ host 调 `SettingsPanel.reveal("models")`。
- 设置中心 webview ↔ host：`settings.ready` / `listModels` / `upsertModel` / `removeModel` / `setProviderKey`（上行）；host → webview `state{ route, ready, capabilities, models, providerKeys, error? }`（下行）。
- 门控：仅当 serve `initialize.capabilities` 含 `upsert_model`/`set_provider_key` 时，下拉「Add Models」与设置页可用；否则回退旧 `<select>`。

### 4.4 调用样例（jsonc）

```jsonc
// 1) 新增自定义模型（只写模型，不带 Key）
→ { "type": "upsert_model", "id": "u1",
    "model": { "id": "my-glm", "api": "openai", "provider": "zhipu",
               "baseUrl": "https://open.bigmodel.cn/api/paas/v4",
               "capabilities": { "tools": true, "reasoning": true } } }
← { "type": "response", "id": "u1", "payload": { "id": "my-glm", "source": "user" } }

// 2) 为该 provider 填 Key（密钥只入不出）
→ { "type": "set_provider_key", "id": "k1", "envName": "ZHIPU_API_KEY", "value": "<secret>" }
← { "type": "response", "id": "k1", "payload": { "envName": "ZHIPU_API_KEY", "keyPresent": true } }
//   注意：payload 里没有、也永远不会有 value。

// 3) 列模型（出参带 source/keyPresent，不含明文）
→ { "type": "list_models", "id": "l1" }
← { "type": "response", "id": "l1", "payload": { "models": [
      { "id": "gpt-5.4", "api": "openai-responses", "provider": "openai",
        "source": "builtin", "apiKeyEnv": "OPENAI_API_KEY", "keyPresent": false },
      { "id": "my-glm", "source": "user", "apiKeyEnv": "ZHIPU_API_KEY", "keyPresent": true }
    ] } }
```

---

## 5. 文件职责总览（One-Glance Map）（MUST）

```text
Rust（tomcat/src/）
  core/llm/admin.rs 【NEW ★写盘中枢】
    · list_model_views(cfg) → Vec<ModelView>（合并+标 source/key_present）
    · upsert_user_model / remove_user_model（models.toml：文件锁+原子写+load 校验）
    · set_provider_key / list_provider_keys（.env：0600、每次 list 热重读、仅合法 *_API_KEY、完整值只入不出）
    · 写后 reload catalog 句柄            [core/llm/tests/admin_test.rs]
        │ 读/写
        ▼
  core/llm/builtin_models.toml 【NEW】内嵌全部预置(OpenAI/MiMo/DeepSeek/GLM/Kimi/Opus)
  core/llm/catalog.rs 【改】解析内嵌 builtin_models.toml 并生成运行时 builtin catalog
    · infer_default_base_url() 追加 zhipu/moonshot/anthropic
        │
        ├─▶ core/llm/openai.rs / openai_responses/ 【改】endpoint path-aware（去掉硬编码 /v1）
        ├─▶ core/llm/anthropic/{mod,wire,stream}.rs 【NEW】/v1/messages + x-api-key + SSE + thinking
        ├─▶ core/llm/registry.rs 【改】PROVIDERS += ("anthropic-messages", build_anthropic)
        ├─▶ core/llm/thinking_policy.rs 【改】+ ThinkingFormat::Anthropic（enabled+budget_tokens 单源）
        ├─▶ core/llm/replay_policy.rs 【改】anthropic 签名保留规则（区别 deepseek strip）
        └─▶ core/llm/resolver.rs 【改】读可重载 catalog 当前值
        │
  api/chat/session_runtime.rs 【改】GlobalServices.model_catalog: RwLock<Arc<ModelCatalog>>
  api/chat/context.rs 【改】构造可重载句柄
        │
  api/serve/types.rs 【改】ServeCommand += UpsertModel/RemoveModel/SetProviderKey/ListProviderKeys
    · 补 command_id()/session_id()/wire_type()
  api/serve/commands.rs 【改】handle_command 新分支 → core::llm::admin::*；ListModels 出参加 source/key
  api/serve/control.rs 【改】initialize.capabilities += upsert_model/remove_model/set_provider_key
        │                                       [api/serve/tests/commands_test.rs]
  api/cli/model_cmd.rs 【NEW】run_model(ModelSub) 复用 admin.rs（list/add/remove/key set/key list/default）
  api/cli/mod.rs 【改】Commands::Model{sub:ModelSub}；run_cli match；nested 守卫；parse_cli_test
  api/cli/models_toml.rs 【改】删除手写 MANAGED_MODELS，改为按内嵌 builtin_models.toml 原样释放/按块追加 seed 并收敛为单源
  api/cli/init_model_wizard.rs 【改】read/write_env_entries 下沉供 admin 复用

TypeScript（tomcat-vscode-ext/src、gui/）
  serveClient/wire.d.ts / protocol.ts 【改】新命令/响应类型
  serveClient/TomcatMessenger.ts 【改】+sendUpsertModel/sendRemoveModel/sendSetProviderKey/sendListProviderKeys
  serveClient/initialize.ts 【改】+SERVE_CAPABILITY_UPSERT_MODEL 等 + 门控
  ui/webview/provider.ts 【改】handleIntent 处理 openModelSettings → SettingsPanel.reveal
  ui/webview/protocol.ts + gui/src/types.ts 【改】+openModelSettings intent（两处镜像）
  ui/settings/SettingsPanel.ts 【NEW】editor-area WebviewPanel 单例(retainContextWhenHidden/CSP/nonce)+typed intent/state 协议
  package.json 【改】contributes.commands += tomcat.openSettings
  gui/vite.config.ts 【改】多入口（新增 settings.html/settings/main.tsx）
  gui/src/components/Composer.tsx 【改】自定义下拉替换原生 select（分隔符+Add Models 页脚）
  gui/src/settings/{main.tsx,Shell.tsx,ModelsPage.tsx,...} 【NEW】左导航/右内容 + Models 页（密文 Key）
  gui/src/shared/* 【NEW】共享 acquireVsCodeApi + 协议类型（两个 webview 复用）
        │                                       [tomcat-vscode-ext/tests/, gui 下 *.test.tsx]
```

> 阅读顺序：自顶向下就是一条链——**`admin.rs`（唯一写盘）←** `catalog.rs`**/provider（模型与 wire）← serve/CLI 两门面 ← TS 宿主 ← 两个 webview（侧栏下拉 + 编辑器区设置页）**。说人话：后端加一个「写模型/写钥匙」的中枢，前面挂命令行和界面两个入口；界面拆成「侧栏下拉」和「独立设置标签页」两块，配置真正落到 `models.toml` 和 `.env`。
