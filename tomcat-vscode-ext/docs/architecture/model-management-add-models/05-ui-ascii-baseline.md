# 模型管理与 Add Models · 05 UI ASCII 设计基线

> 总览见 [`../model-management-add-models.md`](../model-management-add-models.md)（含定位、阅读顺序与文首导图集）。
> 本文承接总览中的架构导图，完整保留原方案里的「总分 ASCII UI 设计图」，只做目录拆分，不改设计内容。
> 相关术语与 Key 保护见 [`01-scope-and-research.md`](01-scope-and-research.md)；已定稿决策见 [`02-decisions-and-delivery.md`](02-decisions-and-delivery.md)。

---

## 附录 A：总分 UI 设计图集（源自开发计划，全量保留）

> 本附录完整保留特性开发计划中的「总图 + 分图」ASCII 设计，作为 UI/交互事实基线；与总览中的文首导读 A.1/A.2（架构视角）互补，本附录聚焦**界面排版与交互**视角。

### A.总图：端到端数据流

```text
┌──────────────────────────── VS Code ────────────────────────────┐
│  Secondary Side Bar                     Editor Area              │
│  ┌────────────────────┐   openModel   ┌────────────────────────┐│
│  │ Agent Box (gui #1) │  Settings     │ Settings Hub (gui #2)  ││
│  │  Composer          │  intent       │ ┌────────┬───────────┐ ││
│  │   Model ▾          │──────────────▶│ │ Nav    │  Models   │ ││
│  │   · ready models   │               │ │ Models●│  form+list│ ││
│  │   ───────────────  │               │ │ Lang ○ │           │ ││
│  │   + Add Models …   │               │ │ Skill○ │           │ ││
│  └─────────┬──────────┘               │ └────────┴─────┬─────┘ ││
│            │ setModel                 upsert/remove/    │intent ││
│            ▼                          setKey            ▼       ││
│         Extension Host (TS)  ── TomcatMessenger(NDJSON) ──      ││
│            │ list_models / set_model / upsert_model /           ││
│            │ remove_model / set_provider_key                    ││
└────────────┼─────────────────────────────────────────────────────┘
             ▼
        tomcat serve (Rust)  commands.rs 派发
             ▼
        core::llm::admin  ── 单一事实来源，CLI 也调它
         · list_model_views()      读
         · upsert_user_model()  ─写→ models.toml
         · remove_user_model()      │
         · set_provider_key()   ─写→ assets/.env (0600)
             │ reload                │
             ▼                       ▼
        ModelCatalog（可重载句柄，builtin + user 合并）
         · builtin: OpenAI(5.4/5.5/5.6)·MiMo·DeepSeek·GLM·Kimi·Opus×3
         · registry(api) → provider
             openai | openai-responses | anthropic-messages(新)
```

同一套 `core::llm::admin` 被两个入口复用：

```text
tomcat model (CLI)  ─┐
                     ├─▶ core::llm::admin ─▶ models.toml / .env
serve commands (GUI)─┘
```

### A.分图 1：下拉框（照抄附件2）

原生 `<select>` 无法渲染分隔符与「页脚动作」，改为**自定义下拉**（参考 `[SessionBar.tsx](../../../gui/src/components/SessionBar.tsx)` 的 open/click-outside/Esc 模式），只列 `key_present` 可用模型。

```text
Model ▾
┌─────────────────────────┐
│ deepseek-v4-pro         │  只列「可用(有Key)」模型
│ gpt-5.4                 │
│ kimi-k2.7-code          │
│ glm-5.2                 │
│ claude-opus-4-8         │
├─────────────────────────┤  分隔符
│ + Add Models …          │  → 发 openModelSettings intent
└─────────────────────────┘
```

### A.分图 2：设置中心（照抄附件3，编辑器区标签页）

