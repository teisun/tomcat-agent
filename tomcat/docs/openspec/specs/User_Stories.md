# tomcat 用户故事
## 故事分级规则
- P0：一期MVP必须实现，核心路径能力，无替代方案
- P1：二期必须实现，核心竞争力能力
- P2：三期及以后实现，核心扩展能力
- P3：长期规划能力，非核心路径

## P0 一期MVP核心用户故事
### Story 1: 宿主引擎初始化与基础配置
**作为用户**，我希望能快速安装、初始化tomcat引擎，完成基础LLM配置，正常启动运行。
**验收标准**：
- [ ] 支持单文件二进制安装，无需额外依赖配置
- [ ] `tomcat init`命令可完成初始化配置，以 model-first 方式选择默认模型，并自动推导匹配的 `provider` / `api_base` / `api_key_env`
- [ ] 首次生成的 `tomcat.config.toml` 默认 `[llm] provider = "openai-responses"`；`provider = "openai"` 仍可手动配置但不再作为 init 默认路径
- [ ] 配置文件可正确加载并生效（敏感信息加密存储 TODO 后续考虑）
- [ ] `tomcat doctor`命令可检测运行环境、WasmEdge依赖、配置合法性，给出修复建议
- [ ] 引擎可正常启动、关闭，无崩溃、无致命错误日志，跨Windows/macOS/Linux三大平台正常运行

### Story 2: 4原语核心能力与安全管控
**作为用户**，我希望Agent能通过4原语安全地操作文件、执行命令，所有操作可控、可追溯。
**验收标准**：
- [ ] 内置 `read` / `write` / `edit` / `bash` 原语 API 规范完整、行为稳定
- [ ] 实现路径授权根机制：`agent_definition_dir` 为默认允许根，`workspace.workspace_roots` / 会话授权用于扩大外部目录访问，`primitive.path_rules` 提供 deny / readonly 规则；`agent_workspace_dir` 只作为当前目录语义来源，不自动授权
- [ ] 实现bash命令管控：`bash_forbidden` 直接拦截，`bash_approval_required` 需用户确认，路径 token 与 `NAME=/path` assignment RHS 都必须进入同一套路径权限预检
- [ ] `search_files` 收到空 `type` / 空 `glob` 时按“未传”处理，不得把空字符串继续透传到 `rg` / fallback，也不得再出现 `rg: unrecognized file type:` 这类占位空串导致的伪错误
- [ ] `bash.cwd` 收到空字符串时按“未传”处理；若路径不存在或不是目录，必须在 `spawn` 前返回可定位错误，文案同时包含解析后路径与原始输入；`$HOME/...` 这类 shell 变量写法只给提示“不自动展开”，不能误伤客观存在的字面目录名
- [ ] write/edit操作前自动备份原文件，支持回滚，操作前显示diff预览与用户确认
- [ ] 所有4原语操作完整记录审计日志，包含操作内容、用户确认状态、执行结果、时间戳
- [ ] 支持全局配置自动确认策略；legacy `path_whitelist` / `bash_whitelist` / `auto_confirm_whitelist` 配置字段已删除
- [ ] `tomcat audit list` 可列出历史审计记录；`tomcat audit show <id>` 查看单条详情；`tomcat audit export <file>` 导出完整日志文件
- [ ] 直接拖拽或粘贴路径回车时按普通聊天发送给 LLM；只有显式 `/path <路径>` 命令进入路径授权菜单，且仅支持单个路径参数
- [ ] `/path` 路径授权命中 deny 或用户选择 cancel 时，本地命令不得发送给 LLM，也不写入 `[drag-cancel]` 合成 note；`/help` 可显示当前本地命令列表
- [ ] deny / readonly `path_rules` 在同一会话内热生效：`[r]`、`[d]` 或 `config_set primitive.path_rules` 写入后，后续 read/write/edit/bash 立即按新规则拦截或降级，不需重启
- [ ] cwd 首次触达授权与 `workspace.workspace_roots` 扩大授权前必须先做 deny 预检；命中 deny 时不得展示“永久允许/本次允许”等扩大授权选项
- [ ] `read` 工具（PR-RA：从 `read_file` 重命名）支持 `offset`/`limit` 分页 + `[tools.read].max_bytes`（默认 25 MiB，metadata 阶段判定）+ **128 KiB 后读预算护栏**（读取/格式化过程中累计最终输出体量，到完整行边界截断并返回 `offset=<next>`；若首个返回行本身超限则结构化报错要求缩窗）+ `cat -n` 行号渲染（默认 `line_numbers=true`）+ `hashline` 行级 `xxh32` 短指纹（默认 `false`，与 `line_numbers` 互斥并优先），重复 read 命中 mtime/size/window 廉价指纹时返回 `FILE_UNCHANGED` stub 短路占用
- [ ] `read` 工具读取**二进制 / 非 UTF-8** 文件返回结构化错误（含首字节十六进制提示），不把乱码注入上下文；命中 PNG/JPEG/GIF/WebP/PDF MIME 时改走 `ReadResult::Image`/`Pdf` 路径，由 wire 翻译层在下一条 user 消息注入 `input_image`/`input_file` parts（≤ `IMAGE_MAX_BYTES` / `FILE_MAX_BYTES`，超限在 metadata 阶段拒绝）
- [ ] LLM 必须把 `agent_workspace_dir` 视为“当前目录 / 这个项目 / 相对路径”的唯一来源；`agent_definition_dir`（`workspace-<agentId>/`）和 `agent_trail_dir`（`agents/<agentId>/`）不得替代用户工作目录
- [ ] 单条大工具结果按 Layer 0 落盘到 `agent_trail_dir/tool-results/{session_id}/` 并在上下文中留下 preview；`agent_definition_dir` 仅是 agent 行为定义工作区，不承载运行态 tool-results

