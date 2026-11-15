# Multica 接入 Tomcat：公司二开迁移说明

```text
Multica daemon / task workspace
            │
            ▼
     Tomcat Backend
            │  ACP v1 / stdio
            ▼
   tomcat acp → 共享 tomcat serve
                         │
                  Tomcat 模型与代理配置
```

这是一份**只针对 Multica** 的独立迁移包。它包含 Tomcat backend、Multica 运行时登记、执行目录/技能接线、数据库迁移、测试和无密钥配置示例；不修改公司仓库，也不依赖其它宿主资料。

## 1. 交付内容

- `new-files/server/pkg/agent/tomcat.go`：Tomcat backend 完整实现；`tomcat_models.go`：只读模型发现 adapter。
- `new-files/server/migrations/`：运行时类型白名单的 up/down 示例；编号必须按公司迁移 ledger 调整。
- `existing-files.patch`：agent factory、daemon 探测、执行环境、模型和版本登记的增量修改。
- `tests/`：backend、daemon、执行环境、迁移和受控 ACP 集成测试。
- `examples/runtime-profile.example.json`：无密钥 runtime profile 请求体。
- `examples/tomcat.config.fragment.toml`、`examples/models.fragment.toml`：Tomcat 执行账号配置片段。
- `FILES.md`：来源变更路径的逐项清单（区分原接入、历史目录修复和本轮 adapter 同步）。
- `manifest.json`：来源提交和 SHA-256。
- `LICENSE`、`NOTICE`：来源 Multica 许可证和归属文件副本。

来源：Multica 原基线 `06835b8f50606ca3f8bebbb5d07ac8901f831e69` → 原接入 `e12a45e59d8bfddf2aba192f163899a9eb290a3b`。上一轮 adapter 来源为 Multica `5e36721c1329372d272e8c4b72f4ada2e31115a7`；本轮推理强度来自该基线上的未提交工作树，配套 Tomcat 为 `bfea42a7625fee14c1a74e83e2ff471bd8209fb3` 基线加本轮工作树。`manifest.json.host.reasoning_effort` 记录逐文件哈希，不把未提交功能冒称为已有提交。

**保留原迁移基线，不升级整个宿主。** 此前目录修复来自 Multica `3c7b4cde72ab4c8389dcec933e2573855b04cb7e` 的已审工作树；此前保留目录实现，更新 `tomcat.go`、启动策略/取消回归，以及旧基线所需的 `Session.CleanupRequired`、daemon handoff、取消回报和工作树保留接线。文件与补丁的精确内容由 `manifest.json` 的 SHA-256 标识，不能把混合来源包说成某个完整宿主提交。

本轮不引入新版 Pi 能力、桌面代码、实时 usage、调度或 native CLI 停机确认协议；在原有修复之上，仅补充 `tomcat.go`、`tomcat_models.go` 和 `thinking.go` 的 Tomcat 推理强度接线。公司分支若已具备等价清理契约，应复用现有实现，不重复添加。**不能只替换 `tomcat.go`**：旧 daemon 若把取消/超时直接视为清理成功，仍可能提前清理被后台进程使用的目录。

**验收状态：本功能、受影响流程和原版本适配包已验收通过。**Multica 全仓门禁另有与本功能无关的失败（部分已由原 HEAD 对照复现，其余未逐项归因），原始结果、未测范围和归档校验见 `VALIDATION.md`；本包不代表全仓或公司部署验收。

## 2. 合并前必须核对

公司二开版本需要具备或等价实现：

- `Backend.Execute`、`ExecOptions`（特别是 `Cwd`、`Model`、`ThinkingLevel`、`ResumeSessionID`、`CustomArgs`、`McpConfig`）；
- `Config`、`hermesClient`、ACP RPC pending/tool 表、通知门控、usage 和 prompt completion 回调；
- 进程树所有权、stderr tail、deliverable tracker、MCP capability 过滤和 resume 错误分类；
- daemon 的 runtime discovery、agent factory、local directory/worktree、`AGENTS.md` 与 `.agents/skills` sidecar 清理；
- `runtime_profile.protocol_family` 的迁移及前端 runtime type 列表。

公司代码可能已经改过 Hermes、daemon、迁移或 provider 白名单。补丁冲突时做语义合并，不覆盖公司 provider，不把已有取消/工作树清理逻辑替换成 Tomcat 专用版本。

## 3. 合并顺序

