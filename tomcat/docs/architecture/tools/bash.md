# `bash` 工具：前台观察、后台任务与输出治理

本文描述当前仓库已经落地的 `bash` 执行契约。历史 `execute_bash` 名称不再注册；实现的事实源是 `catalog.rs`、`tool_exec`、`executor/bash.rs` 与 `bash_task.rs`。

## 1. 目标与原则

`bash` 同时解决四件事：命令先经过权限与 AST 检查、所有模式共用一次 spawn、输出实时可见且有界、长命令不会占住一次工具调用。

```text
LLM 调用 bash
    │
    ├─ permission gate + BashAstChecker
    │
    └─ unified spawn（新进程组）
          │
          ├─ stdout/stderr ─▶ 实时事件 ─▶ CLI / WebView
          ├─ DrainingOutput ─▶ 有界内存结果 + 完整输出落盘
          └─ 前台观察 8–16 秒
                 ├─ 进程结束：返回 completed result
                 └─ 窗口到期：返回 task_id，进程继续后台运行
```

前台等待窗口不是进程生命周期上限。窗口到期只改变“这次调用是否继续等待”，不会终止进程。明确的 `task_stop`、用户取消或宿主 shutdown 才会终止对应 process group 并等待收口。

## 2. 调用与结果协议

### 2.1 `bash`

主要入参：

- `command`：必填命令；
- `cwd`：可选工作目录；
- `args`：可选 argv，存在时不经 shell 字符串解析；
- `run_in_background`：立即转为 tracked background task；
- `foreground_wait_ms`：单次调用覆盖前台观察窗口，执行前统一 clamp 到 `8000..=16000`。

调用在前台窗口内结束时返回退出码和有界输出。窗口到期或显式后台运行时返回 `task_id` 与日志路径，后续由 task 工具接续；它不是失败，也不代表进程已退出。

### 2.2 task 工具

- `task_output(task_id, since, max_bytes, block, wait_ms)`：从字节游标读取增量；`block=true` 时最多等待 `wait_ms`，返回 `finished`、`wakeReason` 与 `next_offset`。等待到期是非终态，可继续读取；
- `task_list`：列出 running / finished / stopped 任务及元信息；
- `task_stop`：终止任务所属 process group，随后 wait/reap，避免只杀 shell 而遗留孙进程。

```text
bash -> { task_id, log_path }
             │
             ├─ task_output(since=N, wait_ms=M) -> chunk + next_offset
             ├─ task_list()                     -> status snapshot
             └─ task_stop()                     -> kill process group + reap
```

后台自然结束会发 lifecycle event；host 使用 completion route 去重后，把 `<background-task-finished ...>` synthetic notification 放入 follow-up queue。它补充上下文，不覆盖更新的真实用户输入。

## 3. 统一 spawn 与取消

前台观察和显式后台都经过同一 spawn 基础设施：

1. 解析并校验 `cwd`；
2. `BashAstChecker` 检查复合命令与 deny 规则；
3. permission gate / confirmation / audit；
4. 创建独立 process group 并 spawn；
5. stdout、stderr reader 持续泵流；
6. 等待自然退出、转后台或收到取消。

`task_stop` 与 cancel 都针对 process group，而不是只调用单个 child 的 kill。终态翻转与 lifecycle event 有一次性 guard，避免 stop 与自然退出竞态产生双通知。

## 4. 输出链路

### 4.1 `DrainingOutput`

reader 在进程运行期间持续 drain stdout/stderr，避免任一 pipe 写满后造成子进程背压死锁。内存累积按 stdout/stderr 分流限制字符数，超限采用头尾保留；完整字节流持续写入 agent trail 的 `tool-results`。

```text
child stdout/stderr
      │
      ├─ Layer 1：实时 ToolExecutionUpdate / output event
      ├─ Layer 2：DrainingOutput 有界 head + tail
      └─ Layer 3：tool-results/bash-<task>.log 完整持久化
```

三层输出面向不同消费者：实时层保障 UI 进度，有界层保障模型上下文，持久化层保障完整诊断。即使输出已经超过模型回执上限，reader 仍继续 drain 并写盘。