### Story 3: WasmEdge+QuickJS沙箱插件系统
**作为用户**，我希望能加载、运行 Wasm 沙箱插件，插件在隔离环境中运行，不影响宿主系统安全。
**验收标准**：
- [ ] 基于WasmEdge实现插件沙箱运行时，每个插件独立Wasm实例隔离
- [ ] 内置WasmEdge官方QuickJS运行时，支持原生JS/TS插件执行，无需手动编译
- [ ] 实现插件全生命周期管理：加载、初始化、启用、禁用、卸载，资源完全释放
- [ ] ExtensionAPI 契约稳定，符合规范的插件可正常加载运行
- [ ] 插件级权限管控，加载插件时需用户确认授权的权限范围，未授权API无法调用
- [ ] 插件执行错误完全隔离，单个插件崩溃不会影响宿主引擎与其他插件运行
- [ ] `tomcat plugin`系列命令可实现插件的加载、卸载、列表查看、启用/禁用

### Story 4: Node.js 兼容层
**作为用户**，我希望沙箱内插件能正常调用 Node.js API，无需为 tomcat 单独改写法。
**验收标准**：
- [ ] 基于 WasmEdge 原生实现 Node.js 核心兼容层，覆盖沙箱插件高频使用的 fs / path / process / console 等全局对象与内置模块
- [ ] 支持CommonJS模块规范，require/import正常工作，插件内相对路径模块加载正常
- [ ] 实现事件循环机制，基于WASI Preview2异步IO，setTimeout/setInterval/Promise异步行为与Node.js完全对齐
- [ ] 支持http/https模块，网络请求受插件权限管控，可正常调用第三方API
- [ ] 绝大多数纯JS npm包可在插件内正常加载使用，无兼容性问题

### Story 5: 宿主核心API与工具注册
**作为插件开发者**，我希望能通过宿主API注册自定义工具，扩展Agent能力，被对话与其他插件调用。
**验收标准**：
- [ ] 实现完整的工具注册 API，支持 registerTool / unregisterTool / getToolList，符合宿主工具注册规范
- [ ] 注册的工具可在对话中被Agent自动调用，也可被其他插件调用
- [ ] 工具调用经过权限校验、审计日志记录，调用参数与结果完整可追溯
- [ ] 支持工具的描述、输入Schema定义，符合LLM函数调用规范
- [ ] 插件卸载时，自动注销该插件注册的所有工具，无残留

