| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Tomcat | 2026-11-06 17:40 +0800 | ACTIVE | feature/multica | - |

### ✅ DONE (已完成/进行中)

- 整改计划生成链路：主章节使用二级标题，概览保留为 Plan 内三级节，术语首次出现时解释；`create_plan` 保存完整正文和标题，不再诱导约2000字符的摘要，保留旧包装兼容并保护围栏示例。
- 将 Final Check 原文抽为技能附件，入口第4步要求成稿后读取并完成检查；仅替换已获准的锁／版本判定短语。reviewer 具体检查职责、分支、UI 布局与用例缺口，要求原位修订并保持图文一致；评审仍为可选建议。
- 恢复两组原有措辞对照例子并逐字核对；现有保存流程图、失败态线框放入图文章节，明确延续 Example 1，删除重复的简化对照。Skill 主文与全部四个附件现共20,093字符，相对最初19,759字符净增334字符；这是文本口径，不是实际模型 token 或账单。
- 调整计划预览六级标题的字号与字重，保持相对阅读字号缩放，避免深层标题比正文小；保留原有语义标题及窄屏规则。
- 使用 `scripts/release-version.mjs bump --all patch` 仅升级一次：CLI `0.1.85 → 0.1.86`、插件 `0.2.3 → 0.2.4`、内置 CLI `0.1.85 → 0.1.86`。五个版本文件只改版本字段，升级前后版本一致性与版本文件空白检查通过；未发布或安装。
- 本次本地提交覆盖21个原已跟踪改动、新增 Final Check 附件及本状态文件。按用户明确要求不重跑测试、不编译、不打包；不推送、不安装、不重启服务。覆盖率未测量，提交信息不填写虚构的 Cov%。

### 2026-11-06：模型发现与统一重试整改

```text
runtime绑定命令 → serve models → 原模型菜单 → 保存原ID → ACP执行
请求失败 → 共用10次预算/30秒退避 → 原错误卡 → Retry或Resume
```

- 新增只读 `tomcat serve models`，复用服务身份、saved-only目录与 `ListModelsPayload`；不启动服务、不建会话、不请求模型/MCP，保留既有temp校验初始化。Multica本轮新增生产改动仅 `server/pkg/agent/models.go` 的Tomcat分支和 `tomcat_models.go`；daemon/API/公共Query/缓存/页面/通知/调度不改。
- 缺凭据模型过滤；零可用、坏载荷、旧命令与超时均error，经原failed/error到创建页已有原因/手填提示。默认优先级与已有执行路径不变，不加toast；详情页错误显示保持原状。
- 删除有人/无人策略分流，所有入口默认10次（含首次）、普通退避30秒，显式保存配置跨主/子Agent一致；输入修复/未知错误独立4次保护、Retry-After及取消边界保留。显式保存4的用户不会被自动改成10。
- 完整工具尾部的Resume不再依赖最初user在末页；Retry仍要求原消息ID。单行请求进度及按钮/点击条件已对齐，复用原五分钟等待器，不给CLI/ACP添加后台计时器。
- 受控真实CLI/共享serve/Hermes测试通过；隔离Multica浏览器旅程 `1793946867530-af28bb` 1 passed，选 `catalog-alias` 经UI保存后真实上游收到 `alias-wire` 并完成任务。私有HOME/registry/独立PGDATA与非5432端口，loopback provider和假Key；7组桌面/窄屏PNG、ARIA、console已读取，无pageerror或console.error，20条字体预加载warning已单列并验证资源200。
- Rust目录黑盒4+嵌套5、聚焦134、扩展/GUI393及既有Multica前端29项回归通过；真实devhost重试/分页1项及恢复4项通过。资料包保持原精确基线，新增修复仅adapter，验证见包内 `VALIDATION.md`。这些是分层有效证据，不冒充最终聚合gate全绿或覆盖率。
- 最终跨包检查补齐：fmt/严格all-targets Clippy通过；extension/GUI其余718+727项、Provider73项通过。ACP终态错误fixture固定单次预算后通过；多工作区fixture等待原ready/bootstrap流程后通过；VSIX打包串行补验通过。真serve五分钟恢复和16组最终功能PNG/ARIA/console证据保持有效。
- 聚合门禁不冒充全绿：Rust工具目录文档有既有`create_plan`描述漂移；Multica Codex调用方deadline的ready-file检查在干净3c7基线也失败。`make test`因Docker前置不可用受阻，替代Go guard使用隔离数据库。完整Go adapter包1475个顶层测试通过，另有2个顶层失败；Muse两子场景在基线及当前树定向通过，Codex既有失败保留。详细命令、原始退出码、补验及清理记录见本机 `.agents/acceptance/tomcat-multica-default-acp.md`。
- 没有安装、发布、推送、修改真实配置/PATH或重启正式服务。当前构建产物不等于用户正在运行的standalone/bundled二进制；正式升级须由用户另行安排。

### 2026-11-06：复审整改与提交边界

```text
保留恢复按钮现状 → 清理旧有人/无人口径 → 补丁恢复完整index
                                        ↓
                    50项聚焦检查 + 干净基线重放29项 → 7文件提交
```