### 3.1 新增 backend 和迁移文件

把 `new-files/` 内容按原路径放到 Multica 仓库根目录：

```text
本包/new-files/server/pkg/agent/tomcat.go
→ 公司Multica/server/pkg/agent/tomcat.go
```

如果目标已有同名文件，停止并人工比较。迁移 SQL 不能直接照抄文件名：先检查公司 `server/migrations/` 已使用的编号。

### 3.2 应用生产补丁

```sh
git apply --check /absolute/path/multica-tomcat-acp-integration/existing-files.patch
git apply /absolute/path/multica-tomcat-acp-integration/existing-files.patch
```

补丁覆盖的生产接线：

- `packages/core/types/agent.ts` 增加 `tomcat` 类型；
- daemon 探测与配置增加 `MULTICA_TOMCAT_PATH`、`MULTICA_TOMCAT_MODEL`；
- execenv 把 `AGENTS.md` 和 `.agents/skills` 接入 Tomcat 运行目录；
- agent factory、launch 参数过滤、模型/版本登记；
- `thinking.go` 的动态目录能力登记：Tomcat 档位由本地目录提供，任务执行前由 ACP 确认；不改其他 Agent 或通用校验算法。
- `Session.CleanupRequired` 与 daemon 结果交接：未收到清理结果或结果为 `cleanup_unconfirmed` 时，不得自动当作取消成功；
- `cleanup_unconfirmed` 的 task failure reason、取消确认回报，以及未确认时保留 sidecar/worktree，不自动提交或删除。
- 迁移之外的 runtime profile API 仍复用原有流程。

### 3.3 调整迁移编号和 provider 白名单

当前示例使用 `584_runtime_profile_add_tomcat.*.sql`，但公司仓库可能已经使用 584。若编号冲突：

1. 选择公司迁移序列中的下一个唯一编号；
2. 同时重命名 up/down 文件；
3. 把公司现有全部 `protocol_family` 值加上 `tomcat`，不能照抄示例列表；
4. 修改 `tomcat_migration_test.go` 中引用的前置迁移和文件名；
5. 在隔离数据库上测试升级、回退和再次升级；
6. 不对生产库盲跑 down。Multica 的常规 migration down 会按项目规则处理已应用迁移，不是“只撤销 Tomcat 这一项”的安全开关。

迁移 down 的语义是恢复写入白名单，不删除历史行；公司如果已有 Tomcat 行或其它 provider，必须根据实际前态设计回退。

## 4. Runtime profile、Agent 和工作目录

创建 runtime profile 的最小 API 请求体：

```json
{
  "display_name": "Team Tomcat",
  "runtime_type": "tomcat",
  "command_name": "tomcat",
  "fixed_args": [],
  "enabled": true
}
```

不要把 `tomcat acp` 放进 `command_name` 或 `fixed_args`。backend 实际启动形态是：

```text
<resolved command> <filtered fixed_args> acp <filtered agent custom_args>
```

`acp` 由 backend 追加；`acp`、`serve`、`--help`、`--version` 等重复/探测参数会被过滤。runtime profile 不负责保存 cwd、model 或 env。

Agent 绑定和执行参数分别负责：

- `agent.model`：可选 Tomcat 模型 ID；空值使用 Tomcat 自己的默认模型；
- `agent.thinking_level`：下次新建或续接任务的推理强度；空值是 Default，重新继承 Tomcat 默认，不是关闭推理；
- `PUT /api/agents/{id}/env` 的 `custom_env`：只放允许的任务环境变量，不放 `HOME`、`PATH`、`TMP` 或 `MULTICA_*`；
- task 的 `local_directory`：决定 cwd，支持 `in_place` 或 `worktree`；
- `mcp_config`：由已有 ACP MCP 流程构造并按能力过滤；
- `ResumeSessionID`：恢复时使用标准 ACP `session/load`，不是 Hermes 的 `session/resume`。

cwd 必须是执行 daemon 上的现有绝对目录，backend 同时用于 ACP `session/new/load` 和子进程工作目录。不要让共享 Tomcat serve 的进程 cwd 充当任务 cwd。

## 5. Tomcat 执行账号配置

在真正运行 Multica daemon 的系统账号/服务环境中准备 Tomcat，而不是只在开发者交互 shell 中准备：

```sh
tomcat init
tomcat model list
tomcat model key set company
tomcat model default company-model
tomcat serve start
tomcat serve status
```