### Story 6: 事件系统完整实现
**作为插件开发者**，我希望能监听宿主核心事件，在关键节点执行自定义逻辑，扩展Agent行为。
**验收标准**：
- [ ] 实现全局事件总线，事件 API 支持 on / emit / off / once 方法
- [ ] 覆盖会话、对话、4原语、插件、工具全生命周期核心内置事件
- [ ] 支持插件监听事件，注册同步/异步回调函数，事件触发时正常执行
- [ ] 单个回调函数执行错误被完整捕获，不影响其他回调与宿主主流程
- [ ] 支持插件发布自定义事件，实现插件间通信
- [ ] 插件卸载时，自动注销该插件注册的所有事件监听，无内存泄漏

### Story 7: LLM统一接入与调用
**作为用户**，我希望能自由配置、切换不同的大模型，插件可通过统一API调用LLM能力。
**验收标准**：
- [ ] 实现统一LLM Provider Trait，兼容所有OpenAI API格式的大模型
- [ ] 支持配置模型的温度、最大Token、上下文窗口等参数，会话级模型配置隔离
- [ ] 实现流式与非流式 LLM 调用 API，插件经 Hostcall 可正常调用
- [ ] 默认 provider 路径使用 `openai-responses`；当 `thinking.show = "summary"` 或 `thinking.show = "full"` 时，CLI 默认链路可稳定展示 reasoning / thinking 摘要
- [ ] CLI 内 `/thinking [minimal|summary|full|toggle]` 可在对话期运行时切换 thinking 显示档位；缺省等价 `toggle`，循环 `summary -> full -> minimal -> summary`；兼容历史 `/thinking on|off`（on→full、off→summary）
- [ ] `PI_CHAT_SHOW_THINKING` 环境变量与配置文件 `[llm.thinking].show` 兼容三档字符串与历史 bool（`true/1/yes/on -> full`、`false/0/no/off/"" -> summary`）；优先级 `PI_CHAT_SHOW_THINKING > config > 代码默认`
- [ ] Responses reasoning 流按 `(item_id, index)` 分桶去重：同一片摘要被网关以 `summary_text.delta + summary_text.done + summary_part.done + output_item.done` 多次重发时，CLI/listener 仅看到单条 `Thinking` 事件，不出现重复字串
- [ ] 支持Token消耗统计与记录，每次对话显示Token消耗
- [ ] API密钥可配置并生效，调用支持限流与指数退避重试（加密存储 TODO 后续考虑）

### Story 8a: 异步 Hostcall 与 JS API 对齐
**作为插件开发者**，我希望在插件中使用 `async/await` 调用宿主 API（如 `await pi.exec("ls")`），耗时操作不阻塞插件执行，与宿主异步 Hostcall 模型一致。
**验收标准**：
- [ ] `pi.exec()`、`pi.createChatCompletion()` 等耗时 API 返回 Promise，插件可用 `await` 消费
- [ ] LLM 调用、命令执行等耗时 Hostcall 不阻塞 Wasm 实例，宿主后台异步处理
- [ ] 返回值格式与 ExtensionAPI 约定一致（`ExecResult: {stdout, stderr, exitCode}`、`CompletionResult: {message, usage}`）
- [ ] `pi.on`/`pi.off`/`pi.emit` 无重复定义 bug，`pi.once` 可用，注册一次后多次 emit 仅触发 1 次
- [ ] 插件内多个并发异步调用（如同时 `await pi.exec()` + `await pi.createChatCompletion()`）可正确运行
- [ ] 异步操作超时后返回清晰错误，插件可通过 `try/catch` 捕获
- [ ] 同步 API（`pi.log`、`pi.registerTool`、`pi.on` 等）行为不变，不受异步改造影响
- [ ] `pi.registerTool` 注册工具后宿主可通过 host_call 感知（registerTool 触发 ≥1 次 host_call）；`pi.unregisterTool` 可正常反注册
- [ ] 以上异步 API 行为均可通过 `tests/wasmedge_e2e_tests.rs` 中 Wasm 真实运行时集成测试验证（E2E-WASM-011/022/023）

