# 模型管理与 Add Models · 01 术语与竞品调研

> 总览见 [`../model-management-add-models.md`](../model-management-add-models.md)（含定位、阅读顺序与文首导图集）。
> 本文对应 [`ARCHITECTURE_SPEC.md`](../../../../tomcat/docs/openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) 的 **§1 术语统一** 与 **§2 竞品 / 选型对比（调研）**。
> 单一事实源：模型目录以 `tomcat/src/core/llm/catalog.rs` 为准；模型管理写盘逻辑统一收敛到拟新增的 `core/llm/admin.rs`；serve 协议以 `tomcat/src/api/serve/types.rs` 为准。

---

## 1. 术语统一（MUST）

| 术语                     | 语义（大白话）            | 数据载体                                                                     | 行为约束                                                                                                              | 说人话                  |
| ---------------------- | ------------------ | ------------------------------------------------------------------------ | ----------------------------------------------------------------------------------------------------------------- | -------------------- |
| ModelEntry             | 模型的连接元数据           | `[catalog.rs](../../../../tomcat/src/core/llm/catalog.rs)` `ModelEntry` | `id/api/provider/base_url/api_key_env/capabilities/thinking_format/...`；merge 后常驻 `ModelCatalog`                  | 一条模型「怎么连」的说明书。       |
| ModelView              | 面向 UI/CLI 的展示视图    | 新增，serve `list_models` 出参                                                | = ModelEntry 子集 + `source(Builtin|User)` + `key_present(bool)`；**绝不含 Key 明文**                                     | 给界面看的「模型 + 状态」，不含密钥。 |
| source                 | 条目来源               | `ModelView.source`                                                       | `Builtin`=`builtin_models()`；`User`=`models.toml` 新增/覆盖                                                           | 分「官方出厂」还是「你自己加的」。    |
| key_present            | 该模型的 API Key 是否已配  | `ModelView.key_present`                                                  | = `api_key_env` 解析出的变量在 `.env`/环境中**非空**                                                                          | 「有没有钥匙」的开关。          |
| api_key_env            | 凭证环境变量名（**不是密钥值**） | `ModelEntry.api_key_env`                                                 | 缺省时按 provider 推断 `<PROVIDER>_API_KEY`（`[auth.rs](../../../../tomcat/src/core/llm/auth.rs)` `env_name_for_provider`） | 只是「钥匙放哪个抽屉」，不是钥匙本身。  |
| API Key（密钥值）           | 真正的密钥字符串           | `~/.tomcat/assets/.env`（0600）                                            | 密文输入、只入不出；serve/host 不回吐 webview                                                                                  | 真正的钥匙，落 `.env`，永不回显。 |
| 官方内置预置                 | 出厂即在目录的模型          | `builtin_models()`                                                       | 开机即列（多为 NeedsKey），用户填 Key 即用                                                                                      | 出厂自带清单，缺的只是钥匙。       |
| 设置中心                   | 编辑器区全屏 webview     | `SettingsPanel.ts` + `gui/src/settings/*`                                | 左导航模块入口 / 右模块内容；本期只 Models 可点                                                                                     | 一个独立的「设置」标签页。        |
| Models 模块              | 设置中心里的模型页          | `gui/src/settings/*`                                                     | 左新增/编辑表单 / 右 Ready+NeedsKey 列表                                                                                    | 设置里专门管模型的那一页。        |
| model-admin capability | serve 是否支持模型管理命令   | serve `initialize.capabilities`                                          | 缺失则下拉「Add Models」与设置页隐藏（旧 serve 兼容）                                                                               | 后端支持才显示入口，旧版不炸。      |

> 时间点钉死：本文「写盘后」指 `admin.rs` 完成 `models.toml`/`.env` 原子写并 `reload` 之后；「立即可用」指同一 serve 进程内下一次 `set_model`/模型解析即生效（Key 新增场景，见 [`04-operations-validation-history.md`](04-operations-validation-history.md) 的 §9 轮换边界）。

---

## 2. 竞品 / 选型对比（调研）（MUST）

关切：本特性有三处需要横向印证——(a) 设置页 UI 形态、(b) 加模型的下拉入口形态、(c) GLM/Kimi/Anthropic 这类第三方模型「怎么接」。

| 竞品 / 参考                                                                                 | 形态                    | 关键设计                                                                                                                      | 我们借鉴的点                                                | 说人话                          |
| --------------------------------------------------------------------------------------- | --------------------- | ------------------------------------------------------------------------------------------------------------------------- | ----------------------------------------------------- | ---------------------------- |
| VS Code / Cursor「Agent Customizations」设置页（附件3；参考 `/Users/yankeben/workspace/vscode` 源码） | 编辑器区全屏 webview        | 左侧模块导航（Overview/Agents/Skills/…）+ 右侧模块内容区                                                                                 | 设置中心「左导航 / 右内容」骨架，模块可扩展                               | 抄它的「左边选模块、右边配」的两栏布局。         |
| Cursor「Select model」下拉（附件2）                                                             | 侧栏内自定义下拉              | 模型列表 + 分隔符 + 底部「Add Models」动作                                                                                             | 下拉「可用模型 / 分隔符 / Add Models 页脚」结构                      | 抄它的下拉排版：上面选模型，下面加模型。         |
| goose（Block）`zhipu` declarative provider                                                | 声明式 provider + env 覆盖 | `crates/goose/src/providers/declarative/zhipu.json`：默认 `open.bigmodel.cn/api/paas/v4`，`ZHIPU_BASE_URL` 可覆盖 Coding Plan 端点 | GLM base_url 带非 `/v1` 路径的事实 → 印证需 path-aware endpoint | 别的 agent 也踩过「智谱路径不是 /v1」这个坑。 |
| Kimi Open Platform 文档                                                                   | OpenAI 兼容             | `base_url=https://api.moonshot.ai/v1`，`/v1/chat/completions`，`kimi-k2.7-code` 等                                           | Kimi 直接复用现有 `openai` wire，无需新 provider                | Kimi 就是 OpenAI 那套，改个地址就行。    |
| Anthropic Messages API 文档                                                               | 独立 wire               | `POST /v1/messages`，`x-api-key`+`anthropic-version`，`thinking:{type:"adaptive"}`，SSE `content_block_delta`                | 必须新写 `anthropic-messages` provider（非 OpenAI 兼容）       | Claude 不是 OpenAI 协议，得单独写一套。  |

为什么选「serve 命令写盘 + 单一 `admin.rs`」而不是「宿主 TypeScript 直写文件」（3–5 条）：

1. **单一事实源**：`models.toml` 合并、校验、`.env` 0600 权限、Key 脱敏逻辑只写一份 Rust，CLI 与 GUI 复用；TS 直写会出现两套解析/校验，极易漂移（现状 `catalog.rs` 已是唯一 merge 实现）。
2. **CLI 硬需求**：需求本身要 `tomcat model` 命令；写盘逻辑放 Rust，CLI 天然共享，零重复。
3. **一致的运行时生效**：写盘后由持有 `ModelCatalog` 的 Rust 侧直接 `reload`，避免 TS 改文件后 Rust 进程不自知。
4. **安全边界内聚**：Key 只入不出、脱敏、权限位都在 Rust 内闭环，不经过 TS 层多一手暴露面。

为什么设置页用**独立编辑器区 webview**而非侧栏 overlay（已与用户确认）：侧栏窄，两栏（左导航/右内容）会挤；编辑器区全屏与附件3 参考一致，且未来可挂更多模块。