`tomcat acp --help` 必须包含 `--skills-dir`。模型片段见 `examples/models.fragment.toml`，实际地址、协议、模型名和 Key 槽必须按公司网关替换。Key 不写入仓库、runtime profile、任务 prompt 或日志。

Tomcat 正式共享后台使用保存的 `llm.proxy` 或系统代理；Multica agent 的 `HTTP_PROXY` 等环境变量不会被转换成共享后台的模型代理配置。任务 bash/stdin MCP 环境仍按 Multica 原规则运行。

`AGENTS.md` 和 `.agents/skills` 由 Multica 既有 execenv 写入/清理。不要把 runtime brief 拼进 prompt，也不要为了测试给 agent custom env 设置 HOME；daemon 的 HOME、Tomcat 数据目录和模型凭据应在服务账号/部署层安排。

### 5.1 发现模型与错误反馈

```text
创建页/显式刷新 → 原 Query/daemon → runtime executable + prefix + serve models
                                  → 保存目录/凭据 → 原 ListModelsPayload
Tomcat adapter → 仅保留有凭据的原始ID/provider → 原菜单 → agent.model → ACP
失败 → adapter error → 原 daemon failed/error → 创建页原有失败原因/手填提示
```

在 daemon 的实际账号与目录下执行 `tomcat serve models`，而不是开发者 shell 中的另一份 `tomcat`。该命令不用已运行服务，不建会话、不调用模型/MCP；保留配置校验可能初始化常规temp目录的既有边界。文本 `tomcat model list` 的shell视角不变。目录包含 `models/defaultModelId/utilityModelId` 及可选warnings；adapter 消费 ID/provider/keyPresent，以及 `supportedReasoningLevels` 和 `defaultModelId`，不把端点、Key 槽或偏好传到服务端。

- 保存/删除凭据或修改目录后点刷新；adapter不加缓存，不要求重启daemon。
- 目录只证明本地配置及凭据存在，不证明网关、余额或网络可用。
- 零可用模型、坏配置/JSON、15秒超时、旧命令不支持都报告错误，不伪造空成功或假模型。
- 创建页复用现有错误标题、原因、刷新、手填ID和默认路径；不新增toast。详情页首次失败可能空态、刷新失败可能保留旧列表而无原因，保持原状。
- 默认仍按 agent.model → daemon默认 → Tomcat默认；不把目录第一项强行写成选择。旧CLI发现会报错，但不提高现有ACP最低版本、也不禁止已有手填/default任务。
- 使用统一入口版本的 Tomcat 时，默认入口是执行账号的 `~/.local/bin/tomcat[.exe]`；VS Code 包内文件只作安装/更新素材，不再维护 standalone/bundled 两条默认运行路径。显式 `tomcat.path` / `MULTICA_TOMCAT_PATH` 覆盖仍保留。
- 构建成功不等于安装，安装不等于运行服务已切换。正式安装、PATH 和重启由用户另行安排；重启会停止全部共享任务，插件维护不会自动重启后台。
- Multica 使用原生命令 `multica daemon start --profile local-tomcat` 和已有 PATH/登录 shell 发现，不再需要 Tomcat 专用启动脚本。先检查已保存 profile 的 daemon 状态，启动后再确认 `running` 且包含 Tomcat；发现成功不等于注册或接单成功。本迁移包不执行登录、安装或 daemon 启停。

### 5.2 启动策略与实际执行入口

四项策略通过既有 agent 启动参数（`ExecOptions.CustomArgs`）原样传入：

| 参数 | 含义 |
| --- | --- |
| `--mode=chat/plan` | 显式指定新建或恢复后的模式；省略遵循 Tomcat 原有规则。 |
| `--plan-review=ask/auto/skip` | 保存后询问、自动评审或跳过评审。 |
| `--question-policy=manual/recommended` | 人工回答或选择显式标记的推荐项；推荐答案不是开发授权。 |
| `--plan-start=ask/auto` | 明确要求开工时保留或省去额外确认；保存计划不等于开工。 |

不新增对应的 `TOMCAT_ACP_*` 环境变量，也不由 Go 改写成布尔值。manual 等待沿用同机、同 HOME、同项目的 VS Code observer；Multica 不新增答题入口，无 observer 时继续等待，取消须撤回真实待答卡片。