### Story 8: CLI工具基础对话与会话管理
**作为用户**，我希望能通过CLI工具与Agent对话，管理会话历史，正常使用插件能力。
**验收标准**：
- [ ] `tomcat chat` 命令启动对话模式，支持自然语言对话，流式响应渲染
- [ ] `tomcat chat --resume` 可恢复上次会话，历史上下文从持久化 JSONL 文件加载并注入 LLM
- [ ] `/model current|list|use <id>` 可查看当前模型、列出 catalog，并把当前会话模型选择持久化到 session；程序重启或 `--resume` 后仍生效，且不覆盖全局 `default_model`
- [ ] 支持多轮对话上下文关联，Agent 可正常调用 4 原语、注册的工具、加载的插件能力；重启后从 JSONL 恢复消息历史，不丢失上下文
- [ ] Agent 可在对话中调用内置 `web_search` / `web_fetch` 工具完成联网检索与单 URL 抓取：搜索结果需结构归一并做域/SSRF 过滤，抓取结果需返回正文或 `tool-results/` 落盘路径；对应场景见 `E2E-CLI-064` / `E2E-CLI-065`
- [x] Skill 系统可在 chat 会话中完成发现 / 披露 / 按名装载闭环：首轮 prompt 注入 `<available_skills>` 元数据，模型可按名调用 `load_skill`；用户可用 `/skill list|reload|use <name> "intent..."` 与外层 `tomcat skill list|reload` 管理技能；`disable-model-invocation` 的技能不出现在 prompt 但仍可被 `/skill use` 显式点名；对应场景见 `E2E-CLI-066` / `E2E-CLI-067` / `E2E-CLI-068`
- [ ] 实现会话管理功能，支持创建、切换、归档、删除、搜索会话，历史持久化不丢失
- [ ] CLI prompt 与实际模式一致：user 端显示 `u[Chat|<model>]>` / `u[Plan:planning|<model>]>` / `u[Plan:executing|<model>]>` / `u[Plan:pending|<model>]>` / `u[Plan:completed|<model>]>`，agent 端显示 `agent.<id>[Plan:planning]>` / `agent.<id>[Plan:executing]>` / `agent.<id>[Plan:pending]>` / `agent.<id>[Plan:completed]>`；普通聊天维持 `agent.<id>>`
- [ ] `/plan build <plan_id/path>` 成功后立即自动进入首个 EXEC 回合，CLI 可见 `u[Plan:executing]> start building <path>`，无需用户再手动补一句触发执行
- [ ] 非 EXEC 状态下 `ask_question` 交互为单选 + 自定义 + `skip` 当前题：非法输入只重试当前题，`c` 与 `c <文本>` 都可录入自定义答案，返回结果显式携带 `skipped: true`
- [ ] 对话中Agent调用4原语/工具时，清晰展示操作内容，等待用户确认后执行
- [ ] 支持Markdown/代码块高亮渲染，快捷键支持（Ctrl+C中断、Ctrl+D退出、↑↓历史导航）
- [ ] `tomcat session` 系列命令（list/new/switch/delete/archive/search）可完整管理会话生命周期
- [ ] Agent 执行工具期间，用户发送新消息可触发 Steering——完成当前工具后跳过剩余工具，注入新指令并重新调用 LLM（中途换方向不需要重新创建会话）
- [ ] Ctrl+C 可触发 Abort——当前工具执行完毕后立即终止 Agent，发布 agent_end(interrupted)
- [ ] Agent 回答完毕后用户继续追加消息（FollowUp），在同一会话上下文中无缝继续，无需重新初始化
- [ ] `tomcat chat --resume` 遇到 transcript 尾部 dangling `assistant.tool_calls` 时，hydrate 会按最后一个 tool_call block 的原顺序补齐所有缺失的 synthetic tool result `[interrupted]`；若中间混入非 `tool` role 则拒绝猜测并保留告警
- [ ] mid-turn `Failed` / 流中断后，本轮已落盘的 `user`、完整 `assistant`、`assistant + tool_calls`、已完成 `tool_result` 仍保留在 transcript；用户下一条输入直接开启下一轮，无需 `/retry`
- [ ] LLM API Rate Limit 或网络超时时，Agent 自动指数退避重试，对用户透明；致命错误（API Key 无效、模型不存在、Parse、RequestTimeout / NonStreamStale 等）给出清晰的阶段化提示并终止
- [ ] 工具执行进度通过事件实时反馈（agent_start/turn_start/tool_execution_start/end/agent_end），CLI 据此渲染执行状态
- [ ] 单条工具结果超过 `[context].layer0_single_result_max_chars`（默认 **50_000** chars，与 [context-management.md §4.4](../../docs/architecture/context-management.md) 一致）时 Layer 0 落盘 + preview，不撑爆单次请求；可观测事件见 `tool_result_truncated` / 压缩相关事件（以代码与 [events.md](../../docs/architecture/plugin-system/events.md) 为准）
- [ ] 长对话 token 超预算时按 [context-management.md](../../docs/architecture/context-management.md) **现行**链路：**Layer 0**（同步：落盘 / compactable 区占位）→ **Layer 1**（异步预热摘要，时机 ⑤ 不阻塞）→ **Layer 2**（Boundary 延迟应用，时机 ②）→ **Layer 3**（仅 API **Context Overflow** 后 `force_drop_oldest_to_target` 兜底）；保护最近若干 turns 与水位线见文档 §4.2；压缩后继续正常对话
- [ ] Session 重载时正确识别 `BranchSummaryEntry`（含 `is_boundary=false` 跳过 / `true` 折叠、`S::E` 锚点，§5.7），恢复 `CompactionSummary` 消息（`MessageKind::CompactionSummary`）/ `Preheat` 状态与运行时一致，不重复摘要

