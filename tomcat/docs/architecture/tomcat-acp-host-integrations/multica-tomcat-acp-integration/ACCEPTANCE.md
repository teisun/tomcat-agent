# Multica 接入验收清单

## A. 代码与迁移

- [ ] 公司基线、Go/Node/pnpm 版本和当前 migration ledger 已记录。
- [ ] `server/pkg/agent/tomcat.go` 未覆盖公司已有 backend 定制。
- [ ] agent factory、daemon 探测、execenv、launch 参数过滤、模型和版本登记已合并。
- [ ] `AGENTS.md`、`.agents/skills` sidecar 和 worktree 清理链路仍完整。
- [ ] Tomcat 迁移编号唯一，up/down/provider 白名单和迁移测试已按公司前态调整。
- [ ] 不使用全局 migrate-down 作为单项回退。

## B. 执行账号

- [ ] daemon 实际运行账号能找到包含 ACP 的 Tomcat 二进制。
- [ ] `tomcat acp --help` 包含 `--skills-dir`。
- [ ] daemon 服务环境拥有正确 HOME、Tomcat 数据目录、模型配置和 Key。
- [ ] `tomcat serve status`、模型设置和代理配置在同一执行账号下检查。
- [ ] 没有把 `HOME/PATH/TMP/MULTICA_*` 通过 agent custom_env 注入。

## C. 受控 Multica 旅程

| 场景 | 必须看到 |
| --- | --- |
| runtime profile | `runtime_type=tomcat`，command 为 `tomcat`，fixed args 不含 `acp`。 |
| 新任务 | local_directory cwd 同时进入 ACP session 和真实子进程。 |
| AGENTS/skills | 模型能看到任务规则；worktree 结束后 sidecar 和目录按既有机制清理。 |
| 取消 | 关闭 forwarder stdin，实际进程退出；超时强杀明确记录，不伪称共享服务已确认停止。 |
| 继续 | 兼容时走 `session/load`；worktree 已销毁时按宿主规则 fresh 或明确失败。 |
| 结果 | 完成、取消、恢复拒绝、usage partial/unpriced 状态准确。 |
| Windows | 预启动共享服务后再派任务；不能依赖受管 Job 内临时拉起服务。 |

## D. 公司分支结论

受控协议测试通过后，还必须在公司 daemon、数据库和实际 runtime profile 下跑一次完整任务。未跑完前，不要写“公司 Multica 已接入成功”。