使用这些参数前，在 daemon 的实际账号下核对实际可执行文件的 `acp --help`；早期策略适配使用 CLI `0.1.89`，原 `0.1.80` ACP 最低版本并不证明支持新策略。本轮强度需要更新 CLI 和运行中的服务，版本与能力判定见 §5.3。内建 runtime 在握手前、stdout 为空、未超时且退出码恰为 2 时，报告参数错误及实际执行路径，保留恢复引用；自定义命令、有 stdout 或其它退出码仍按清理未确认处理。

同一后端只保留桌面 daemon 或一个 CLI profile 执行 Tomcat。启动前分别核对状态；更新 PATH 或安装文件不会改写已运行 daemon 继承的 `MULTICA_TOMCAT_PATH`。若仍指向旧 VS Code 扩展目录，应由用户安排切换和重启，不由本包自动安装、重启或回退到另一份二进制。

### 5.3 推理强度：下次任务生效，不改别的会话

```text
Multica：选择模型/强度 -> 保存智能体 -> 下次新建或续接任务
                                           |
                           new/load -> model -> effort -> prompt
                                                   |
                             设置失败或未确认 ------+--> 报错，不执行
```

- 档位和默认模型来自现有 `serve models`。没有档位的模型不显示强度控件；不拿目录第一项冒充默认模型。
- **High/Low 等明确档位**写入当前 Tomcat 会话。两个智能体使用同模型时可以各选不同档位，ACP 不修改共享模型默认；恢复会话保留选择，每次续接仍会应用当时的智能体配置。
- **Default** 在 ACP 中发送 `effort=default`，清除会话覆盖并继承 Tomcat 模型默认。`off` 若在目录中，是一个档位，不是 Default。切到不同模型时清除旧覆盖，再应用新选择；重复设置同一模型不清除。
- 严格先设置模型，再设置强度，确认回显后才发任务。非空要求没有 `effort` 选项、被拒或未确认时明确失败，不发 prompt、不另建会话重试；恢复失败保留原会话引用，新会话设置失败不保留新引用。
- 旧 Tomcat 可以发现档位，但执行时可能没有 `effort`。此时选 Default 仍可沿原路径执行；要使用明确档位，须由部署者更新 CLI，并在确认任务后重启对应共享服务。**只看 `0.1.96` 等版本号不够**：同版本旧工作树或未重启服务不一定包含本功能。以运行服务返回的 ACP `effort` / `thought_level` 选项为准；本包不含 CLI/扩展二进制，也不会自动安装或重启。
- VS Code 在任务开始时刷新同会话模型/强度，不承诺无任务时跨窗口即时同步。计划预览显示该会话的有效值；手动修改和 CLI `/effort` 仍保存模型默认，并清除目标会话的同模型覆盖。部分失败如实报告“默认已保存，但本会话覆盖未清除”。
- Tomcat 现有限制不变：`llm.thinking.enabled=false` 或 `level=off` 时，有声明档位的模型按原策略取最低档；classic Anthropic 的固定 `max_tokens` 预算可覆盖档位预算。目录两端一致，不为 ACP 另做过滤；本地声明不证明中转站接受或转发参数。
- 原 Multica 基线对过期档位保留原来的**日志告警并丢弃**；新版宿主可把告警写进任务时间线。本包不为此导入新版告警机制或改校验算法。相应用例回迁到包内 `tomcat_models_test.go`。

## 6. 取消、恢复和 Windows

- 单任务取消关闭 ACP forwarder stdin，等待 Tomcat 清理确认；超时才杀掉 backend 自己拥有的进程树，并明确记录共享服务清理未确认。不能把强杀写成“服务内所有工具已停止”。
- 正常完成要等 forwarder 实际退出后再交给 Multica 做 worktree 清理，避免目录提前删除。
- daemon 原有 30 秒 handoff 不延长为 adapter 的 65 秒 grace；超过 handoff 或未交付清理结果时，以 `cleanup_unconfirmed` 阻断本次自动继续，并保留 worktree、运行时文件和诊断。不要在人工确认残留进程前重新派发到该目录。
- 包保留旧基线 native `daemon stop` 的既有语义，不提供新版强停机确认协议。daemon 不在运行、活动任务为零或一次 stop 成功，都不能单独证明共享服务内的工具已停止。
- 已完成的 turn 不被迟到的 cancel 改写；`session/load` 失败只有结构化 unknown session 等情况才进入宿主的 fresh/resume 拒绝流程。
- Windows 下 Multica Job 可能不允许 Tomcat ACP 再脱离创建共享服务。执行任务前在相同账号和配置目录的独立终端执行 `tomcat serve start`，再派任务；不要修改 Job 权限绕过这个边界。
- 这是同一台执行机器上的本地进程和本机 IPC，不是让浏览器直接访问远程 Tomcat。远程任务必须把 daemon、Tomcat 和 cwd 一起部署到目标执行机。