### 4.2 结果字段

结果通过 `completed` / `finished`、`exit_code`、`task_id`、`truncated`、`persisted_output_path`、offset 等字段表达状态。前台观察到期通过 tracked-background 结果表达，不伪装成退出码或错误。

## 5. 配置与生产装配

配置优先级为 `env > TOML > default`：

```toml
[tools.bash]
# 合法范围 8000..=16000；默认 16000
foreground_wait_ms = 16000
# stdout / stderr 各自的内存字符上限；默认 30000
max_output_chars = 30000
```

等价环境变量：

```text
TOMCAT__TOOLS__BASH__FOREGROUND_WAIT_MS=9000
```

`foreground_wait_ms` 越界在配置加载/校验时报错；工具单次参数仍在 dispatcher / executor 边界 clamp，形成双层防御。旧 `tools.bash.timeout_ms` 被拒绝并提示迁移到 `foreground_wait_ms`。

生产装配必须把同一份 resolved policy 注入两条路径：

```text
AppConfig.tools.bash
    │
    ├─ DefaultPrimitiveExecutor
    │    ├─ foreground_wait_ms
    │    ├─ max_output_chars
    │    └─ <agent-trail>/tool-results
    │
    └─ BashTaskRegistry
         ├─ foreground_wait_ms
         └─ 同一个 tool-results 路径
```

这样，命令从前台转后台后不会发生等待策略或日志路径漂移。

## 6. AST、权限与审计

`BashAstChecker` 在 permission gate 之前执行：顶层 `;`、`&`、换行、`&&`、`||`、`|` 会按段检查；引号、命令替换和子 shell 内分隔符不会误切。deny 命中与不支持的 heredoc / 流程控制会在 spawn 前拒绝。AST 不替代现有 permission gate；未命中 allow/deny 的命令继续走 gate、confirmation 与审计。

成功、拒绝、spawn 失败和 stop 都保留审计信息；后台任务额外携带 `task_id`，便于把命令、日志与终态关联起来。

## 7. 状态机

```text
                     foreground window elapsed
Spawned / Running ─────────────────────────────▶ BackgroundRunning
       │                                                  │
       │ natural exit                                     ├─ natural exit ─▶ Finished
       ▼                                                  ├─ task_stop   ─▶ Stopped
    Finished                                              └─ cancel      ─▶ Stopped
```

窗口到期不是终态；`task_output` 的一次 `wait_ms` 到期同样不是终态。只有自然退出或显式 stop/cancel 才结束 process group。

## 8. 测试重点

- 配置默认值、TOML/env 非默认值、边界拒绝及 legacy key 迁移提示；
- primitive 与 registry 的生产 resolved policy 一致；
- 8 秒下界、16 秒上界及单次参数 clamp；
- 前台窗口到期后任务仍可由 `task_output` 读取并自然结束；
- stdout/stderr 实时事件、有界结果、完整落盘三层一致；
- `task_stop` / cancel 终止 process group 且 lifecycle 只发一次；
- `task_output(block=true, wait_ms=...)` 的游标、等待到期与 finished 协议。

## 9. 风险与约束

- 长时间不读取 pipe 会死锁：所有路径必须使用统一 reader / `DrainingOutput`；
- 只终止直接 child 会泄漏孙进程：停止路径必须面向 process group；
- foreground 与 registry 分别拼配置会漂移：生产只从同一 resolved policy 装配；
- 输出事件可能很密：UI 层应节流展示，但不能阻塞 drain 与落盘；
- lifecycle 与主动读取可能重复交付：completion route 与一次性 guard 共同去重。

## 10. 关联实现

- `src/core/tools/primitive/executor/bash.rs`：权限后执行、统一 spawn 与前台观察；
- `src/core/tools/primitive/bash_task.rs`：tracked task、日志、task 三件套与 lifecycle；
- `src/core/tools/primitive/executor/output_accum.rs`：有界输出；
- `src/core/agent_loop/tool_exec/branches/bash.rs`：参数解析与结果协议；
- `src/api/chat/context.rs`：生产配置装配；
- `src/infra/config/types/tools.rs`：配置契约与范围常量。
