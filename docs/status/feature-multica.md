| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tomcat | 2026-11-08 23:31 +0800 | ACTIVE | feature/multica | - |

### ✅ DONE (已完成/进行中)

```text
Mika → planner 写计划 / 独立评审 → 附件审阅 / 明确批准
                                             ↓
Mika 汇报 ← coder 验证并交付 ← 执行、更新同一 PlanFile
                      manual 问题 → 同项目 VS Code 旁观答题
```

- ACP 通过现有自定义参数接入四项独立设置：初始 Chat/Plan 模式、计划评审、普通问题答题和明确开工时的确认策略。新建及载入在首轮任务前应用设置，复用标准 `configOptions` / `session/set_config_option`；握手核对后台已接受启动设置，旧后台不支持时明确失败，不静默降级或自动重启。
- 默认普通问题为 manual；recommended 记录真实的策略答案来源，不代替开发批准、计划评审同意或工具授权。策略跟随控制连接，接管时恢复新控制者的设置或默认值，不写入计划或会话持久数据。
- 复用原提问桥，让同 HOME、同项目的 VS Code 旁观窗口回答 ACP 问题，支持迟到和重连补发。旁观者不能取消整个问题、接管控制或批准工具权限；服务端唯一采纳，其他窗口撤卡，客户端只缓存本窗实际提交的答案，不冒充服务端确认。
- 同机、同 HOME 下按原 `plan_id` 交接和更新标准 plans 目录内的同一文件。文件路径由智能体读取取得 ID，不新增路径参数或 SHA 交接协议。执行归属优先使用 `session_id`；旧 planner 恢复、冷载入或写入不能接管 coder 正在执行的计划。
- 整改完成：模式通知只在已有进行中 prompt 时排队，发送失败只记日志；恢复提问诊断日志，记录启动配置失败的真实原因；补齐唯一模式通知、控制者接管策略重置及消费者错误分类回归，并删除重复检查。
- 更新架构、计划工具和规划技能说明，纳入已交付的调研及早期方案文档。早期方案是历史材料，不代表其中的数据库、专用模式 UI 或运行中切换建议已实施；当前配置指南维护在 Multica 仓库 `tomcat_config.md`。
- 纳入此前已完成的版本同步：CLI / bundled CLI `0.1.88`、扩展 `0.2.6`，本次提交不再次升版。darwin-x64 完整 VSIX 已另行交付，构建产物保持忽略，不入库。

### 🔌 INTERFACE (接口变更)

- `tomcat acp` 新增 `--mode=chat/plan`、`--plan-review=ask/auto/skip`、`--question-policy=manual/recommended`、`--plan-start=ask/auto`。不指定 mode 时，新会话为 Chat，恢复会话保留原模式；`--plan-start=auto` 不等于保存即开工。
- ACP select 配置保留 `model`，新增 `agent_mode`、`plan_review`、`question_policy`、`plan_start`，返回实际值与完整选项；Agent 执行期间自行切换模式时发送完整 `config_option_update`。
- 共享服务 ACP 握手回显 `acpOptions`，启动模式设置失败向客户端返回固定 `session_config_failed`。Native wire 没有新增可反序列化的策略命令，权限、路径隔离和既有控制权守卫保留。
- Multica 产品 adapter 继续原样传入自定义参数，不新增产品参数解析器、数据库字段、审批协议或轮询器。

### 验证与提交范围

| 范围 | 结果 | 证据与边界 |
| :--- | :--- | :--- |
| 当前交付快照 | Tomcat 68 个原变更文件、Multica 3 个原变更文件哈希一致；源码与整包基线一致 | 本轮 `1794151679400-2f39sl`，exit 0；包含明确保留、不提交的备份与生成文件 |
| 格式与版本 | Rust fmt、版本一致性和两仓差异空白检查通过 | 本轮 `1794151679400-2f39sl`；暂存范围另含新增 Markdown 文档 |
| 扩展快速门禁 | 扩展/GUI 类型检查、core 947 项和 GUI 904 项通过 | 本轮 `1794151679461-iwtxzt`，exit 0；预期故障用例的 stderr 不计为测试失败 |
| Multica 静态与错误分类 | gofmt、tagged go vet、配置失败不误判会话丢失测试通过 | 本轮 `1794151679528-2ekzb0`，exit 0 |
| Rust 标准 gate-fast | Clippy；lib 3498 passed / 7 ignored；并行集成 396 passed / 27 skipped；串行集成 92 passed / 3 skipped | 复用未变化源码的 `1794147050609-i31uzu`，exit 0；ignored/skipped 不计通过 |
| Go 跨仓集成 | 使用开发构建的 `^TestTomcat` 受控集成通过，88.539s | 复用 `1794147050670-yl7yk6`，exit 0；私有 HOME / 本地模型 fixture，不冒充付费模型验收 |
| 真实平台与旁观界面 | 已有人工及自动推荐协作旅程、VS Code 真宿主与旁观重连记录 | 本轮复用，不重启已停止的专用验收环境，不把本次单测称为新的视觉或安装态验收 |

本轮对照已完成的开发/整改计划和当前差异自检，仅补充分支状态并提交既有交付内容，不声称新增独立模型代码评审。详细本机证据在 `.agents/acceptance/multica-plan-collaboration/REPORT.md` 和 `.agents/acceptance/multica-tomcat-remediation/REPORT.md`；证据目录、截图、VSIX 与旧 `.bak` 备份不作为产品源文件纳入提交。保留原文报告及用户已有修改，不执行拟议的帮助示例测试任务。

### ⚠️ BLOCKED (阻塞/风险)

| 项目 | 原因／影响 | 后续与边界 |
| :--- | :--- | :--- |
| 默认答题升级行为 | 未配置普通问题策略的 ACP 智能体会等待同项目 VS Code 人工回答 | 无人值守须显式 recommended；不将自动答题当作开发或工具授权 |
| 当前后台与安装状态 | 磁盘产物或本地提交不证明正在运行的后台已更新 | 安装、重启由用户另行操作；本次不安装、不改真实配置/PATH、不重启服务、不推送或发布 |
| 验证边界 | 覆盖率未测量；Windows/Linux/arm64 原生旅程及远端 CI 未由本轮验证 | Cov% 留空值 `-`，提交信息省略覆盖率，不宣称跨平台或所有可能测试全绿；历史失败与环境限制保留在既有证据中 |