> **Story 8 — 自动化 / 集成索引（上下文与 JSONL）**  
> 逐条对照见 [docs/reports/traceability-story8-context.md](../../docs/reports/traceability-story8-context.md)；E2E 场景编号见 [guides/testing/E2E_SCENARIO_LIBRARY.md](guides/testing/E2E_SCENARIO_LIBRARY.md) 表中 **E2E-CLI-081～091、092、093**（Story 9 小节，覆盖 AgentLoop + 上下文管理）。  
> **Transcript 格式（开发阶段）**：压缩摘要行仅支持 JSONL **`type: branch_summary`**；**不**提供读盘时将历史 `type: compaction` 映射为 `branch_summary`。无法反序列化的行在 `read_entries_tail` 中 **warn + skip**（见 `src/core/session/transcript.rs`）。  
> **§5.7.5.1 陈旧 `CompactionResult`**：单测 `check_after_reply_stale_apply_removes_branch_summary_and_keeps_preheat_idle`（`src/core/compaction/tests.rs`），与 [context-management.md §5.7.5.1](../../docs/architecture/context-management.md) 一致。

## P1 二期核心用户故事
### Story 8b: 长生命周期 VM 与有状态插件支持
**作为插件开发者**，我希望插件的全局变量、事件监听器、定时器能在整个会话期间保持，不因事件触发而重置，与长生命周期 VM 运行模型一致。
**验收标准**：
- [ ] 插件的全局变量跨多次事件调用保持（如 `let counter = 0` 在多次 `tool_call` 事件间累加）
- [ ] `pi.on()` 注册的 handler 只需注册一次，后续事件直接触发，无需每次重新执行插件脚本
- [ ] 周期性定时器在会话期间持续运行（`setInterval` 或等价的 `setTimeout` 链；后者为 wasmedge_quickjs 兼容实现，见 E2E-WASM-033）
- [ ] `session_start` 初始化的数据可在后续 `before_agent_start`、`tool_call` 等事件中读取
- [ ] 会话结束时（`session_shutdown` 或用户退出）VM 正常关闭，资源完全释放
- [ ] 典型有状态插件场景（git-checkpoint、todo、plan-mode、ssh 等）在长生命周期 VM 下可正确运行

### Story 9: 插件自举全闭环
**作为用户**，我希望Agent能根据我的自然语言需求，自主生成、编译、加载插件，无需人工干预。
**验收标准**：
- [ ] Agent 可从自然语言需求中提取插件功能点，生成符合宿主插件规范的 JS/TS 代码
- [ ] 支持运行时动态加载生成的插件代码到WasmEdge沙箱中，无需重启引擎
- [ ] 插件加载/运行失败时，自动提取错误信息，反馈给LLM自动修复代码，形成闭环
- [ ] 插件生成前清晰告知用户功能、权限需求，获得用户确认后方可生成加载
- [ ] 实现插件模板库，内置常用场景插件模板，提升生成成功率
- [ ] CLI新增插件生成相关命令，支持手动触发插件生成、预览、安装

### Story 10: 插件管理与模板库完善
**作为用户**，我希望能方便地管理本地插件，导入导出插件模板，复用社区资产。
**验收标准**：
- [ ] 实现插件本地仓库管理，支持插件的分类、标签、版本管理
- [ ] 支持插件的导入导出，可分享给其他用户使用
- [ ] 实现插件安全扫描门禁，导入插件时自动检测安全风险
- [ ] 支持第三方插件源配置，可在线检索、安装社区插件
- [ ] 完善插件详情查看、配置管理、日志查看能力