### 6.1 Sharing a session with VS Code

```text
Multica 的 ACP 轮次 -> 同一 serve 会话 -> VS Code 同项目打开并持续收流
  VS Code：待发 / 立即发送插话 / 停止 / 附件准备
  会话静止：VS Code 写入接手 -> 引用并修订原计划
  新 Multica 任务：session/load 接回原会话 -> 新轮
```

需要支持 `shared_session_input` 的 Tomcat 服务和匹配扩展；旧服务仍只读。`access` 仅表示是否宿主，不再由 `origin` 永久限制输入。插话、停止、附件准备不换宿主；其它写入和 ACP load 仅在旧活宿主没有活工作时接手，并切换执行环境、私有 MCP、确认路由与交互策略。会话历史、模型、模式、planId 与计划文件不变；VS Code 恢复人工确认策略，不自动开工。删除不接手；他端活宿主（含空闲）仍会阻止，断开后的 ACP 来源本身不阻止。

运行提示只显示当前宿主名称，例如“正在 multica-agent-sdk 中运行”；名称取当前连接上报值，不沿用会话创建来源，也不硬编码品牌别名。接手、换宿主或断开会同步更新/清空；未上报名或旧服务缺字段时显示“正在其他窗口或客户端中运行”，空闲后隐藏，不再附操作说明。

**仅 Tomcat 改动的七个边界（Multica 代码不变）：**

1. 插话原文不出现在 Multica 时间线：Tomcat ACP 实时映射和 Multica `normalizeACPUpdateType` 都不提供该展示；模型会收到插话，原任务期间的后续回复仍回到 Multica，不能把用户输入伪装成助手消息。
2. VS Code 停止当前轮次，Multica 将任务记为 `aborted`（`tomcat.go` 的 cancelled 分支）。
3. Multica 仅在自己的 prompt 期间接收，轮次结束关闭 stdin。VS Code 后续新轮不在 Multica 实时显示；其下次 load 继承历史/计划上下文，但不回放到 Multica UI。VS Code 重试成功也不会把原失败 Task 改成成功。
4. VS Code 正在运行或仍有后台工作时，Multica load 返回 `session_in_use`，不是 fresh 信号；等会话静止后再触发。
5. 删除对话不删计划文件；之后 Multica 收到 `unknown_session`，按其原有恢复拒绝流程新建会话、从 issue 重建上下文。旧对话和计划关联不会自动恢复。
6. 仅网络中断附英文诊断；429、5xx、额度、鉴权等如实返回。是否自动重派由 Multica 决定：其当前策略排除 autopilot、triage、`max_attempts ≤ 1` 等情况，并有次数上限。若有 VS Code 正在显示该会话，符合恢复条件的失败也可触发原有 5 分钟倒计时；新轮开始会取消它。Multica 排队超过 5 分钟时 VS Code 可能先接手，随后 load 仍可能被忙碌状态拒绝。
7. VS Code 接手不继承已结束任务的 `MULTICA_TOKEN`、任务 ID 或临时目录；之后运行 `multica` 使用本机 CLI 登录身份，通常是用户本人，未登录可能失败。若要求 Agent 身份，须另行在 Multica 侧设计有效授权。

**网络诊断是对 Multica 当前分类词表的依赖，不是新重试协议。** serve 的失败 `agent_idle` 携带与 transcript 相同来源的 `failure {kind, phase}`。仅 `kind=stream_interrupted` 时，ACP `session/prompt` 的 `error.data` 附下列不本地化文本，`message` 保留原摘要：

| phase | data |
| --- | --- |
| Connect | `network error: unable to connect to the model provider` |
| Send | `network error: error sending request to the model provider` |
| ReadTimeout | `network error: i/o timeout waiting for the model provider` |
| BodyRead / IdleTimeout / 无阶段 | `network error: model provider stream disconnected` |

