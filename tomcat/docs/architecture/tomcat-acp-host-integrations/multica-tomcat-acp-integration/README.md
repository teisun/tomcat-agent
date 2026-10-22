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

- `new-files/server/pkg/agent/tomcat.go`：Tomcat backend 完整实现。
- `new-files/server/migrations/`：运行时类型白名单的 up/down 示例；编号必须按公司迁移 ledger 调整。
- `existing-files.patch`：agent factory、daemon 探测、执行环境、模型和版本登记的增量修改。
- `tests/`：backend、daemon、执行环境、迁移和受控 ACP 集成测试。
- `examples/runtime-profile.example.json`：无密钥 runtime profile 请求体。
- `examples/tomcat.config.fragment.toml`、`examples/models.fragment.toml`：Tomcat 执行账号配置片段。
- `FILES.md`：23 个来源变更路径的逐项清单。
- `manifest.json`：来源提交和 SHA-256。
- `LICENSE`、`NOTICE`：来源 Multica 许可证和归属文件副本。

来源：Multica 基线 `06835b8f50606ca3f8bebbb5d07ac8901f831e69` → 接入提交 `e12a45e59d8bfddf2aba192f163899a9eb290a3b`。Tomcat 依赖代码为 `401b6f0cbd8be8478b5453815450f7ce6ada00e7`。

核心实现确实只有一个 backend 文件；其余生产修改是把它接入已有 runtime、daemon、执行环境和模型流程。不能只复制 `tomcat.go` 后认为集成完成。

## 2. 合并前必须核对

公司二开版本需要具备或等价实现：

- `Backend.Execute`、`ExecOptions`（特别是 `Cwd`、`Model`、`ResumeSessionID`、`CustomArgs`、`McpConfig`）；
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

## 6. 取消、恢复和 Windows

- 单任务取消关闭 ACP forwarder stdin，等待 Tomcat 清理确认；超时才杀掉 backend 自己拥有的进程树，并明确记录共享服务清理未确认。不能把强杀写成“服务内所有工具已停止”。
- 正常完成要等 forwarder 实际退出后再交给 Multica 做 worktree 清理，避免目录提前删除。
- 已完成的 turn 不被迟到的 cancel 改写；`session/load` 失败只有结构化 unknown session 等情况才进入宿主的 fresh/resume 拒绝流程。
- Windows 下 Multica Job 可能不允许 Tomcat ACP 再脱离创建共享服务。执行任务前在相同账号和配置目录的独立终端执行 `tomcat serve start`，再派任务；不要修改 Job 权限绕过这个边界。
- 这是同一台执行机器上的本地进程和本机 IPC，不是让浏览器直接访问远程 Tomcat。远程任务必须把 daemon、Tomcat 和 cwd 一起部署到目标执行机。

## 7. 最小验证

环境要求：Go `1.26.9`、Node `>=22`、pnpm `10.28.2`。从 Multica 根目录先执行生产包聚焦测试：

```sh
(cd server && ../scripts/go-test-with-agent-cli-guard.sh -- \
  go test -p 2 -parallel 2 ./pkg/agent ./internal/daemon ./internal/daemon/execenv \
  -run '^TestTomcat' -count=1)
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

它使用明确的 Tomcat 二进制、隔离 HOME、本地 HTTP fixture 和假 Key，会真实启动本地服务但不调用外部模型。真实宿主验收仍需另外执行：runtime profile → 本地目录 → 新任务 → 单任务取消 → 原目录/会话继续或明确 fresh → worktree 清理。

## 8. 回退和许可证边界

回退前停止派发新任务并等待运行中的 backend 收尾；然后按公司 migration ledger 反向回滚本次代码。不要直接运行全局 `make migrate-down` 来“撤销 Tomcat”，也不要只删除 `tomcat.go` 而留下 runtime type、daemon 探测和 provider 白名单。

本包随附 `LICENSE` 和 `NOTICE`。Multica 来源许可证包含 Apache 2.0 文本及额外的商业托管、品牌和归属条件；公司内部使用、对外托管、产品嵌入和再分发应由公司法务/许可证负责人按公司来源版本判断。本说明不授予额外许可证。

本包只证明来源开源分支的迁移材料完整；公司 GitLab 二开分支、公司 daemon 服务账号、数据库、Windows 环境、代理和发布流程尚未由本包代为验收。
