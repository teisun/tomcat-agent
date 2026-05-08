本文为 [Architecture](../Architecture.md) 中「2. 宿主核心能力层」的详细设计，总览见主文档。

## 2. 宿主核心能力层

项目的可信核心引擎，所有业务逻辑的底层支撑，仅在宿主层运行，不向插件开放直接访问权限。
#### 2.1 会话管理模块

负责会话全生命周期管理、对话上下文组装、消息持久化与会话关联追溯；支持会话级插件、权限、LLM 配置隔离。设计面向多 Agent、多 channel。

- **存储约束**：会话内容不使用 SQLite，仅使用 **pi-mono 相容 JSONL transcript**；索引与路由由 **sessions.json**（元数据 store）提供。
- **两层**：元数据 store（sessions.json，`sessionKey -> SessionEntry`）+ 对话 transcript（**pi-mono 相容** JSONL，与 pi-mono 上游格式对齐）。
- **约定**：列表与「当前会话」由 sessions.json 提供；transcript 内容以 JSONL 为准，sessions.json 为元数据与路由的权威。

#### Transcript 的存储与读取约定

- **禁止全量加载**：禁止一次性将整份 transcript 文件读入内存（禁止整文件 `from_str`）；列表与「当前会话」由 sessions.json 提供，transcript 仅按需读取。
- **流式读取**：使用 `io::BufReader` 等逐行/流式读取 JSONL；SessionHeader 等首行解析采用流式反序列化（如 `StreamDeserializer`），避免单行过大撑爆内存。
- **最近 N 条**：上下文组装（如供给 LLM 的对话历史）仅保留「最近 N 条」消息，N 可配置（MVP 可用固定值）；不因会话变长而无界增长内存。
- **零拷贝解析**：在生命周期允许的前提下，对 **sessions.json、config.toml、单行 JSONL** 的解析优先使用 `serde_json::from_slice` + 借用（`&'a str` 等），减少分配；跨 await 或长期持有的数据不强制零拷贝。

会话路径、sessionKey/sessionId 约定及 SessionEntry、transcript 格式等见 [会话存储数据结构设计](session-storage.md)。存储根目录与多 agent 目录约定见 [工作目录与数据布局](work-dir-and-data-layout.md)。

#### 2.2 LLM接入模块
基于适配器模式实现统一LLM Provider Trait，兼容所有OpenAI格式大模型，支持流式响应、限流重试、Token统计、会话级模型配置，是插件调用LLM能力的唯一可信入口。

多厂商 / OpenAI Completions 与 Responses 边界、**`LlmProvider` 与 `OpenAiProvider` 当前能力、配置与演进约束** 的冻结说明见 [**多 LLM / OpenAI 对接技术方案**](llm-multiprovider-integration.md)；与 pi-mono、hermes、openclaw、pi_agent_rust 的完整横向对照见 [`docs/reports/multi-agent-openai-api-integration.md`](../../../docs/reports/multi-agent-openai-api-integration.md)。

#### 2.3 4原语执行引擎
宿主可信核心，完全对齐pi-mono的4原语规范，是插件访问系统资源的唯一通道，所有操作必须经过权限校验、用户确认、审计日志记录。
- **Read原语**：文件读取、目录列表、元数据获取，路径白名单校验，大文件分块读取
- **Write原语**：文件写入、目录创建、路径删除，操作前备份、用户二次确认、权限校验
- **Edit原语**：基于diff的精确行编辑、内容替换，编辑前diff预览、原子化操作、失败自动回滚
- **Bash原语**：shell命令执行，分级管控（白名单/审批/禁止）、实时流式输出、资源限制、超时控制、完整审计

#### 2.4 工具注册中心
参考pi-agent-rust设计，实现全局工具注册与管理，宿主内置工具、插件注册的自定义工具统一管理，支持工具的注册/注销、权限校验、调用统计，是插件扩展能力的核心入口。

#### 2.5 插件生命周期管理
负责插件的加载、初始化、启动、停止、卸载全流程管理，每个插件对应独立的WasmEdge实例，完全隔离，执行完成后完全释放资源，无内存泄漏。

#### 2.6 权限管控模块
插件级细粒度权限管控，默认最小权限原则，支持4原语权限、网络权限、LLM调用权限、工具访问权限的精细化配置，所有跨宿主调用必须经过权限校验。