## P2 三期及以后核心用户故事
### Story 11: 多Agent体系完整落地
**作为用户**，我希望能创建自定义Agent，每个Agent有独立的人格、权限、插件列表，完全隔离互不干扰。
**验收标准**：
- [ ] 支持创建、编辑、删除、启用/禁用自定义Agent
- [ ] 每个Agent可独立配置人格设定、System Prompt、权限等级、启用的插件列表、LLM配置
- [ ] 每个会话可绑定指定Agent，切换Agent后上下文与配置自动同步
- [ ] 内置核心系统Agent，不可修改删除，负责项目核心流程
- [ ] Agent配置自动持久化，重启后不丢失

### Story 12: 长程记忆系统落地
**作为用户**，我希望每个Agent有独立的记忆空间，对话内容自动记忆，零上下文遗忘，越用越贴合我的需求。
**验收标准**：
- [ ] 每个Agent有完全独立的记忆空间，不同Agent的记忆完全隔离，无串扰
- [ ] 对话全流程自动提取核心信息、用户偏好、专业知识，向量化存入长期记忆库
- [ ] 对话时自动检索相关记忆，注入Prompt上下文，保证历史信息不丢失
- [ ] 支持记忆的手动编辑、分类、标签、删除、导入导出
- [ ] 支持每个Agent独立配置记忆检索规则、自动提取/注入开关

### Story 13: Skills技能系统落地
**作为用户**，我希望能创建、保存、复用标准化的工作流模板，不用每次重复写提示词。
**验收标准**：
- [ ] 支持创建、编辑、执行、删除Skill，每个Skill有完整的工作流定义
- [ ] 实现Skill工作流引擎，支持条件分支、循环执行、工具调用、4原语调用
- [ ] 支持Skill的导入导出、版本管理、Agent专属绑定
- [ ] 用户输入时，自动检索匹配相关的Skill，推荐给用户选择
- [ ] 内置常用Skill模板，覆盖代码开发、数据分析、文档写作等高频场景

## P3 长期规划用户故事
### Story 14: Web前端与Android跨平台适配
**作为用户**，我希望能通过Web界面和Android手机使用tomcat，核心能力与CLI完全对齐，数据可跨端同步。
**验收标准**：
- [ ] 基于Tauri+React实现Web桌面端界面，核心功能与CLI100%对齐
- [ ] Android端APK可正常安装、启动，核心功能与桌面端完全对齐
- [ ] 界面适配移动端竖屏与触控操作，无错乱、无误触
- [ ] 支持桌面端与Android端的数据导出导入，无数据丢失（加密 TODO 后续考虑）
- [ ] 全平台体验一致性优化，操作逻辑统一

### Story 15: 独立应用生成能力
**作为用户**，我希望Agent能根据我的需求，生成完整的独立可执行应用，无需依附主程序运行。
**验收标准**：
- [ ] Agent可根据自然语言需求，拆解技术方案，生成完整的多文件项目代码
- [ ] 在隔离容器环境中完成项目编译、打包，生成对应平台的可执行安装包
- [ ] 支持生成桌面端、CLI工具、前端项目等常见类型的应用
- [ ] 复杂应用可通过多Agent协作并行开发，提升效率与质量
- [ ] 生成的应用完全独立，可直接运行、分发、二次开发，不依赖主程序

### Story 16: 插件市场与生态建设
**作为用户**，我希望能在插件市场中检索、安装、分享优质插件、Agent模板、Skill模板，完善社区生态。
**验收标准**：
- [ ] 实现官方插件市场，支持插件、Agent、Skill的检索、预览、一键安装
- [ ] 实现开发者上传、发布、更新插件的完整流程
- [ ] 实现插件评分、评论、使用量统计，优质内容推荐
- [ ] 完善插件安全扫描、合规审核门禁，保证市场内容安全
- [ ] 实现开发者文档、最佳实践、插件开发模板，降低开发门槛

### 注：以下 Agent Loop 能力延迟到二期
- **工具循环自动检测**：Generic Repeat / Ping Pong / Global Circuit Breaker 三道防线