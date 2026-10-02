# ModelPicker：模型 + Speed + Effort + Context 合并选择器技术方案

> 适用范围：把「选模型」与「选思考深度 / 上下文窗口」合并进一个入口、一套弹框，三处表面（Composer 底栏 / 聊天内计划卡片 / 计划预览工具条）复用同一组件；内置模型声明可选 Context 档位与 Effort 档位，新建中转站同名模型时按 `model_name` 快照复用 `models.toml` 用户配置或内置目录的能力。
> 上位文档：[`model-management-add-models.md`](model-management-add-models.md)（模型管理与 Add Models 的写盘中枢、`admin.rs` 单一事实源）。本文只覆盖「选择与配置」这一层，写盘与目录合并语义沿用上位文档。
> 单一事实源：模型能力以 `tomcat/src/core/llm/builtin_models.toml` + `catalog.rs` 解析结果为准；用户选择以 `~/.tomcat/model-thinking.json`（`ModelPrefsStore`）为准；协议以 `tomcat/src/api/serve/types.rs` 为准，三份产物 `serve.schema.json` / `serve.d.ts` / `wire.d.ts` 由它重生。

**一句话定位**：一个模型该怎么用，分成**能力声明**（`context_window_options` / `supported_reasoning_levels` / `supported_speeds`）与**用户选择**（`reasoning` / `contextWindow` / `speed`，存在 `ModelPrefsStore`）。模型能力由内置目录与用户 `models.toml` 合并；Settings 可维护用户的 Effort / Speed 声明，Context / output 仍只读。点整行切模型，点该行 Edit 侧浮配置框选 Speed / Context / Effort。三处表面共用 `ModelPicker` 与 `buildPickerModels`。

---

## 目录