```text
┌ Tomcat Settings ─────────────────────────────────── □ x ┐
│ ┌──────────────┬───────────────────────────────────────┐│
│ │ ▸ Models   ● │  Models                                ││
│ │   Language ○ │  ┌ Add model ──────┐  ┌ Configured ──┐ ││
│ │   Skills   ○ │  │ id   [        ] │  │ Ready         │ ││
│ │   Plugins  ○ │  │ api  ▾  provider│  │ · gpt-5.4  ×  │ ││
│ │              │  │ base_url [    ] │  │ · glm-5.2  ×  │ ││
│ │  ○=未实现占位 │  │ API Key ●●●●●●  │  │ Needs API key │ ││
│ │              │  │ caps □v □f □t.. │  │ · opus-4-8    │ ││
│ │              │  │ [  Save model ] │  │  ●●●●● [Save] │ ││
│ │              │  └─────────────────┘  └───────────────┘ ││
│ └──────────────┴───────────────────────────────────────┘│
└──────────────────────────────────────────────────────────┘
```

左导航是「模块入口」（后续 语言/Skill/Plugin 配置都挂这里；本期只 Models 可点，其余灰置占位）。右侧渲染当前模块。点击 Add Models 时直接激活 Models 模块。两处 `●●●●●` 均为密文 API Key 输入框（见 [`01-scope-and-research.md`](01-scope-and-research.md) 的 §1 术语「API Key」与 [`02-decisions-and-delivery.md`](02-decisions-and-delivery.md) 的 §3.1 D8）。

### A.分图 3：Models 模块（左表单 / 右列表）

- 左：新增/编辑自定义模型表单，字段对齐 `models.toml`：`id / model_name / api(下拉) / provider / base_url / thinking_format(下拉) / context_window / capabilities(多选)`；外加两个 Key 相关字段：
  - **API Key（密文输入，masked）**：模型的密钥值本身；保存时经 `set_provider_key` 写入 `.env`，供该 provider 复用。
  - `api_key_env`（高级/可选）：显式环境变量名；留空则按 provider 自动推断 `<PROVIDER>_API_KEY`（不是密钥值，只是变量名）。
- 右：已配置列表，两组：
  - Ready（`key_present=true`）：可用，行内 Edit / 删除（内置不可删，可覆盖）。
  - Needs API key（`key_present=false`，多为官方内置预置）：行内一个**密文 API Key 输入框 + Save**，保存即调 `set_provider_key` 写 `.env`，`key_present` 翻转为 true、升入 Ready 组并进入下拉。
- **Key 保护**：两处 Key 输入均 `type=password` 不回显、`autocomplete=off`；保存后仅回传 `key_present`，serve/host 决不把 Key 明文回吐 webview；`.env` 0600、日志脱敏。

### A.分图 4：后端 provider 分派 + endpoint 构造

```text
ModelEntry.api ─▶ registry PROVIDERS (registry.rs)
  "openai"             → OpenAiProvider           …/chat/completions
  "openai-responses"   → OpenAiResponsesProvider  …/responses
  "anthropic-messages" → AnthropicProvider(新)    …/messages
                          headers: x-api-key + anthropic-version

endpoint 构造改为 path-aware（解决 GLM 路径不是 /v1 的问题）:
  base 无路径(仅host) → 追加 /v1/<leaf>   (deepseek / moonshot / anthropic) 向后兼容
  base 带路径         → 追加 /<leaf>      (zhipu: /api/paas/v4 → /api/paas/v4/chat/completions)
```

### A.分图 5：CLI 命令树（外层 + 内层）

```text
tomcat model                         (外层)
  ├─ list                            列出全部(内置+用户)，标 ready / needs-key
  ├─ add  --id --api --provider --base-url [--model-name]
  │        [--api-key-env] [--thinking-format] [--vision --files --tools --reasoning]
  ├─ remove <id>                     删用户模型（内置不可删）
  ├─ key set <provider|ENV_NAME> [--value <k>|交互输入]   写 .env
  ├─ key list                        列各 provider 是否已配 Key
  └─ default <id>                    设 llm.default_model
```
