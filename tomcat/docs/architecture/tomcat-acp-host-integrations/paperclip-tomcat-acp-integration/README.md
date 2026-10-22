# Paperclip 接入 Tomcat：公司二开迁移说明

```text
Paperclip 任务 / 本地 workspace
            │
            ▼
   tomcat-local 适配器
            │  标准 ACP / stdio
            ▼
   tomcat acp → 共享 tomcat serve
                         │
                  Tomcat 保存的模型与代理配置
```

这是一份**只针对 Paperclip** 的独立迁移包。它把开源 Paperclip 的 Tomcat 适配器接入点、完整新增文件、现有文件增量补丁、测试和无密钥配置示例放在同一目录中；不要求阅读其它宿主资料，也不修改公司仓库。

## 1. 交付内容

- `new-files/`：本次新增的生产文件和适配器包配置，复制到 Paperclip 仓库同路径。
- `existing-files.patch`：现有生产/注册/共享 ACP 引擎文件的增量修改；**必须人工审查后应用**。
- `tests/`：新增测试、夹具、测试包配置，以及现有测试入口的增量补丁。
- `examples/adapter-config.example.json`：无密钥的适配器配置示例。
- `examples/tomcat.config.fragment.toml`、`examples/models.fragment.toml`：Tomcat 执行账号的配置片段。
- `FILES.md`：46 个来源变更路径的逐项清单。
- `manifest.json`：来源提交和 SHA-256，用于核对资料包未被改写。
- `LICENSE`：来源 Paperclip MIT 许可证副本。

生产接线的本质不是把 92 行 ACP 文件塞进仓库，而是：

1. 新增 `packages/adapters/tomcat-local/`；
2. 将 `tomcat_local` 登记到 shared/server/CLI/UI；
3. 将 Tomcat 加入 ACPX 的稳定命令映射和 `session/load` 复用名单；
4. 将它登记到 conversation continuation 与 UI 恢复流程；
5. 保留测试和包管理依赖。

来源：Paperclip 基线 `2f6ad2226aec330251438c47cbc45fd92e0d3132` → 接入提交 `1f010b1655b85ec4cfcf4ca45114e8fa899a601c`。Tomcat 依赖代码为 `401b6f0cbd8be8478b5453815450f7ce6ada00e7`。

## 2. 合并顺序

### 2.1 先检查公司二开基线

公司仓库需要具备或等价实现以下接口；缺失时先做兼容改造，不能删除功能检查来迁就补丁：

- `AdapterExecutionContext`、`ServerAdapterModule`、`createAcpxEngineExecutor`；
- `context.paperclipWorkspace.cwd`、`executionTarget`、`runtimeMcp`、取消信号和 session codec；
- `packages/adapter-utils` 的 skills、环境快照、权限、取消和 workspace recovery helpers；
- `acpx@0.12.0` 及本仓对其环境继承/进程取消的 patch；
- 现有 server/CLI/UI adapter registry 和 conversation continuation 状态。

当前实现只支持 **local execution**。远程 execution target 会被适配器拒绝，因为 Paperclip 执行端、Tomcat ACP 进程和工作目录必须在同一执行环境。

### 2.2 导入新增文件

把 `new-files/` 下的内容复制到 Paperclip 仓库根目录对应位置。例如：

```text
本包/new-files/packages/adapters/tomcat-local/src/server/acp.ts
→ 公司Paperclip/packages/adapters/tomcat-local/src/server/acp.ts
```

目标已有同名文件时停止并人工比较，不覆盖公司的二开实现。

### 2.3 应用现有文件补丁

```sh
git apply --check /absolute/path/paperclip-tomcat-acp-integration/existing-files.patch
git apply /absolute/path/paperclip-tomcat-acp-integration/existing-files.patch
```

补丁涉及：

- shared adapter type、server builtin registry、CLI formatter；
- ACPX command identity 和 `session/load` MCP 兼容名单；
- server/UI registry、Tomcat display、New Agent 配置和运行时检查；
- conversation continuation、agent identity、任务线程状态；
- workspace package 依赖、TypeScript references 和 lockfile。

公司已有同名改动时按逻辑合并。尤其不要覆盖公司自己的 provider 注册、权限策略、任务恢复条件或 UI 变体。

### 2.4 导入测试

把 `tests/new-files/` 按原路径复制；再先检查、后应用：

```sh
git apply --check /absolute/path/paperclip-tomcat-acp-integration/tests/existing-files.patch
git apply /absolute/path/paperclip-tomcat-acp-integration/tests/existing-files.patch
```

测试可以因公司分支路径或断言不同而调整，但不能删除以下行为覆盖：readiness 不调用模型、cwd 明确传递、skills 隔离与清理、session/load 恢复、取消、registry/UI 注册。

## 3. Paperclip 中的 Tomcat 配置

