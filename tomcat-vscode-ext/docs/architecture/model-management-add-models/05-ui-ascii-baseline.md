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

### A.分图 2：设置中心（编辑器区标签页，列表优先）

```text
┌ Tomcat Settings ────────────────────────────────────────────────┐
│ ┌──────────────┬──────────────────────────────────────────────┐ │
│ │ ▸ Models   ● │  Models                     [ + Add Model ]  │ │
│ │   Sessions ○ │  Manage built-in and custom models...        │ │
│ │   Tools    ○ │  ──────────────────────────────────────────  │ │
│ │              │  Ready                                        │ │
│ │  ○=未实现占位 │  ● gpt-5.4                         (i) Edit │ │
│ │              │  ● glm-5.2                         (i) Edit │ │
│ │              │  ──────────────────────────────────────────  │ │
│ │              │  Needs API key                               │ │
│ │              │  ○ claude-opus-4-8                (i) Edit │ │
│ │              │    [ Save ANTHROPIC_API_KEY ]      [Save]  │ │
│ └──────────────┴──────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
```

左导航是「模块入口」（后续配置可继续挂这里；本期只 Models 可点，其余灰置占位）。右侧先展示 Ready / Needs API key 列表，`Configured Models` 总标题被去掉，只保留分区标题。行内的 `● / ○` 负责轻量表达 ready 状态，`(i)` 为 hover/focus 才展开的字段提示卡，里面用字段名解释 `Source / API / Provider / API Key Env / Base URL` 等信息。点击 `+ Add Model` 或行内 `Edit` 再打开表单弹窗。

### A.分图 3：Models 模块（扁平列表 + info 提示 + 弹窗表单）

- 页面右侧先展示已配置列表，两组，使用扁平列表行而不是厚卡片：
  - Ready（`key_present=true`）：可用，行内 Edit / 删除（内置不可删，可覆盖）。
  - Needs API key（`key_present=false`，多为官方内置预置）：行内一个**密文 API Key 输入框 + Save**，保存即调 `set_provider_key` 写 `.env`，`key_present` 翻转为 true、升入 Ready 组并进入下拉。
- 行内的圆形 info 按钮 hover/focus 后，显示带字段名的提示卡：`Source / API / Provider / API Key Env / Base URL / Thinking / Context Window / Upstream Model`；不再把 `user · openai · deepseek` 这种裸值直接印在列表上。
- 新增/编辑整条模型通过弹窗完成，字段对齐 `models.toml`：`id / model_name / api(下拉) / provider / base_url / thinking_format(下拉) / context_window / capabilities(多选)`；外加两个 Key 相关字段：
  - **API Key（密文输入，masked）**：模型的密钥值本身；保存时经 `set_provider_key` 写入 `.env`，供该 provider 复用。
  - `api_key_env`（高级/可选）：显式环境变量名；留空则按 provider 自动推断 `<PROVIDER>_API_KEY`（不是密钥值，只是变量名）。
- **Key 保护**：两处 Key 输入均 `type=password` 不回显、`autocomplete=off`；保存后仅回传 `key_present`，serve/host 决不把 Key 明文回吐 webview；`.env` 0600、日志脱敏。

```text
                         ┌ Add Model ──────────────────────── x ┐
                         │ Model ID        [                ]   │
                         │ Model Name      [                ]   │
                         │ API ▾           Thinking ▾           │
                         │ Provider        [                ]   │
                         │ API Key Env     [                ]   │
                         │ Base URL        [ https://...    ]   │
                         │ Context Window  [      ] API Key ●●  │
                         │ caps  □vision □files □tools ...      │
                         │                    [Cancel][Save]    │
                         └───────────────────────────────────────┘
```

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