- 用户决定不实施“错误卡恢复动作只由持久历史判定”，删除该 todo 及依赖它的测试、文档、界面验收要求。`agent_end` 实时恢复卡、历史重建、手动按钮、五分钟自动恢复及服务端最终校验均保持现状，13.3.2 说明未改；取消项不计为已实施。
- 清理残留旧口径：`agent-loop.md` 只改13.10节退避描述（普通退避统一30秒、Retry-After优先）；`core/README.md` 配置表改为默认10次、统一30秒并澄清历史提交不代表当前策略；`run_basic_test.rs` 重命名旧用例与断言、移除三个重试用例无关的 executing PlanRuntime 预置，billing 改用默认预算仍只发1次请求，stalled-plan 用例及用途保留。
- 两份接入补丁用 `git diff --full-index --binary` 在声明基线重建，9+7条 index 均为完整40位哈希；另一份干净副本 apply check/apply 成功，29个目标文件内容与 SHA-256、许可证全部一致，adapter-only 边界不变。
- 必要纠正：manifest 实际也记录两份补丁自身 SHA-256，原文“只有目标文件哈希”与事实不符。仅同步这两个校验值，29项 `host.files` 与其余字段不变；反向恢复后可得到与整改前完全一致的清单哈希。
- 聚焦复验：`cargo fmt --check` 与 `core::agent_loop::tests::run_basic_test` 50项通过（`1793955322007-ido3t0`）；补丁重放与目标哈希见 `1793955352036-uzmf8i`、包内 `VALIDATION.md`。
- 以执行前1759个 Tomcat、7308个 Multica 文件哈希快照核对，本轮仅7个既有文件变化；webview/恢复/五分钟逻辑及相关测试、历史文档、Multica 主工作树全部未改，一次性副本已删除。`tests/existing-files.patch` 两处原有空白告警仍单列，不称无排除的整仓检查全绿。
- 提交本身不重跑测试、不编译、不打包；不推送、不安装、不改真实配置或重启正式服务。

### 2026-11-06：CLI 0.1.87 / 插件 0.2.5 升版与整包

```text
bump --all patch → CLI 0.1.86→0.1.87 / EXT 0.2.4→0.2.5 / bundled 0.1.87
      ↓ release 二进制 + package:vsix
darwin-x64 整包（内含 bin/tomcat 0.1.87）
```

- 用 `scripts/release-version.mjs bump --all patch` 仅升级一次，让新 EXT 内置新 CLI；升级前、升级后 `check` 均通过，仅五个版本文件的版本字段变化，无依赖变更。
- 按用户要求打含内置 CLI 的 darwin-x64 整包；未构建 arm64/linux/win32，也未打不含 CLI 的纯插件包。产物在 `tomcat-vscode-ext/dist/`（已被 gitignore），不入库。
- 首次打包因 `dist/` 目录不存在而 vsce 报 ENOENT，建目录后重打成功；不把失败那次当作通过。
- 编译 release 二进制并打包属于本轮本地验证，不等于安装或发版；未跑测试套件、未安装、未打 tag、未创建 release。

### 🔌 INTERFACE (接口变更)

- `create_plan.draft` 的含义明确为 Goal 与自动待办区之间的完整 Markdown 正文；入参字段、类型、必填集合、权限、锁、文件格式与评审同意流程不变。普通标题不再降级，独立 Notes 章保留；只对开头为旧 Goal／Draft／Plan 包装的输入作兼容处理。
- `write-plans` 新增 `references/final-check.md`；主文仅导航，不留重复检查副本。恢复的两个措辞例子保留在 `writing-standards.md`，图示延续保存场景，四字段用例仍由结构附件负责。
- `core_identity.txt`、确认／授权规则与运行时保护未改；不实施 Planner–Worker 业务功能，不增加运行时审批、预算机制或新依赖。

### 验证记录与本次提交边界

以下是此前实施阶段已经完成的验证，不冒充本次提交或升版后的重新验收：

| 范围 | 已有结果 | 边界 |
| :--- | :--- | :--- |
| 提示词、catalog 与正文保存 | 分别33、21、13项通过 | 保存测试含真实写盘读回、长正文、标题／空白与旧包装围栏兼容 |
| 原文恢复后的技能验证 | 原始两组措辞例子逐字比较通过；33项提示词与1项正式物化／加载测试通过 | 任务 `1793878508306-mcumd7`；本次提交不重跑 |
| 其余 Rust 库检查 | 3410通过、1失败、6忽略；68项已聚焦覆盖的用例被过滤 | 任务 `1793876468133-yr6wc6`；失败见 BLOCKED，不称整库全绿 |
| 格式与严格 Clippy | 此前 `cargo fmt --check`、`cargo clippy --all-targets -- -D warnings` 通过 | 不将既有结果称为新版本编译验收 |
| 计划预览 | 此前43项聚焦 GUI 测试、TypeScript 与构建通过；桌面／窄屏 PNG、ARIA 与 console 已核对 | 未更新已安装扩展，本次不重跑视觉验收 |

复核为源码／差异自审，不是独立模型评审或长期生成质量实验。本地 `.agents/acceptance/` 记录、截图、原稿及备份不作为产品源文件强制纳入提交。

### ⚠️ BLOCKED (阻塞/风险)

| 项目 | 原因／影响 | 后续与边界 |
| :--- | :--- | :--- |
| 已知搜索 fallback 上下文失败 | 默认 Agent PATH 找不到 `rg`，既有 fallback 不提供前后上下文；此前及本轮原聚合命令失败 | 本轮仅在测试子进程PATH加入VS Code现有rg后原用例通过（`1793949497653-iw3rfq`）；不改持久PATH、搜索实现或断言，原聚合退出码仍按失败记录 |
| 覆盖率与新版交付 | 未测量覆盖率；此次仅提交源码和版本号，不重新测试、编译、打包、安装或发布 | 不编造 Cov%，不把版本升级等同于已安装或已发布 |