适配器配置示例见 `examples/adapter-config.example.json`：

```json
{
  "command": "/opt/tomcat/bin/tomcat",
  "cwd": "/srv/workspaces/example-project",
  "model": "default",
  "permissionMode": "deny-all",
  "nonInteractivePermissions": "deny"
}
```

字段含义：

| 字段 | 要求 |
| --- | --- |
| `command` | `tomcat` 或绝对可执行文件路径，默认 `tomcat`。必须是包含标准 ACP 的构建。 |
| `agentCommand` | 可选完整 launcher；必须接受适配器追加的 `--skills-dir`，否则不要配置。 |
| `cwd` | 现有的绝对目录。优先使用 Paperclip 任务 workspace 的 cwd；不能回退到 Paperclip server 进程 cwd。 |
| `model` | 空值或 `default` 使用 Tomcat 自己的默认模型；其它值通过 ACP `session/set_config_option` 发送。 |
| `permissionMode` | `approve-all`、`approve-reads` 或 `deny-all`，只控制 ACP approval 回复。 |
| `nonInteractivePermissions` | `deny` 或 `fail`。 |
| `env` | 宿主任务环境；不要把 Paperclip API key 当成 Tomcat 模型 key。 |

Tomcat 模型和模型 Key 在**执行账号自己的** `~/.tomcat/` 中配置：

```sh
tomcat init
tomcat model list
tomcat model key set company
tomcat model default company-model
tomcat serve start
tomcat serve status
```

`tomcat acp --help` 必须包含 `--skills-dir`。模型 Key 不写入 Paperclip 配置、Git、任务 prompt 或日志。

正式共享后台使用 Tomcat 保存的 `llm.proxy` 或操作系统代理；不会因为 Paperclip 进程有 `HTTP_PROXY` 等变量就切换共享后台的模型代理。bash 和 stdio MCP 的任务环境仍由原 Paperclip/工具规则处理。

## 4. 技能、cwd、取消和恢复

- 每次执行由适配器准备本轮私有 skills 目录，通过 `--skills-dir` 传给 Tomcat；任务结束后清理，不把技能复制进共享 Tomcat HOME。
- Paperclip workspace cwd 必须在执行端存在且可访问，传给 ACP `session/new` / `session/load`；适配器不会使用自身进程 cwd 猜目录。
- Paperclip Stop 先沿共享 ACPX 流程取消；超过宿主宽限期才强制结束本地转发进程。不要用 `tomcat serve stop` 代替单任务取消，因为它会影响共享服务上的全部任务。
- 续接使用 Paperclip 已有的 conversation continuation 规则。配置、cwd、权限、skills 身份等变化可能要求新会话；适配器测试证明的是兼容条件下可 `session/load`，不是所有 Retry 都保证原 ID。
- Paperclip 的 `runtime test` 只检查 CLI、`acp --help` 和 `serve status`，不会启动模型或创建对话；不要把 readiness pass 当成真实任务通过。

## 5. 最小验证

环境要求来自来源仓库：Node `>=24.11.0`、pnpm `9.15.4`、workspace links 正常、ACPX patch 已应用。建议从 Paperclip 根目录执行：

```sh
pnpm install
pnpm run preflight:workspace-links
pnpm --filter @paperclipai/adapter-tomcat-local typecheck
pnpm exec vitest run --project @paperclipai/adapter-tomcat-local --exclude '**/dist/**'
pnpm exec vitest run \
  server/src/adapters/registry.test.ts \
  server/src/services/conversation-continuation.test.ts \
  ui/src/adapters/tomcat-local/index.test.ts \
  ui/src/pages/NewAgent.test.tsx \
  ui/src/components/TaskChatThread.test.tsx \
  --exclude '**/dist/**'
```

如果公司分支的测试数据库或 stable runner 不可用，记录为环境阻塞，不改成通过。然后用固定的公司执行账号做一次真实但受控的 Paperclip 任务：就绪检查 → 新任务 → 受控模型/网关任务 → 单任务取消 → 原任务恢复或明确 fresh → 结果和 workspace 清理。真实模型调用需单独审批。

## 6. 回退和边界

回退前先停止新建任务并等待现有 Tomcat 任务收尾，再在公司分支回滚本次适配器提交；不要只删除 `tomcat-local` 目录而留下 registry、ACPX 名单或 lockfile 修改。Tomcat 共享服务由执行账号明确 `tomcat serve stop` 管理，停服会影响该服务上的全部任务。

当前包验证的是开源 Paperclip 本地分支的实现快照；**公司 GitLab 二开版本尚未由本包代为验收**。公司特有的权限、工作树、队列、审计、容器和发布流程仍需要在公司的分支中重新验证。

来源许可见本包 `LICENSE`。本说明不授予公司任何额外 Paperclip、Tomcat 或模型服务许可。
