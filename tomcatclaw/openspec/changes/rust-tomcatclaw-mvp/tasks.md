# Tasks: rust-tomcatclaw-mvp

## MVP 实现步骤（本变更）

### 1. 项目骨架

- [ ] 使用 `cargo new` 创建 tomcatclaw 项目（或放在现有 repo 子目录）。
- [ ] 配置 Cargo.toml：依赖 tokio、axum、reqwest、serde、serde_json、tracing；可选 rustls。
- [ ] 建立 `src/` 下模块：`config`、`channels`、`routing`、`agents`、`outbound`、`events`（可为空壳）。

### 2. 配置

- [ ] 定义最小配置结构（单 agent、单 channel、LLM 端点 + API Key）。
- [ ] 从文件或环境变量加载配置；校验必填项。
- [ ] 预留 bindings / agents 数组等字段，便于后续扩展。

### 3. 渠道（钉钉或飞书二选一）

- [ ] 选定首渠道：钉钉 或 飞书。
- [ ] 实现入站：接收该渠道的 webhook/事件回调（HTTP POST），解析出消息体、发送者、会话 id。
- [ ] 实现出站：调用该渠道的发送 API，将回复内容发回对应会话/群。
- [ ] 将「入站解析结果」映射为内部结构：channel、peer、sessionKey、文本内容。

### 4. 路由与会话

- [ ] 定义内部 session key 格式（与 openclaw 兼容形状）。
- [ ] MVP：单 agent、单会话；从配置或默认值得到 agentId。
- [ ] 预留：根据 channel + peer 查 bindings 得到 agentId（后续实现）。

### 5. Agent 与 LLM

- [ ] 实现 LLM 适配：HTTP 调用兼容 OpenAI 的 API（URL + API Key 可配置），传入消息列表，取回复文本。
- [ ] 单轮对话：入站文本 → 构造 messages → 调用 LLM → 取第一条回复。
- [ ] 无工具、无记忆、无 sessions_send；仅文本入、文本出。

### 6. 事件流与 Outbound

- [ ] 定义「入站消息」为一种事件类型，入队或直接调用执行（MVP 可无队列，单请求单处理）。
- [ ] 执行路径：入站 → 路由 → Agent(LLM) → 得到回复文本。
- [ ] 通过 Outbound 抽象（或直接调用 channel 发送接口）将回复发回渠道。

### 7. 入口与运行

- [ ] 提供 HTTP 服务：至少一个 POST 端点接收渠道 webhook。
- [ ] 配置监听地址与端口；启动时加载配置、注册路由。
- [ ] 文档：如何配置钉钉/飞书机器人、填写 webhook URL、配置 LLM 端点与 Key；本地运行与验证步骤。

### 8. 验收

- [ ] 在钉钉或飞书中创建机器人，配置 webhook 指向本机或公网暴露地址。
- [ ] 发送一条消息，确认收到一条 LLM 生成的回复。
- [ ] 记录 MVP 完成状态，便于后续接 Phase 2。

---

## 后续阶段任务（不在本变更实现）

### Phase 2：多 Agent 与渠道扩展

- [ ] 完整 bindings 配置与解析；channel + peer → agentId。
- [ ] 每 Agent 独立 Sessions、AgentDir、Workspace 目录。
- [ ] SOUL.md / PROMPT.md / USER.md 加载与注入。
- [ ] Outbound 统一接口；注册多个 channel 实现；群级策略（如 requireMention）。
- [ ] 身份展示：name、emoji 写入配置并在发送时体现。

### Phase 2.5：Agent 运行时（Pi 兼容，方案 B）

- [ ] **树形会话**：会话节点带 id/parentId，支持分支、回溯、标签；JSONL 持久化；与 OpenClaw/Pi 会话形状兼容。
- [ ] **四大工具（或子集）**：Read（文件读取）、Write（新建/覆盖）、Edit（基于 diff 的编辑）、Bash（Shell 执行）；通过统一工具接口暴露给 LLM。
- [ ] **多 LLM 封装**：统一封装多厂商 API（OpenAI 兼容、Ollama 等），同会话可切换模型；类型安全 Trait 抽象。
- [ ] **扩展机制（可选）**：支持一种或多种——Wasm（wasmtime/wasmer）热加载、或嵌入 Deno Core 兼容 TS 扩展、或 dylib 动态加载；扩展生命周期 install/uninstall；主程序托管状态，扩展通过 API 读写（getState/setState），热重载保留状态。
- [ ] **上下文工程**：分层加载 AGENTS.md、SYSTEM.md/APPEND_SYSTEM.md、Skills 按需加载；会话接近窗口时可选压缩策略。
- [ ] **与 Pi 协议/行为对齐**：会话格式、工具调用格式、事件流与 OpenClaw 所用 Pi 可互操作或一致，便于 Control UI、多 Agent 玩法复用。

### Phase 3：主动联系与协作

- [ ] Cron：jobs 存储（如 JSON）、调度执行、enqueueSystemEvent / runIsolatedAgentTurn。
- [ ] Webhook：任务完成 POST、webhookToken、SSRF 防护。
- [ ] Heartbeat：间隔配置、requestHeartbeatNow；与事件队列对接。
- [ ] sessions_send 与 agentToAgent 白名单；主 Agent 派单、专家 Agent 执行。

### Phase 4：记忆、沙箱与体积

- [ ] 记忆：向量存储与检索、embedding、MEMORY.md 或等价物。
- [ ] 沙箱与命令执行：白名单、审批、安全边界。
- [ ] Gateway 完整：WS、JSON-RPC Methods、config-reload。
- [ ] 体积优化：strip、依赖裁剪、CI 监控；Android 条件编译与裁剪验证。
