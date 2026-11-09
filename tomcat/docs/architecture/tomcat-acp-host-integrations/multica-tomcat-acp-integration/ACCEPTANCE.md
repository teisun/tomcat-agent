# Multica 接入验收清单

## A. 代码与迁移

- [ ] 公司基线、Go/Node/pnpm 版本和当前 migration ledger 已记录。
- [ ] `server/pkg/agent/tomcat.go` 未覆盖公司已有 backend 定制。
- [ ] agent factory、daemon 探测、execenv、launch 参数过滤、模型和版本登记已合并。
- [ ] `AGENTS.md`、`.agents/skills` sidecar 和 worktree 清理链路仍完整。
- [ ] `Session.CleanupRequired`、daemon handoff、`cleanup_unconfirmed` 分类与取消回报已语义合并；未确认时保留 worktree/sidecar，不自动提交或删除。
- [ ] Tomcat 迁移编号唯一，up/down/provider 白名单和迁移测试已按公司前态调整。
- [ ] 不使用全局 migrate-down 作为单项回退。

## B. 执行账号

- [ ] daemon 实际运行账号能找到包含 ACP 的 Tomcat 二进制。
- [ ] `tomcat acp --help` 包含 `--skills-dir`。
- [ ] 实际入口的 help 支持所配置四项启动策略；旧入口参数早退显示实际路径和 stderr，不被误判为清理未确认。
- [ ] daemon 服务环境拥有正确 HOME、Tomcat 数据目录、模型配置和 Key。
- [ ] `tomcat serve status`、模型设置和代理配置在同一执行账号下检查。
- [ ] 同一后端仅桌面或一个 CLI profile 执行 Tomcat；已检查运行 daemon 继承的实际路径，而非仅检查当前 shell。
- [ ] 没有把 `HOME/PATH/TMP/MULTICA_*` 通过 agent custom_env 注入。
- [ ] 模型目录使用实际runtime的 `serve models`，不创建临时ACP会话或调用付费模型；缺凭据模型不展示。
- [ ] 零模型/旧命令/坏配置/超时经原failed/error到创建页原因提示；默认/手填路径仍可用，无新增toast或页面生产改动。
- [ ] 保存/删除凭据后显式刷新可见最新目录；保存的是原始ID，ACP执行使用对应model_name。
- [ ] 已读取桌面/窄屏PNG、ARIA及console；目录成功没有被当作网关/额度健康证明。

## C. 受控 Multica 旅程

| 场景 | 必须看到 |
| --- | --- |
| runtime profile | `runtime_type=tomcat`，command 为 `tomcat`，fixed args 不含 `acp`。 |
| 新任务 | local_directory cwd 同时进入 ACP session 和真实子进程。 |
| AGENTS/skills | 模型能看到任务规则；worktree 结束后 sidecar 和目录按既有机制清理。 |
| 取消 | 关闭 forwarder stdin，实际进程退出；超时强杀明确记录，不伪称共享服务已确认停止。 |
| 清理未确认 | 超过 handoff、关闭 Result 无值、清理超时或异常退出均保留清理错误与目录；不因用户取消吞掉诊断，不自动 fresh 重试。 |
| 参数解析早退 | 仅内建 runtime、空 stdout、exit 2、未超时适用普通失败例外；三个保守反例仍为清理未确认，恢复引用不丢失。 |
| 启动策略/人工等待 | 四参数独立透传；推荐答案不是批准，保存不是开工；真实待答卡片取消后撤回，同会话可继续。 |
| 继续 | 兼容时走 `session/load`；worktree 已销毁时按宿主规则 fresh 或明确失败。 |
| 结果 | 完成、取消、恢复拒绝、usage partial/unpriced 状态准确。 |
| Windows | 预启动共享服务后再派任务；不能依赖受管 Job 内临时拉起服务。 |

## D. 公司分支结论

受控协议测试通过后，还必须在公司 daemon、数据库和实际 runtime profile 下跑一次完整任务。未跑完前，不要写“公司 Multica 已接入成功”。