Multica `hermes.go` 将 data 拼入错误，`pkg/taskfailure/classify.go` 据关键词识别 `provider_network`，`internal/service/task.go` 决定重派并保留原会话引用。若词表或结构化错误协议变化，需要重新验证。当前工作仅用 Tomcat 的真实 ACP 新进程模拟重派、同会话 load；**没有运行真实 Multica 全链路**，不据此声称 issue 评论或通知行为已实测。本节只更新使用契约，不更改迁移包代码、补丁或来源哈希。

## 7. 最小验证

环境要求：Go `1.26.9`、Node `>=22`、pnpm `10.28.2`。从 Multica 根目录先执行生产包聚焦测试：

```sh
(cd server && ../scripts/go-test-with-agent-cli-guard.sh -- \
  go test -p 2 -parallel 2 ./pkg/agent ./internal/daemon ./internal/daemon/execenv ./pkg/taskfailure \
  -run '^Test(Tomcat|AllReasons|ExecuteAndDrain.*Terminal|ExecuteAndDrainDoesNotDelayBackendsWithoutATerminalBoundary|IdleWatchdogYieldsToTerminalObservedDuringCleanup|HandleTask_AcksCancel)' -count=1)
```

迁移测试必须使用受管隔离 `DATABASE_URL`；没有数据库时的 `skip` 不是通过：

```sh
(cd server && go test ./internal/migrations \
  -run '^(TestTomcatProtocolFamilyMigration|TestMigrationNumericPrefixesAreUnique|TestMigrationFilesHaveMatchingDirections)$' \
  -count=1)
```

UI/typecheck：

```sh
pnpm --filter @multica/views test \
  runtimes/components/runtime-profile-catalog.test.ts \
  runtimes/components/runtime-profiles-dialog.test.tsx
pnpm typecheck
```

受控 ACP 跨仓协议测试（不是付费模型测试）：

```sh
(cd server && MULTICA_RUN_REAL_AGENT_SMOKE=1 \
  MULTICA_TOMCAT_TEST_BIN=/absolute/path/to/tomcat \
  go test -tags=agentintegration ./pkg/agent \
  -run '^TestTomcatSharedServeWithHermesClient$' -count=1 -v)
```

它使用明确的 Tomcat 二进制、隔离 HOME、本地 HTTP fixture 和假 Key，会真实启动并停止**私有**本地服务但不调用外部模型；包含发现零请求、会话清单前后不变、原始 ID/model_name、四项启动策略作用于同一计划，以及人工待答卡片取消后撤回。普通测试仍经过 CLI guard，不会自动发现或启动用户的 Tomcat。

`tests/new-files/e2e/tomcat-model-discovery.spec.ts` 是显式opt-in的创建页旅程，使用现有 `TestApiClient`，不修改页面或mock目录API。运行前按 `VALIDATION.md` 准备独立HOME/registry/PGDATA/端口、loopback provider和本轮二进制；`--ephemeral` 本身不是隔离。测试根与证据目录通过 `TOMCAT_DISCOVERY_FIXTURE_ROOT` / `TOMCAT_DISCOVERY_EVIDENCE` 指定；未准备时skip不计通过。旧基线的页面路径/英文资源/任务载荷可能不同，此版 E2E 源码面向 `5e36721c` 基线加本轮工作树，验证选档、保存、刷新、续接与 Default；不把新版页面倒入原基线，也不冒称旧页面已运行此浏览器脚本；原基线接线由独立 Go/真实 ACP 重放验证。

真实宿主的其它部署验收仍需另外执行：runtime profile → 本地目录 → 新任务 → 单任务取消 → 原目录/会话继续或明确 fresh → worktree 清理。

## 8. 回退和许可证边界

回退前停止派发新任务并等待运行中的 backend 收尾；然后按公司 migration ledger 反向回滚本次代码。不要直接运行全局 `make migrate-down` 来“撤销 Tomcat”，也不要只删除 `tomcat.go` 而留下 runtime type、daemon 探测和 provider 白名单。

本包随附 `LICENSE` 和 `NOTICE`。Multica 来源许可证包含 Apache 2.0 文本及额外的商业托管、品牌和归属条件；公司内部使用、对外托管、产品嵌入和再分发应由公司法务/许可证负责人按公司来源版本判断。本说明不授予额外许可证。

本包只证明来源开源分支的迁移材料完整；公司 GitLab 二开分支、公司 daemon 服务账号、数据库、Windows 环境、代理和发布流程尚未由本包代为验收。
