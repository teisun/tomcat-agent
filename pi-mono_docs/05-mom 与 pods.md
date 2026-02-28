# pi-mom 与 pi-pods

pi-mom 是 Slack Bot，将 @mention 与 DM 委托给 pi coding agent，在受控工作区内自管理工具与技能；pi-pods 是 GPU Pod 上 vLLM 部署与管理 CLI，提供 OpenAI 兼容 API 与交互式 agent 测试。

---

## 1. pi-mom（@mariozechner/pi-mom）

### 1.1 定位与特性

- **Slack 集成**：通过 Socket Mode 连接，订阅 app_mention、message.channels、message.groups、message.im；在频道或 DM 中 @mention mom 或直接发 DM 触发回复。
- **委托给 pi**：底层使用 pi coding agent 的 Agent 能力（读/写/编辑、bash、工具执行），mom 不实现自己的 LLM 循环，而是把 Slack 消息转为 prompt，交给同一套 agent 逻辑执行。
- **自管理**：在沙箱内可安装依赖（apk/apt/npm 等）、配置凭证、创建并维护自定义 CLI 工具（skills）；用户无需预先装好环境。
- **Docker 沙箱**：推荐 `--sandbox=docker:<container-name>`，仅能访问挂载的 data 目录与容器内环境，保护宿主机。
- **工作区与持久化**：每个频道/DM 对应一个子目录，含 log.jsonl（完整历史）、context.jsonl（送入 LLM 的上下文）、MEMORY.md、attachments/、scratch/、skills/；支持 compaction 与对 log.jsonl 的 grep 以扩展“记忆”。

### 1.2 CLI 与环境变量

- **用法**：`mom [options] <working-directory>`；工作目录即 data 目录。
- **选项**：`--sandbox=host`（不推荐）或 `--sandbox=docker:<name>`。
- **环境变量**：`MOM_SLACK_APP_TOKEN`（xapp-...）、`MOM_SLACK_BOT_TOKEN`（xoxb-...）；LLM 鉴权可用 `ANTHROPIC_API_KEY` 或通过 pi 的 `/login` 后把 `~/.pi/agent/auth.json` 链到 `~/.pi/mom/auth.json`。

### 1.3 数据目录结构

- **data/**：根目录；**MEMORY.md**（全局记忆）、**settings.json**（compaction、retry 等）、**skills/**（全局技能）。
- **data/<channel_id>/**：每个频道一个目录；**MEMORY.md**（频道记忆）、**log.jsonl**（全部消息，事实源）、**context.jsonl**（每次回复前从 log 同步、送入 LLM）、**attachments/**、**scratch/**、**skills/**（频道专属技能）。

### 1.4 流程简述

- 消息到达 → 写入该频道的 log.jsonl；若有附件则存到 attachments/。
- 用户 @mention 或 DM mom → 将未读消息从 log.jsonl 同步到 context.jsonl；加载全局与频道 MEMORY.md；调用 agent 响应（可读附件、执行 bash、读写文件、使用技能、attach 回 Slack）；回复写入 log.jsonl，详细 tool 结果等留在 context.jsonl 供后续上下文使用。
- 当 context 超长时进行 compaction（保留近期完整，旧消息摘要）；更早历史通过 grep log.jsonl 检索。

### 1.5 工具与技能

- **内置工具**：bash、read、write、edit、attach（回传文件到 Slack）。
- **Skills**：SKILL.md（frontmatter：name、description）+ 脚本/程序，放在 data/skills/ 或 data/<channel>/skills/；agent 可见技能名与描述，需要时读取 SKILL.md 并按说明调用脚本。用户可让 mom 创建技能，或从外部仓库（如 pi-skills）克隆到 workspace。

### 1.6 文档入口

- **packages/mom/docs/**：slack-bot-minimal-guide.md（Slack 配置）、sandbox.md（Docker vs host）、artifacts-server.md、events.md 等。

---

## 2. pi-pods（@mariozechner/pi-pods）

### 2.1 定位与特性

- **vLLM 部署**：在远程 Ubuntu GPU Pod 上安装/配置 vLLM，支持多种预定义模型（Qwen、GPT-OSS、GLM 等）与自定义 `--vllm` 参数；自动配置 tool calling 等以支持 agent 工作流。
- **Pod 管理**：`pi pods setup <name> "<ssh>"` 注册 Pod，可选 `--mount`（如 NFS）、`--models-path`、`--vllm release|nightly|gpt-oss`；`pi pods` 列出、`pi pods active <name>` 切换、`pi pods remove <name>` 移除。
- **模型管理**：`pi start <model> [--name <name>]` 启动模型（可 `--memory`、`--context`、`--gpus`、`--pod`、`--vllm` 透传）；`pi stop [<name>]`、`pi list`、`pi logs <name>`。
- **OpenAI 兼容 API**：每个模型暴露独立端点（如 :8001/v1），可设 `OPENAI_BASE_URL`、`OPENAI_API_KEY`（或 PI_API_KEY）供任意客户端使用。
- **Agent 测试**：`pi agent <name> "message"` 单次请求；`pi agent <name> -i` 交互式对话；agent 带 read、list、bash、glob、rg 等工具便于在 Pod 上测试 agent 能力。

### 2.2 支持的主机与存储

- **DataCrunch**：推荐；NFS 共享存储，模型一次下载多 Pod 复用。
- **RunPod**：网络卷持久化，不能多 Pod 同时挂同一卷。
- **其他**：Vast.ai、Prime Intellect、AWS EC2（配 EFS）等，只要 Ubuntu + NVIDIA + SSH 即可；`--mount` 与 `--models-path` 配合使用。

### 2.3 预定义模型示例

- **Qwen**：Qwen2.5-Coder-32B-Instruct、Qwen3-Coder-30B、Qwen3-Coder-480B（多卡）等。
- **GPT-OSS**：需 setup 时 `--vllm gpt-oss`；如 openai/gpt-oss-20b、gpt-oss-120b。
- **GLM**：zai-org/GLM-4.5（多 GPU、thinking）、GLM-4.5-Air 等。
- 自定义模型通过 `pi start <model> --name xxx --vllm --tensor-parallel-size 4 ...` 传入 vLLM 参数。

### 2.4 多 GPU 与 API 集成

- 多模型时 pi 自动分配不同 GPU；预定义模型可用 `--gpus` 指定卡数；大模型用 `--vllm --tensor-parallel-size N` 等。
- 启动后可通过 `OPENAI_BASE_URL` + `OPENAI_API_KEY` 用 curl、pi-coding-agent 或任意 OpenAI 兼容客户端访问；pi agent 子命令用于在 Pod 上快速验证。

### 2.5 文档与脚本

- **packages/pods/docs/**：qwen3-coder.md、gpt-oss.md、implementation-plan.md、models.md 等。
- 根命令为 `pi`（npm 包名 @mariozechner/pi），安装后提供 `pi pods`、`pi start`、`pi agent` 等；与 pi-coding-agent 的 `pi` 不同包，若同时全局安装需区分二进制名或路径。

---

## 3. 关键文件路径（参考）

| 包 | 路径/说明 |
|----|-----------|
| pi-mom | packages/mom/：Slack 连接、消息路由、工作区与 log/context 同步、调用 pi agent、Docker 沙箱执行、MEMORY.md 与 skills 加载；docs/ 为上述文档 |
| pi-pods | packages/pods/：pods setup/start/stop/list、vLLM 配置与启动、模型预定义、agent 子命令与 OpenAI 兼容端点；docs/ 为模型与方案说明 |