1. [问题与目标](#1-问题与目标)
2. [架构主线：能力与选择分离](#2-架构主线能力与选择分离)
3. [数据模型：能力层](#3-数据模型能力层)
4. [数据模型：选择层](#4-数据模型选择层)
5. [运行时解析：从选择到预算](#5-运行时解析从选择到预算)
6. [协议层：serve 命令与产物](#6-协议层serve-命令与产物)
7. [UI 层：一个入口，两层弹框](#7-ui-层一个入口两层弹框)
8. [中转站同名模型复用](#8-中转站同名模型复用)
9. [关键不变量与校验](#9-关键不变量与校验)
10. [被否掉的备选与推翻条件](#10-被否掉的备选与推翻条件)
11. [文件职责地图](#11-文件职责地图)
12. [验证矩阵](#12-验证矩阵)

---

## 1. 问题与目标

改造前 Composer 底栏是三个平级下拉：`Mode ▾ | Model ▾ | Effort ▾`。用户想把 `gpt-5.6` 调成 Extra High 要跨两个控件点两次，而这两个选择在用户心里是**一件事**——「我这次要用多强的脑子」。更实际的问题是底栏挤：模型名可用宽度经常只有七十几像素，长名字必然被截断。

根因不是「布局没调好」，而是**信息模型错了**：底栏把「模型」和「档位」当成两个独立维度平铺，但它们实际是「一个模型 + 该模型的一个档位」的**从属关系**。平铺就必然出现「Effort 下拉里的档位属于哪个模型」这种含糊——事实上 Effort 的档位清单确实跟着当前模型变，可 UI 上完全看不出这种依赖。

目标：砍掉 Effort 下拉，把档位收进模型条目内部，让从属关系在 UI 上显式化；同时把背后缺的数据补齐——Context 只有标量、没有档位也没有选择，必须补成和 Effort 完全对称的形状。

```
改造前：三个平级下拉，档位与模型的从属关系不可见
┌──────┬──────────────┬────────┐
│Mode ▾│ gpt-5.6    ▾ │Effort ▾│   ← Effort 属于谁？看不出来
└──────┴──────────────┴────────┘

改造后：两个下拉，档位嵌在模型里
┌──────┬────────────────────────┐
│Mode ▾│ gpt-5.6 Xhigh         ▾ │   ← 一眼看出「这个模型 + 这个档位」
└──────┴────────────────────────┘
         └─ 省下的 62px 全给模型名，长名字不再被截断
```

三处表面（Composer 底栏、聊天内计划卡片、计划预览工具条）复用同一个 `ModelPicker` 组件。

---

## 2. 架构主线：能力与选择分离

这是全篇最重要的一个决定。窗口、推理档和速度档是**模型能力声明**，来自内置目录与用户 `models.toml`。Settings 可编辑用户的推理和速度声明，但不能改变上游真正支持的能力。选择 1M、Max 或 Fast 是另存的**用户偏好**，运行时必须落在声明允许的范围内。

改造前推理档位已经是这么做的（能力 `supported_reasoning_levels` + 选择 `model-thinking.json` + 夹取 `clamp_reasoning_level`），Context 却只有一个标量能力、没有档位也没有选择。本方案把 Context 补成和 Reasoning 完全对称的形状，并把两个「选择」合并进同一个存储。

```
┌── 能力 CAPABILITY ── builtin_models.toml / 用户 models.toml ─────────────┐
│  context_window           = 400000            ← 默认档（必须是 options 的一项）│
│  context_window_options   = [400000, 1000000] ← 可选清单（留空 = 单档）        │
│  supported_reasoning_levels = ["low","medium","high","xhigh"]               │
│  max_output_tokens / description                                             │
└───────────────────────┬─────────────────────────────────────────────────────┘
                        │ ModelCatalog 按 id 合并（用户覆盖内置，见 §8）
                        ▼
┌── 选择 CHOICE ── ~/.tomcat/model-thinking.json（ModelPrefsStore）──────────┐
│  { "gpt-5.6": { "reasoning": "xhigh", "contextWindow": 1000000 } }         │
└───────────────────────┬─────────────────────────────────────────────────────┘
                        │ 夹取：选择必须落在能力集合内（§5、§9）
                        ▼
              EffectiveModelLimits::resolve   +   resolve_thinking_level
                        │
              ┌─────────┴─────────┐
              ▼                   ▼
        上下文预算            请求字段
   input_budget_tokens    reasoning_effort / thinking
```

这条分离线决定了一切下游设计：UI 只暴露能力清单里的选项、store 只存用户的选择、resolver 用夹取后的选择覆盖能力默认值。任何一层越界（UI 让用户选了模型不支持的窗口、store 存了不在 options 里的值、resolver 不夹取直接用）都会让「能力 / 选择」这层边界失效。

---

## 3. 数据模型：能力层

能力层定义在 `ModelEntry`（`tomcat/src/core/llm/catalog.rs:50-75`），是 `builtin_models.toml` 与用户 `models.toml` 共用的结构：

```rust
pub struct ModelEntry {
    pub id: String,
    pub model_name: Option<String>,        // 上游真名；中转站与 id 不同
    pub api: String,
    pub provider: String,
    pub api_key_env: Option<String>,
    pub base_url: Option<String>,
    pub capabilities: Capabilities,
    pub context_window: Option<u32>,       // 默认档
    pub context_window_options: Vec<u32>, // 可选清单；空 = 单档
    pub max_output_tokens: Option<u32>,
    pub description: Option<String>,
    pub thinking_format: Option<String>,
    pub supported_reasoning_levels: Vec<String>,
    pub supported_speeds: Vec<Speed>,      // 只列 fast / ultrafast，Standard 隐含
}
```

### 3.1 `context_window` 与 `context_window_options` 的语义

```
context_window          = 这个模型默认用哪一档（options 非空时必须是其中一项）
context_window_options  = 用户可切换的档位；留空 = 没有替代档（单档）
```

- **留空即单档**：`options` 为空表示这个模型只有一种窗口。配置框仍把
  `context_window` 规范化为唯一的 Context 选项并显示它：用户既能看见当前
  生效窗口，也不会误以为这个模型没有 Context 能力。
- **用户只覆盖 `context_window` 时，`options` 一并重置为空**（`catalog.rs:480-486`）。理由：用户显式写了一个单值，意思就是「我这个中转站只有这一档」，此时继承内置的多档清单会让用户在 UI 上选到一个上游根本不支持的窗口。这条规则让「只写单值」永远不可能产出自相矛盾的状态。
- **用户同时写了两者**：按校验规则要求 `context_window ∈ options`，违反时按 §9 的降级处理。

### 3.2 档位取值规则（默认 = 不涨价的最大档）

每个真实窗 > 400K 的模型提供两档 `[400000, 1000000]`；默认选「最大的、不涨价的那一档」——满档不涨价的默认满档（1M），满档要涨价的默认 400K。真实窗 ≤ 400K 的模型只有一档。

这条规则把 `gpt-5.x` 的逻辑（`400K - 128K output = 272K input`，恰好卡在 OpenAI >272K 输入按全会话 2× 计费的涨价线下沿）推广到所有模型：**默认 = 用户不必为长上下文额外付费的最大窗**。逐个查证结论（来源见 `builtin_models.toml` 每条的注释与 `research-model-specs` 调研）：

```
模型族                              内置默认 Context  最大输出       官方文档值（归一化依据）
──────────────────────────────────────────────────────────────────────────────────────────────
gpt-5.4 / 5.5 / 5.6                 400K            128K          约 1.05M → 目录统一 1M；
                                                                    >272K 输入触发长上下文计费
deepseek-v4-pro / flash / utility   1M              384K          1M / 384K，统一 token 费率
kimi-k3                             1M              128K          1M；1M 档不另涨价
claude-*（1M 那批）                  1M              128K          1M；GA 后取消 >200K 溢价
mimo-v2.5-pro                       1M              128K          1M / 128K
glm-5.2                             1M              128K          PaaS 标准接口即 1M
gpt-5.2                             400K            128K          400K / 128K
kimi-k2.7-code                      256K            按剩余窗      256K；输出是剩余 context，
                                                                    不是固定模型上限
```

`gpt` 真实窗约 1.05M，本期统一取整到 1M（50K 差异是噪声，图各模型整齐）。

### 3.3 档位值口径：总窗口，不是输入预算

档位值 = **总窗口**（含输出预留），不是输入预算。理由：这个数必须与 `models.toml` 里的 `context_window`、与 Settings 里展示的数字**是同一个数**，否则用户在三个地方看到三个数会疯。输入预算（`context_window - output_reserve`）是运行时算出来的派生量，不在 UI 上单独暴露为档位标签。

UI 标签用紧凑写法 `400K` / `1M`（`ModelPicker.tsx:515-521` 的 `formatContextWindow`）。

### 3.4 `max_output_tokens` 是逐模型能力字段

`max_output_tokens` 与 `context_window` / `context_window_options` 并列，是逐模型的 catalog 字段。它**不进 ModelPicker**——ModelPicker 只切 `context_window` 档位，不改输出窗口；输出窗口只在 catalog / Settings 里维护。它对预算的影响见 §5。

---

## 4. 数据模型：选择层

选择层定义在 `ModelPrefs`（`tomcat/src/core/session/model_thinking.rs:15-22`），承载用户对一个模型的两个选择：

```rust
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelPrefs {
    pub reasoning: ThinkingLevel,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u32>,
}
```

落盘格式是干净的对象（`~/.tomcat/model-thinking.json`）：

```json
{ "models": { "gpt-5.6": { "reasoning": "xhigh", "contextWindow": 1000000 } } }
```

### 4.1 不做向后兼容读

开发期，`ModelPrefs` 直接 `#[derive(Deserialize)]` 只认对象形状，不保留裸字符串 `"xhigh"` 的旧读分支，也不保留把 context 编码进假模型 id（`__tomcat_context_window__:gpt-5.4:400000`）的旧写法。开发者本机旧文件一次性迁移到新格式，迁移动作不进产品代码。

### 4.2 损坏文件保留而非清空

JSON 解析失败时，把原文件重命名为 `model-thinking.json.corrupt-<时间戳>` 再建空 store（`model_thinking.rs:136-144`），不再静默清空。这与附件存储的坏 blob 处理一致（`attachments/tests.rs` 的 `.corrupt-` 约定）。丢数据的下限是「改名保留」，不是「静默清零」。

### 4.3 写盘在锁内

`ModelPrefsStore::update`（`model_thinking.rs:106-118`）持锁 → 改内存 → 写盘（`write_file_atomic`，临时文件 + rename）→ 释放锁。写盘的额外代价是微秒级，换掉一整类「锁内 clone 快照、锁外写盘、两个线程互相覆盖」的竞态。

### 4.4 prefs 是必选依赖

`DefaultLlmResolver::new` 直接收 `Arc<ModelPrefsStore>`，没有 `Option` 包装、没有 `with_model_prefs` builder。理由：一个「传了行为 A、不传行为 B」的可选依赖，等于把两套行为的正确性都押在每个调用点记得传上——而 CLI 路径（`session_cmd.rs:170`）曾经就是那个忘了传的调用点，导致 CLI 与 GUI 算出的上下文预算不一致。测试要「无选择」的场景，传一个空 store 即可。

### 4.5 渲染期不伪造已选值

`ModelView::from_entry`（`admin.rs:124-150`）把 `selected_context_window` / `selected_reasoning_level` 一律置 `None`。只有 `list_model_views_with_prefs`（`admin.rs:221-253`）在拿到真实 prefs 时才填真值：

- `selected_reasoning_level`：模型有推理能力时，取显式 prefs 的 `reasoning`，否则取 store 默认值，再 `clamp_reasoning_level` 夹回能力集合。
- `selected_context_window`：取显式 prefs 的 `context_window` 且必须落在 `context_window_options` 内；否则回落 `entry.context_window`（仅当它是单档或属于 options 时）。

这样 UI 永远不会把「能力清单第一项」伪装成「用户已选」。

---

## 5. 运行时解析：从选择到预算

`EffectiveModelLimits::resolve_with_context_window`（`resolver.rs:107-153`）是把能力 + 选择合成运行时唯一解释的地方：

```
1. selected_context_window 只在它落在 options 内时才被采纳（resolver.rs:112-114）
   ── 这是「选择必须夹回能力集合内」的运行时兜底。即便 store 里存了脏值，
      也不会被用成一个模型不支持的窗口。
2. 采纳的档位 → context_window；否则回落 entry.context_window；再否则回落 config.context_window_fallback
3. output_reserve = max( config.output_reserve_tokens(默认 0), model.max_output_tokens )
   ── 逐模型的输出窗口参与预算扣减
4. input_budget_tokens = context_window - output_reserve
   ── 这是压缩水位的分母
5. validate_model_limit_values 兜底：output_reserve 必须 < context_window
```

### 5.1 预算与水位

压缩**水位**是 `usage_ratio = estimated_tokens / input_budget_tokens` 的固定比例（`compaction/apply.rs`、`preheat.rs`），分母是这个**逐模型**算出来的 `input_budget_tokens`，不是原始 `context_window`。所以 `max_output_tokens` 填错会直接算错预算、进而算偏水位——这就是 §3.4 要把它逐个补准的原因。

### 5.2 会话中途改档位 = 下一回合按新预算走

`ContextState::apply_limits`（`types.rs:263-269`）只改两个阈值字段，注释明确写了 "never restores compacted history"：

```rust
self.context_budget_tokens = limits.input_budget_tokens;
self.context_budget_chars = limits.input_budget_chars;
```

所以改了档位，下一回合预算就变。往小改（1M → 400K）时历史可能瞬间超预算，此时**下一回合自然走超预算压缩路径**，不拦截切换、不弹确认框、不回溯已压缩的历史。`SetContextWindow` 命令在档位生效到当前会话模型时主动调一次 `apply_limits`（`serve/commands.rs:1144-1162`），让水位立即更新而不是等下一回合。

---

## 6. 协议层：serve 命令与产物

### 6.1 新增命令

`ServeCommand::SetContextWindow { id, session_id, model, context_window }`（`serve/types.rs:330`），与既有 `SetThinkingLevel` 对称。处理逻辑（`serve/commands.rs:1099-1170`）：

1. `lookup_explicit(&model)` 找不到模型 → 返回错误。
2. `context_window` 不在 `entry.context_window_options` 内 → 返回 `invalid_context_window` 错误并回显合法档位。
3. `model_prefs.set_context_window(&model, Some(context_window))` 持久化。
4. 若该模型正是当前会话模型，`resolve_call` 后 `apply_limits`，立即发水位快照。
5. 回显 `{ sessionId, model, contextWindow }`。

`SetThinkingLevel` 同构：`parse_serve_thinking_level` 校验 → `model_prefs.set_reasoning` 持久化 → 回显。两者都走 `model_prefs` 这一个 store，落盘语义统一。

### 6.2 webview intent

新增 `setContextWindow` intent，与既有 `setModel` / `setThinkingLevel` 并列（名字不改，只新增）。host 侧 `provider.ts` 转发到 serve：

- `setContextWindow`（`provider.ts:1870-1901`）：`sendSetContextWindow` → 失败 appendMessage 报错 → 成功 `refreshModels()` + `refreshSessionState` + `postState`。
- `setThinkingLevel`（`provider.ts:1837-1869`）：对称，成功时也 `refreshModels()`（计划侧配置框要看到新档位）。

### 6.3 三份协议产物

协议一改必须重新生成 `serve.schema.json` / `serve.d.ts` / `wire.d.ts`，由 `serve_schema_fixture` 与 `check:wire` 两个守卫测试断言不过期。前端要用的 `contextWindowOptions` / `selectedContextWindow` / `selectedReasoningLevel` 等字段就来自 `wire.d.ts`，产物没生成就写前端等于对着不存在的类型编程。

### 6.4 E2E 契约

`e2e-harness` 直接发 `setThinkingLevel` / `setModel` intent，这两个名字**不许改**。新弹框里的 Effort 选项沿用 `thinking-level-option` testId、容器沿用 `thinking-level-dropdown`，让既有 E2E 原地可用。还有一个隐形 `legacyThinkingTriggerTestId` 按钮保住旧 E2E 的 `thinking-level-select` 触发器契约——它丑，但 E2E 改一次成本远高于留一个隐形按钮。

---

## 7. UI 层：一个入口，两层弹框

### 7.1 三处表面共用一个组件

```
                    ┌─ modelLabel.ts（纯函数，无 React）─┐
                    │  formatModelLabel(id, reasoning)    │
                    │  modelLabelParts → { id, reasoning } │
                    └───────────────┬────────────────────┘
                                    │ 三处共用
        ┌───────────────────────────┼───────────────────────────┐
        ▼                           ▼                           ▼
   Composer 底栏            聊天内计划卡片              计划预览工具条
        │                           │                           │
        │  buildPickerModels（共享纯函数，叠加会话档位）            │
        └──────────── 同一个 ModelPicker 组件 ─────────────────────┘
```

- `modelLabel.ts`：纯 TS、不依赖 React。聊天 App 与计划预览 App 是两个独立入口（`main.tsx` 与 `plan/main.tsx`），纯函数才能被两边无副作用引入。档位缺失时不拼接尾空格，避免 `gpt-5.6 ` 这种脏串。
- `buildPickerModels.ts`：把目录元数据与活动会话的选择叠加，**不改目录快照**。活动模型用会话档位，非活动模型用目录值，不在目录里的历史模型仍出现在列表里。Composer 与计划侧共用这一份，消除了「计划侧不叠加会话档位」的旧分叉。
- `ModelPicker.tsx`：唯一的选择组件。`PlanBuildModelSelect` 这个纯转发壳已删，两个调用点（`PlanFileCard`、`PlanActionStrip`）直接用 `ModelPicker` + `buildPickerModels`。

### 7.2 一行的右侧槽位（互斥）

同一个右侧槽位按状态切换三种内容，优先级 `Edit > ✓ > 空`：

```
┌──────────────────────────────────────────────┬──────────┐
│ gpt-5.6          Xhigh                        │    ✓     │  未 hover + 已选中
│ gpt-5.6          Xhigh   ← hover 灰底          │  Edit    │  hover（Edit 顶掉 ✓）
│ claude-opus-4-8  Max                          │          │  未 hover + 未选中
└──────────────────────────────────────────────┴──────────┘
```

- ✓ 用 `codicon-check`（沿用 `ToolRow.tsx` 的既有约定），不写死 `✓` 字符。
- Edit 按钮在 hover / focus / 配置打开时可见（`editVisible`，`ModelPicker.tsx:310-313`），否则透明且 `pointer-events: none`，让 ✓ 显出来。
- **Edit 永远可达**：所有模型行在 hover / focus 时都有 Edit，哪怕 Context
  与 Effort 都没有可切换项。这样行的交互结构稳定；有多少选项只决定二级框
  显示什么，不能决定入口是否突然消失。

### 7.3 配置框：独立浮层，侧向定位

点 Edit 后，配置框作为**独立 portal** 渲染到 `document.body`（`ModelPicker.tsx:411-425` 的 `createPortal`），脱离一级弹框的 DOM 层级。理由：一级弹框的列表容器是 `overflow-y: auto`，隐式带 `overflow-x: auto`，会把侧向绝对定位的子元素裁掉。Portal 让配置框不再是一级弹框的 DOM 后代，从根上避开裁切。

定位由 `useLayoutEffect`（`ModelPicker.tsx:122-175`）算：

```
anchorRect = Edit 所在行的 getBoundingClientRect()
configRect = 配置框的 getBoundingClientRect()
rightSpace = innerWidth - anchorRect.right
leftSpace  = anchorRect.left
placement  = rightSpace 够 或 rightSpace >= leftSpace ? "right" : "left"
rawLeft    = placement=="right" ? anchorRect.right + gap
                              : anchorRect.left - gap - configWidth
centeredTop = anchorRect.top + anchorRect.height/2 - configHeight/2
left = clamp(rawLeft, 8, innerWidth - configWidth - 8)
top  = clamp(centeredTop, 8, innerHeight - configHeight - 8)
```

- **配置框中间与 Edit 来源行垂直对齐**（`centeredTop`）。
- **只浮左 / 右，没有 bottom 腿**。两侧都不够时由 clamp 兜底贴边，不落到底部。
- `position: fixed`（`styles.css:845`），不受父级 transform / overflow 影响。
- 开框时量一次，`resize` 重算；transcript `scroll` 时直接关闭（fixed portal 不会跟着滚动，关比飘着好）。

### 7.4 两层弹框的关闭逻辑

关闭逻辑从「一刀全关」改为「按层级逐层关」（`ModelPicker.tsx:184-220`）：

```
              点击落点 / 按键
                    │
     ┌──────────────┼──────────────┬────────────────┐
     ▼              ▼              ▼                ▼
  落在配置框内     落在一级内      落在两者外        Escape
     │              │              │                │
   什么都不关     只关配置框      全关          有配置→只关配置
                                                 无配置→关一级
```

关键：portal 在 `rootRef` 之外，所以 `mousedown` 判定时**先查配置框 ref**（`configPopoverRef.current?.contains(target)`），命中就 return，让配置框内的选项能收到后续 `click`。这是「点配置框选项却把一级关掉、选项 onClick 不触发」那个 P0 bug 的正解。

### 7.5 配置框内容

```
┌─ 二级 ──────────────┐
│ Context             │ ← onSelectContextWindow 存在且有默认窗时渲染
│   400K         ✓    │
│   1M                │
│ Effort              │ ← supportedReasoningLevels 非空才渲染
│   Low               │
│   Xhigh      ✓      │
└─────────────────────┘
```

- **无模型名标题**：目标模型由行高亮和 Edit 的 `aria-label` 明确，重复标题是浪费。
- **术语统一为 Effort**：与 `/effort` 命令、旧 `fieldLabel` 一致；`Reasoning` 这个分叉名已废弃。
- **单档模型仍显示唯一 Context 项**：`options` 为空时以
  `context_window` 作为唯一项；它不是“伪造第二档”，而是让用户看见唯一合法
  档位与当前选择。
- **testId 一律不动**：`thinking-level-dropdown` / `thinking-level-option` / `context-window-option` 保持原样，E2E 契约不断。

### 7.6 触发器与底栏布局

- 触发器 `className` 复用 `tc-topbar__trigger tc-topbar__trigger--compact tc-model-picker-trigger`。**扁平（无边框、透明底）是 `.tc-model-picker-trigger` 自身契约**，三表面（Composer / PlanFileCard / PlanActionStrip）共用；不要依赖 Composer 祖先选择器，也不要把扁平规则写进基类 `.tc-topbar__trigger`（SessionBar 会话下拉仍要有边框）。Composer 底栏里 Mode 下拉继续靠 `.tc-composer__bar .tc-field--dropdown .tc-topbar__trigger` 覆盖保持扁平。
- **弹出方向（`placement`）按表面固定，不做视口自动 flip**：Composer 默认 `above`；PlanFileCard 显式 `above`（与 Composer 同，避免盖住下方 transcript）；PlanActionStrip（计划预览顶栏）显式 `below`。对应 class：`tc-model-picker-dropdown--above` / `--below`；chevron 随方向（up / down）。
- `.tc-field--model` 改 `flex: 0 1 auto` + `max-width`，按内容宽但封顶，长名字 ellipsis，不再吃掉全部剩余空间。
- `.tc-model-picker-dropdown` 固定 `width: min(320px, calc(100vw - 24px))`，删掉 `max-content`，宽度与内容解耦，搜索过滤不再缩。
- 搜索框直接 `aria-label="Search models"`，删掉未定义的 `sr-only` span。

### 7.7 搜索：紧凑化 + 分词 AND

`modelSearch.ts` 的 `filterModels`：候选串与查询都紧凑化（小写 + 删 `- . / _ 空格`），查询按空格分词，每个词都要是候选紧凑串的子串（AND，与顺序无关）。一次紧凑化同时解决 `56`（丢了点）、`gpt56`（跨分隔符）、`opus48`、`5.6 terra`、`terra 5.6`（乱序）全部用例。匹配范围只含 `id` 与 `modelName`，**把 `description` 从搜索串里去掉**——描述是长句，参与匹配会让一个词命中全部模型。不做打分排序，保持目录原序。

### 7.8 计划创建卡片，而非所有计划工具卡片

`create_plan` 是计划文件从无到有的主流程节点，因此
`createPlanFileCardFromTool()` 把它渲染成带 ModelPicker 的 `PlanFileCard`。`update_plan`
只是对既有文件的普通变更，保留为 `ToolRow`，不伪装成第二张计划卡。

`TranscriptView` 的两条渲染路径（独立 timeline 项 / 助手回复段内的 action-tool
段）都会先尝试 `createPlanFileCardFromTool()`，只有 `create_plan` 会命中
（`PlanFileCard.tsx:84-94`）；`PlanFileCard` 的档位回调是**必填**，任一真实卡片
漏传即编译报错。

---

### 7.9 Speed：第三个兄弟，不是 Effort 的别名

```text
models.toml：supported_speeds = ["fast", "ultrafast"]  （只列加速档）
                              │
ModelPicker：Speed → Standard / Fast / Ultrafast
             ────────────────────────────────
             Context / Effort（含声明的 Max）
                              │ setSpeed(modelId, speed)
宿主：sendSetSpeed → serve set_speed → ModelPrefsStore.set_speed
                              │ 按 catalog ID 保存，成功后刷新目录
下次运行：resolve_speed → AgentLoopConfig.speed → ChatRequest.speed
                              │ 只在 OpenAI 适配器转换
Standard → service_tier="default"  Fast → "priority"  Ultrafast → "ultrafast"
```

- Standard 默认、隐含；空声明隐藏整个 Speed 区且不发速度字段。曾存的付费档被撤掉后回落 Standard，不自动改成另一个收费档。
- 对声明了速度控制的模型显式发 `default`，防止省略字段时使用上游项目的默认 Fast。选中 Fast 只是请求意图，不证明上游没有降档。
- Fast / Ultrafast 复用 `ConfigSection` / `ConfigOption`、分隔线与两层关闭机制，title 提示“更快输出、费用更高、需上游权限”。不新增 pending 锁或 busy 禁用。
- Speed 与 Effort 独立；`max` 仍是现有字符串档位。Ultrafast 逐模型开放，不按版本继承资格；Ultra 多代理不属于本期，也不是合法 effort。
- ModelPrefsStore 持锁复制 → 原子写盘 → 成功才提交内存，失败时 Effort / Context / Speed 全部保持原值。
- Settings 使用 nullable 原始 `reasoningLevelsText` / `speedsText`：null 保留未声明/继承，空串保存为空数组表示显式清空；输入只改文字，生成保存 payload 时用共享 `parseCommaList` 按 `/[,，]/` 拆分、trim、去空、去重。取消、切换模型与重开重新初始化草稿。
- `ModelView` / `ModelEntryInput` 增 `supportedSpeeds`；目录视图增 `selectedSpeed`；聊天与计划预览两条宿主链都透传。Rust 是协议源，三份 wire 产物自动重生。
- 新测试分层：Rust mock HTTP 覆盖流式/非流式三档 + Max；GUI 覆盖三表面、逐字符逗号与保存往返；既有 Add Models E2E 覆盖真实 VS Code 渲染与重启偏好，并采集普通/窄宽 PNG、ARIA、console。真实网关档位由单独的限量 smoke 报告核实。

---

## 8. 中转站同名模型复用

Speed 能力也纳入同名模型创建期快照：初始化 `speedsText`、保存为 `supported_speeds`，后续只编辑描述也不能丢档位。

### 8.1 问题：匹配维度选错了

中转站模型的 id 是派生的——`relayDerive.ts` 把它拼成 `chatanywhere/gpt-5.6` 这种 `slug/modelName` 形式。所以按 id 匹配内置模型**永远不可能**命中内置的 `gpt-5.6`。真正能对上的维度是 **`model_name`**：中转站转发的就是同一个上游模型，`model_name` 必然相同。

```
用户输入 base_url = api.chatanywhere.tech
        model name = gpt-5.6
                │
                ▼  relayDerive 派生
        id = "chatanywhere/gpt-5.6"  ──按 id 找内置──→ ✗ 永远找不到
        model_name = "gpt-5.6"       ──按 name 找──→ ✓ 命中内置 gpt-5.6
                                                      └→ context 档位 / 最大输出 / 推理档位
```

### 8.2 机制：创建期快照，不是活继承

`findReusableModelByName`（`modelBuiltinMatch.ts`）按 `model_name` 找创建期的能力模板：先找用户 `models.toml`，再找内置目录。命中多个内置条目时保留 catalog 书写顺序的首条（`deepseek-v4-flash` 同时对应 `deepseek-v4-flash` 与 `utility-flash` 两个 id，靠这条 tie-break 锁定唯一 winner）。

`SettingsApp.tsx` 在「新建 relay」时（`formMode === "create" && dialogKind === "relay"`）把该模板的 API、能力、thinking format 与 reasoning levels 放进表单草稿；除 Context Window 和 Maximum output tokens 外，用户可继续编辑。Context（默认档）、Context options 和 output limit 是 catalog 的只读快照，保存时仍经 `upsert_model` 明确落进 `models.toml`。

relay 的 `provider` 与 `api_key_env` 不从同名上游模型搬运：它们是 endpoint 的连接身份，先精确复用已有用户 relay 的同一 endpoint，未命中才按 URL host 启发式推导。

按名模板的九个字段是 `model_name`、`api`、`capabilities`、
`context_window`、`context_window_options`、`max_output_tokens`、
`description`、`thinking_format`、`supported_reasoning_levels`。其中模型名由当前
输入保留；Context 与 output 是只读展示；其余草稿字段可以在保存前改写。
`id`、`base_url`、`provider`、`api_key_env` 属于这条新 relay 的身份或连接配置，
不作为同名上游模型的复制来源。

```
新接入模型的最简体验：只填 端点 + 模型名 + api key
  ┌─ Settings relay 表单（主字段）─────────────┐
  │ Model name: gpt-5.6                          │ ← 唯一必填的语义字段
  │ Base URL:  https://api.chatanywhere.tech    │
  │ API key:   [新建或复用]                      │
  └─────────────────────────────────────────────┘
        │ findReusableModelByName("gpt-5.6")
        ▼  命中用户配置或内置 gpt-5.6
  context_window         = 400000       ← 只读快照
  context_window_options = [400000, 1000000] ← 只读快照
  max_output_tokens      = 128000       ← 只读快照
  supported_reasoning_levels = [...]    ← 草稿初值
  capabilities / thinking_format         ← 草稿初值，可改
        │ upsert_model 落盘
        ▼
  models.toml 多一条 chatanywhere/gpt-5.6，带档位
```

这是**创建期快照**：内置档位日后变更不回灌到已建 relay。理由：relay 是用户拥有的配置，创建那一刻就该自洽；活继承会让 relay 的行为随内置目录变化而漂移，反而不可预测。

### 8.3 Settings 的 Context 只读

Settings 里 Context Window **不再手填**（旧 override 输入已删），改为只读展示该模型的默认档（`SettingsApp.tsx:1784-1793`）：

```
Context window
  400,000
  Choose a different Context tier from ModelPicker;
  Settings always inherits the catalog default.
```

理由：档位是「从内置离散档里选」，不是「填任意数」。留手填框既和 ModelPicker 的档位选择打架，又能填出模型不支持的数。`SetContextWindow` 只由 ModelPicker 触发。`maxOutputTokens` 同样是 catalog 的只读能力：Settings 显示最终值，但保存时总写入明确值，不再提供一个会制造未知输出预算的手填入口。

### 8.4 复用顺序与边界

- 新建 relay 先按 `model_name` 查复用源：同名 `models.toml` 用户模型优先，其次内置目录；这是创建时快照，不是运行时活继承。模型名没命中时显式写入全局默认 `context_window=400K` 与 `max_output_tokens=128K`。
- endpoint 复用有两层：先将输入的 `base_url` 规范化后与已有**用户 relay** 精确匹配，复用其真实 `provider` / `api_key_env`；没有精确命中才运行 `deriveRelayFields` 的 host 品牌启发式。两层都只是初值，用户仍可修改。
- CLI / 手改 `models.toml` 仍按 id 合并（`merge_user_model` 只认 id），不按名活继承——GUI 主路径只在创建时作快照。

---

## 9. 关键不变量与校验

### 9.1 一份校验函数，两条路径共用

`validate_context_window_options`（`catalog.rs:112-144`）被 catalog 加载与 `ModelEntryInput::into_model_entry`（`admin.rs:177-182`）两条路径共用，不允许出现「catalog 一套规则、upsert 另一套规则」的第二份真理：

```
档位升序去重  →  每档过 validate_model_limit_values（>0 且 max_output <= context_window）
              →  options 非空时 context_window 必须 ∈ options
```

### 9.2 不对称的失败策略（故意的）

```
用户 models.toml 档位先归一化（升序 + 去重）；归一化后仍非法
（某档 <= 0 / context_window ∉ options / 输出预留不合法）
   └─→ 丢弃这份 options，退化为单档模型，warn 记明模型 id 与原因
       ── serve 永远起得来（catalog.rs:519-526，degrade_invalid_context_options = true）

内置 builtin_models.toml 非法
   └─→ 硬失败（builtin_seed_entries_result 返回 Err）
       ── 我们自己的 bug，早炸早发现
```

「context_window 必须落在 options 里」是一条用户根本不知道存在的新规则。老配置违反时让整个工具打不开且用户不知为何，是不可接受的——所以用户文件降级，内置文件硬失败。这条不对称是故意的。

### 9.3 运行时夹取

即便 store 里存了脏值（不在 options 内），`resolve_with_context_window`（`resolver.rs:112-114`）也会丢弃它，回落 `entry.context_window`。`list_model_views_with_prefs`（`admin.rs:241-249`）在渲染期同样过滤。两层夹取保证「选择」永远不可能越界「能力」。

### 9.4 不变量清单

| 不变量 | 守护点 | 测试 |
|---|---|---|
| `context_window ∈ options`（options 非空时） | `validate_context_window_options` | catalog / upsert 单测 |
| 用户只覆盖 `context_window` 时 options 清空 | `merge_user_model:480-486` | catalog 合并单测 |
| 用户非法档位降级不硬失败 | `merge_user_model:519-526` | catalog 加载单测 |
| 内置非法档位硬失败 | `builtin_seed_entries_result` | 内嵌 toml 全量校验 |
| 选择不在 options 内被夹取 | `resolve_with_context_window` | resolver 单测 |
| 选中档位真正改变 `input_budget_tokens` | `resolve_with_context_window` | `gpt-5.6` 选 1M 后预算从 272K 升到 872K |
| CLI 与 GUI 预算一致 | prefs 必选依赖 | `session_cmd` 与 `ChatContext` 路径算出的 limits 相等 |
| 并发写不丢 | `update` 锁内写盘 | 两线程改不同模型，落盘两个改动都在 |
| 损坏文件保留 | `preserve_corrupt_store` | 非法 JSON → `.corrupt-*` 存在且新 store 为空 |
| 默认档 = 不涨价的最大档 | `builtin_models.toml` | `gpt-5.6` 默认 400K、`claude-opus-4-8`/`deepseek-v4-pro`/`kimi-k3` 默认 1M |

### 9.5 `ask_question`：单一身份与诚实占位

`ask_question` 也是 ModelPicker 三处表面共享的会话状态，不能把它当成
“临时弹窗”。断线或重启时，历史中的 `[pending]` 只证明“这里有一题”，不携带
能安全回包的 live route。

```
durable transcript                    frontend-only live route
assistant tool_call id = toolCallId ──┐
tool result = [pending]               │
                                      ▼
timeline approval identity = toolCallId
                                      │
                         ┌────────────┴────────────┐
history rebuild          │ live=false: 恢复中，占位 │  不可点
control_request arrives  │ live=true: 可回答的卡片  │  可点
                         └─────────────────────────┘
```

不变量：

- `toolCallId` 是 approval 在历史重建、去重、React key 与 live 合并中的唯一身份；
  `requestId` 只用于本次 `control_response` 路由，绝不能拿来去重。
- `live=false` 的卡片没有按钮，也不进入粘性回答区；一次真正
  `control_request` 只能把它升级为 `live=true`，历史刷新不能降级或覆盖 live
  的 route。
- serve 在 restart 后等 `initialize` 完成再重臂。否则控制帧可能在 webview
  安装接收器前丢失，留下永远无法回答的“恢复中”卡片。

先例核查不是按印象选方案：

- Continue 的
  `continue/gui/src/util/toolCallState.ts:addToolCallDeltaToState` 与
  `toolCallState.test.ts` 都以 `toolCallId` 合并流式工具状态；
- Codex 的
  `codex-rs/tui/src/chatwidget/tests/exec_flow.rs` 明确断言 approval 以 modal
  呈现、不会伪装成历史 cell；
- VS Code 的
  `src/vs/workbench/services/agentHost/test/common/agentHostResourceService.test.ts`
  覆盖 pending request 按 host 归属、在 host 清理后只保留仍有效的请求。

这支持“稳定工具身份 + 明确 live 状态”的分层，而不是给 `[pending]` 伪造
`requestId`。反例与推翻条件：若协议未来把可回放、可验证的 response route
持久化为 transcript 的一部分，且能证明其跨 host 重连仍可安全提交，则
`live=false` 占位可以升级为可答卡；在此之前不能猜测 route。

---

## 10. 被否掉的备选与推翻条件

### 10.1 方案二：运行时查 `/models` 接口

被否。实测四类 provider 里只有 Anthropic 的 `/models` 返回 `max_input_tokens`，中转站（主力场景）全军覆没。6 个同级仓库没有一个把它当唯一来源。Anthropic 的能力记为将来的可选增强，本期不做——只覆盖 1/4 的 provider 却要引入网络依赖、缓存、失败降级三套新代码，不划算。**推翻条件**：若中转站普遍开始返回结构化档位，可重提。

### 10.2 Codex 的双标量形状

Codex 用 `context_window`（默认）+ `max_context_window`（天花板）两个标量，用户可填 500000 这种中间值并 clamp 到上限。本期不采用，理由：Tomcat 的 `context_window_options: Vec<u32>` 已落进三份协议产物，改形状要重生三份 + 改 serve + 改前端，换来的用户可见收益是零（真实档位只有两个离散值，中间值没意义）；离散档位在 UI 上是两个可点选项，标量 + clamp 需要数字输入框，与目标形态不符。**推翻条件**：若出现「用户想把窗口卡在任意数值以控制成本」的真实诉求，或三家以上厂商有连续可调窗口，应改双标量并把 Settings 手填框升级为主入口。

### 10.3 cc-fork-01 / Cline 的模型别名 + beta 头

cc-fork-01 用 `sonnet[1m]` 模型别名 + beta 头表达长上下文，Cline 用 `:1m` id 后缀。两者把档位编码进模型标识，适合「档位需要改变请求参数」的场景。本期不采用——Anthropic 2026-03-13 已把 1M GA 并取消 >200K 溢价，无需 beta 头；`glm-5.2` 查证下来 PaaS 标准接口即 1M，也无需 `[1m]` 后缀。**推翻条件**：若某模型服务端将来开始要 beta/后缀才放行大窗，那一个模型才退化为「单档 + 子功能待办」，届时可参考 cc-fork-01 的 `check1mAccess` + 后缀联动。

### 10.4 迁移到新文件 `model-prefs.json`

被否。多出一个「双文件并存 + 谁是权威」的窗口期，且 `deny_unknown_fields` + 自动重置会让旧代码撞上新文件静默清空用户偏好。就地扩值（文件名不变、值从字符串升级为对象、宽松解析）风险更低。**推翻条件**：无，这是开发期的定死决定。

### 10.5 `none` 推理档位

不做。Tomcat 已有 `off` 表达「不推理」，再引入 `none` 只是多一个语义近似的枚举值。`ThinkingLevel` 枚举不动。

---

## 11. 文件职责地图

### 11.1 Rust 后端

| 文件 | 职责 |
|---|---|
| `tomcat/src/core/llm/builtin_models.toml` | 内置模型能力单源；每条带来源 URL 与查证日期注释 |
| `tomcat/src/core/llm/catalog.rs` | `ModelEntry` / `validate_context_window_options` / `merge_user_model`（合并与降级） |
| `tomcat/src/core/llm/admin.rs` | `ModelView` / `ModelEntryInput` / `list_model_views_with_prefs`（带 prefs 的视图） |
| `tomcat/src/core/llm/resolver.rs` | `EffectiveModelLimits::resolve_with_context_window`（选择 → 预算） |
| `tomcat/src/core/session/model_thinking.rs` | `ModelPrefs` / `ModelPrefsStore`（选择层存储，锁内写，损坏保留） |
| `tomcat/src/api/serve/types.rs` | `ServeCommand::SetContextWindow` / `SetThinkingLevel` 协议定义 |
| `tomcat/src/api/serve/commands.rs` | 两个命令的处理：校验 → 持久化 → `apply_limits` → 回显 |
| `tomcat/src/api/chat/context.rs` | `resolve_thinking_level`（与 resolver 对称） |

### 11.2 前端 GUI

| 文件 | 职责 |
|---|---|
| `gui/src/components/ModelPicker.tsx` | 唯一的选择组件；一级弹框 + portal 配置框 + 侧向定位 + 两层关闭 |
| `gui/src/components/modelLabel.ts` | 纯函数：`formatModelLabel` / `modelLabelParts`（id + reasoning 拼接） |
| `gui/src/components/modelSearch.ts` | 纯函数：`filterModels`（紧凑化 + 分词 AND） |
| `gui/src/components/buildPickerModels.ts` | 纯函数：目录元数据 + 会话档位叠加，Composer 与计划侧共用 |
| `gui/src/components/Composer.tsx` | 接入 `ModelPicker`，删独立 Effort 下拉 |
| `gui/src/components/PlanFileCard.tsx` | 计划卡片直接渲染 `ModelPicker`，回调必填 |
| `gui/src/components/PlanActionStrip.tsx` | 计划预览工具条接入同一 `ModelPicker` |
| `gui/src/components/TranscriptView.tsx` | 两条渲染路径都直接渲染 `PlanFileCard`，不搭 `ToolRow` 便车 |
| `gui/src/settings/modelBuiltinMatch.ts` | `findReusableModelByName`（先用户 `models.toml`、后内置目录） |
| `gui/src/settings/relayDerive.ts` | `findConfiguredRelayByBaseUrl`（endpoint 精确复用）与 host 启发式 |
| `gui/src/settings/SettingsApp.tsx` | relay 新建按名快照、endpoint 复用优先、Context / output 只读展示 |
| `gui/src/WebviewErrorBoundary.tsx` | 顶层 ErrorBoundary + `window.onerror` / `unhandledrejection` 兜底 |
| `gui/src/styles.css` | 两层弹框 CSS；配置框 `position: fixed`；触发器复用 `tc-topbar__trigger` |

### 11.3 协议产物（由 `serve/types.rs` 重生）

| 文件 | 守卫 |
|---|---|
| `tomcat/tests/fixtures/serve/serve.schema.json` | `serve_schema_fixture` |
| `tomcat/tests/fixtures/serve/serve.d.ts` | `serve_schema_fixture` |
| `tomcat-vscode-ext/src/serveClient/wire.d.ts` | `npm run check:wire` |

---

## 12. 验证矩阵

按失败类型分层，每一层只覆盖它独有的失败模式，同一件事不在三层各测一遍。

### 12.1 Rust 单元测试（覆盖「算错」）

| 用例 | 断言要点 |
|---|---|
| 档位升序去重 | `[1000000, 400000]` 与 `[400000, 400000, 1000000]` 都归一为 `[400000, 1000000]` |
| 只覆盖单值时 options 置空 | 用户写 `context_window=200000` 不写 options → options 为空 |
| 用户非法档位降级 | `context_window ∉ options` → 模型仍可用 + warn，`load` 返回 `Ok` |
| 内置非法档位硬失败 | `builtin_seed_entries_result` 对非法内置数据返回 `Err` |
| 内嵌 toml 全量校验 | 每个声明了 options 的内置模型，`context_window ∈ options` |
| 默认档取值 | `gpt-5.6` 默认 400K；`claude-opus-4-8`/`deepseek-v4-pro`/`kimi-k3` 默认 1M |
| 选择夹取 | prefs 存了不在 options 内的值 → 回落默认 `context_window` |
| 选中档位生效 | `gpt-5.6` 选 1M 后 `input_budget_tokens` 从 272K 升到 872K |
| CLI/GUI 预算一致 | 同模型同档位，`session_cmd` 与 `ChatContext` 路径算出的 limits 相等 |
| 并发写不丢 | 两线程改不同模型，落盘两个改动都在 |
| 损坏文件保留 | 非法 JSON → `.corrupt-*` 存在且新 store 为空 |

### 12.2 前端单元测试（覆盖「渲染错 / 交互错」）

| 用例 | 断言要点 |
|---|---|
| ✓ 只出现在选中项 | 选中行有 `codicon-check`，其它行没有 |
| hover 时 Edit 顶掉 ✓ | hover 选中行 → Edit 可见且 ✓ 隐藏 |
| 单档模型显示唯一 Context 项 | options 为空 → 以 `context_window` 渲染唯一合法档位 |
| 配置框独立于一级 | 点 Edit 后配置框 portal 到 body，一级裁不到它 |
| 配置框垂直居中对齐行 | `centeredTop` 与 Edit 来源行对齐 |
| 侧向 right/left | 右侧够 → `is-side-right`，仅左侧够 → `is-side-left` |
| 配置框内点击不关一级 | `mousedown` 落在配置框内 → 一级仍在 DOM |
| 二级优先关闭 | Escape → 先关配置框，再关一级 |
| 组合标签 | 已选 xhigh → 触发器与行文本含模型名与 `Xhigh` |
| 搜索过滤 | 输入 `opus48` 后只剩 `claude-opus-4-8` |
| 默认打勾不同 | `gpt-5.6` 勾 400K、`claude-opus-4-8` 勾 1M |
| ErrorBoundary | 子组件抛错 → 显示兜底而非白屏 |

### 12.3 集成测试（覆盖「没存住 / 没传对」）

- `SetContextWindow` 写入后 `list_models` 的 `selectedContextWindow` 反映新值。
- 非法档位被 serve 拒绝并返回明确错误文案。
- `upsert_model` 带 `context_window_options` 落盘后能被重新读出。
- relay `gpt-5.6` 只填名/URL/key → 落盘 `options=[400K,1M]` 默认 400K；relay `opus-5` → 默认 1M。

### 12.4 E2E（覆盖「整条链断了」）

一条路走完：开弹框 → 点 Edit → 改 Context 与 Effort → 关框看组合标签更新 → `restartServe()` → 标签仍是新值。沿用 `model-switch-reverify` 的结构，不为每个字段单独跑。

### 12.5 视觉验收

重建 VSIX（禁用 `--skip-build`，新鲜度门禁把关产物不旧于源码）后，以右侧够、仅左侧够、两侧都不够三种视口断言配置框不被裁切、不落底、不越出视口。`WebviewErrorBoundary` 兜住渲染期崩溃，不再纯空白。

---

## 附：与两份计划的关系

本方案是两份计划的最终落地形态：

- **第一份计划**（`plan_cursor_tomcat_composer_hover_edit_edit_c_08f458de`）定下了架构主线（能力 / 选择分离）、六个语义决定、共享组件与协议通路。本文 §2–§7 的骨架来自它。
- **第二份计划**（`modelpicker_整改计划_53a8b76a`）是交付后的二次整改：删信息卡、保留 Edit、配置框独立 portal、Context 档位真正通电（21 个内置模型补 `[400K, 1M]`）、偏好落盘改干净对象、prefs 改必选依赖、计划卡片从 `ToolRow` 上移、前端类型门禁、ErrorBoundary 兜底。本文 §3.2、§4、§7.3–7.4、§7.8、§9、§12.5 记录的是这些整改后的最终状态。

两份计划里被否掉的备选与推翻条件统一收在 §10，作为「什么条件下这个决策应当被推翻」的存档。

