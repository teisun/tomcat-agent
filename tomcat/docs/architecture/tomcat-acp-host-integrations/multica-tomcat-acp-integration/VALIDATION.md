# Multica 资料包校验记录

```text
06835b8f5 精确基线 → 原接入23项 → 目录修复29项
                  → 上轮adapter/清理接线31项 → 本轮强度32项 → patch重放 → 全文件SHA-256 → Go回归
```

本文件区分资料包重放、当前分支受控旅程与正式部署；不代替公司分支验收。

## 本轮推理强度同步（2026-11-14/15）

- 原 Multica 基线不变：`06835b8f50606ca3f8bebbb5d07ac8901f831e69`。先重放原包并核对 31 个目标与许可证，再移入本次能力；新包 32 个目标，仅新增一个 `thinking.go` 补丁目标。新增文件与补丁目标不重复。
- 来源是 Multica `5e36721c` 和 Tomcat `bfea42a7` 基线上的**未提交工作树**，不是新提交；`manifest.json.host.reasoning_effort` 记录当前来源哈希及原基线测试适配。原基线没有新版的任务告警消息机制，保留日志告警/丢弃行为，四项选择测试放在已有 Tomcat daemon 测试中，不扩生产范围。
- 本轮检查记录在下表；历史结果仅保留追溯，不代替本轮。

| 检查 | 本轮实际结果 | 证据 |
| --- | --- | --- |
| Tomcat 会话/请求 | effort 库用例 27 通过；真实 ACP observer 4、stdio 27 通过；包含隔离、重启、Default、部分保存失败与实际 effort 参数 | `1794661649704-a8t1xf` exit 0 |
| Tomcat 默认门禁（终审前） | Clippy 严格检查、doctest（0 用例）、库其余 3417（7 ignored）、默认集成并行 399 / 串行 63 均通过；逐名审计确认当前 thinking/effort 并集 131/131 已有有效通过记录，不把两个筛选集的重叠项相加 | `1794662819175-m22fka` exit 0；`rust-library-coverage.json` 的 missing=[]；聚焦和 ACP 为 `1794659720205-aytheq` / `1794661649704-a8t1xf` |
| 终审子 Agent 强度归属 | 默认模型刷新后，只让当前会话模型继承会话档位，不改原模型路由；新红测先失败，再由整个 plan_runtime 153 项复验通过。fmt、all-targets strict Clippy、debug CLI 构建及反向依赖检查均通过；当前库列表 3556 项，其余仍有效结果复用 | `1794677740119-yh4ei1` exit 0；`subagent-default-effort/{README.md,red.json,green.json}`；红测采集器的错误 Off 预期未冒称通过 |
| 扩展门禁 | 分批覆盖 core 962、GUI 910；双端 TypeScript 与 check:wire 通过；最后一次 DevHost 重新构建当前 GUI/host | core 807：`1794663767107-y5gr33`；provider 100：`1794660323249-y5tqub`；plan 55 与 lint：`1794668228726-mwq0m3`；GUI 56+854、wire：`1794660323249-y5tqub` / `1794662872995-qbustd` |
| VS Code 真实旅程与视觉 | 直接从同一次 GetState 取模型与强度，避免侧栏切换或模型缓存落后的错配；DevHost 再次 1 通过，4 组最终 PNG/ARIA/console 已读回：任务开始 Low、计划预览 Low→手动 Medium，草稿保留；控件无重叠，窄屏按原样省略模型文字、ARIA 保留完整档位，采集范围 errors/events 为空 | 两项红测 `1794664198856-m2bknp` / `1794668126888-qf4ml7`；最终复验 `1794668228726-mwq0m3` exit 0；`devhost-20261115-r3/gui-8s2d68` |
| 原包核对 | 更新前 31/31 目标、两份补丁与两个许可证匹配 | `1794662109462-a04skx` exit 0 |
| 新包精确重放与回归 | 干净原基线上两份补丁 check/apply、32/32 目标及 2 个许可证匹配；四包 guard 聚焦 race、整个 Go module 构建及真实 ACP 五子场景全部通过（包括强度新建/续接/清空） | `1794662587945-eoz857` exit 0；本机 `bundle-replay.json` 保存 3 项退出码 0 |
| 当前 Multica 聚焦与类型 | Tomcat/thinking 的 guard race、真实跨仓 ACP、整仓 typecheck 通过；生产仍仅三个既定文件 | `current-20261115-r3` 的 focused-go / cross-repo-acp / full-typecheck 均 exit 0 |
| 最终浏览器旅程 | 1 passed（1.4m）；保存 High→新任务、Low→原会话续接、Default→原会话续接，三个任务均 completed，实际主请求 high→low→high；四项清理均 exit 0 | `1794673959317-ia8m5o` exit 0；`current-20261115-r11/{journey.json,provider-models.jsonl,commands.json,cleanup.json}` |
| 最终 Multica 视觉 | 1440×1000 / 390×844 共 13 张原始 PNG 逐图读取，ARIA/console 配套核对；Low/High 在窄屏视口内。无 console/page errors；20 条字体 preload 警告单列，102 个捕获资源响应无 4xx/5xx | `current-20261115-r11/visual-review.md`；与 DevHost r3 四图合计 17 图，不用 superseded 截图代签 |
| 其余 TS 单测 | core 2588、ui 326、web 292、desktop 837 通过；views 原批 6876 通过/4 超时，四条保持源码与 HEAD 一致后单独补验全部通过 | r4 分包日志；`1794673847481-2jqz6w` exit 0，`views-failure-check.log` 4 passed / 56 skipped；不把原失败整批改为 exit 0 |
| 最新全量 Go | 清除 28 个探测路径覆盖后重跑，regular 批次仍 exit 1（make exit 2）；repocache 1 项、handler 3 项失败，agent 半批未执行。随后独立完整 agent guard/race 子批 exit 0（478.317s），不改写原失败 | regular：`1794674310916-irf1r9`，四项清理成功；agent：`1794675314787-aobcpy`，`go-agent-full-clean/{test.log,result.json}` |
| Go 失败受控对照 | 原 HEAD 和当前分别使用全新已迁移私有数据库；四项均实际运行、0 skip。两个 dashboard exact-window 用例两边同样失败，确认不是本轮新增；repocache 和 webhook 两边均通过，原失败未复现、根因未确认 | `1794675880987-2z026p` 调查脚本 exit 0，但两组 Go 各 exit 1；`go-regular-failure-comparison/{baseline.jsonl,current.jsonl,result.json}`；私有 PostgreSQL/worktree/HOME 清理完成 |
| 中间快照重放（终审前） | 更新最后执行的浏览器源后，在另一干净原基线 check/apply 成功，32/32 目标、两份补丁完整 index、两个许可证、FILES 和全部来源哈希匹配；其他目标与已测 Go 副本相同 | `1794674592084-y08og4` exit 0；`bundle-final-replay.json`；manifest SHA-256 `6a26d1c1949e9a057ec73055acd1137820dfd0853f8a7c3f74d2ae25813339d8` |
| 终审交付重放 | 当前 32 目标、两份补丁完整 index、两个许可证、FILES、两仓来源哈希和全部交付 Go 文件的 gofmt 均通过；thinking.go 比旧 Go 测试副本只多一个独立空行，精确删除即可逐字节还原，其余非浏览器目标同字节，不冒称重跑 Go | `1794679465802-as08x3` exit 0；`bundle-delivery-verification.json`；当前 manifest SHA-256 `4582649f706400b5b4c9a0b627f75d38e51de8be9f53e74761fa3af9840cc8b4`；终审 Rust 修复的两份来源哈希已同步 |
| 其余浏览器 E2E | 最终串行补验实际完成：42 passed / 36 failed（37.9m），runner exit 1 / make exit 2；包含 MCP 按钮缺失、请求/断言失败、开发 overlay 拦截和超时。未做干净 HEAD 浏览器对照，不能一概归为既有失败 | `1794676082978-eulxlv`；`current-20261115-r13/{remaining-e2e.log,failure-summary.json}`；保留 28 份中途错误上下文，最终四项清理 exit 0、临时根已删除 |
| 最终 ZIP | 使用 `multica-tomcat-acp-integration-reasoning-20261115.zip` 新文件名，逐成员核对目录字节、CRC 与 SHA-256；结果以外置校验报告为准，不把旧快照当新包 | 本机 `.agents/acceptance/multica-reasoning/` 下同名 `.sha256` 与 `archive-verification.json` |
| 复核整改与重新打包 | Tomcat 档位显示名复用已有 `claudeEffortLabel`（`xhigh` 显示 Extra high，未知值仍首字母大写），与其他 Agent 一致；文档验收结论按已确认范围改写，失败明细不变。当前 Multica 聚焦 race 通过；干净 `06835b8f` 重放 32 目标、2 补丁、2 许可证、gofmt 一致，聚焦 race 与 `go build -p 2 ./...` 通过，临时 worktree 已删除。只改显示文字，浏览器旅程与真实 ACP 结果沿用上表 | 新 ZIP `multica-tomcat-acp-integration-reasoning-20261115-r2.zip` 及同名 `.sha256`、`archive-verification-r2.json`；上一版 ZIP 保留 |

首次扩展补余门禁 `1794662872995-qbustd` 的 core 命令误叠加两次 `--maxWorkers`，未启动测试；以同 runner 的直接 Vitest 调用补验 807，通过，未改产品代码。该整批 exit 1 保留，GUI 854 与 wire 子命令各自成功；不把整批改记为 exit 0。Multica 早期浏览器因菜单触发器名称、设置页入口和冷编译导航失败，原日志保留；最终 r11 已完成完整旅程，不再称最新旅程仍失败。r10 窄屏强度图未拍到菜单，最终脚本先滚到 Thinking 再打开菜单，并加入视口断言，r11 复验通过；这是测试采集修正，不是修改产品布局。

**本功能、受影响流程和原版本适配包已验收通过；Multica 全仓检查不是全绿**，以下失败与本功能无关（部分已由原 HEAD 对照复现，其余未逐项归因），原始结果保留，本包不代表全仓或公司部署验收：`make test` 的 Docker 前置在本机不可用；隔离 PostgreSQL+vector 已实际迁移并运行数据库测试，但不因此抹去原命令失败。首次全量 TS 的失败和超时保留，views 四个超时已聚焦补验；其余 E2E 首次因 `plugin-surface-document.spec.ts` 旧导出而收集失败，r4 扩大回归被主动中断。最终 r13 排除此收集失败文件后跑完 78 项，42 通过 / 36 失败，不能记为通过。原始断言包括 MCP Apps / Activity 按钮找不到、附件上传 404、日期/项目过滤断言、Next 开发 overlay 拦截点击及导航超时；Node 源码/清单/锁文件与 HEAD 无差异不等于这些失败已被基线证实。下一步需对照干净基线分组定位，未授权扩大修改其它产品路径来凑绿。r4 和 r13 的 managed/Tomcat/PostgreSQL/worktree 四项 finally 清理均成功；最初 provider readiness 失败残留已清理。Go 对照只确认两个 dashboard 失败在原 HEAD 同样存在；repocache 的 `stop process tree: operation not permitted` 和 webhook 的无可领取工作错误在两边定向复验均未重现，不宣称根因已解决。

本包不含 Tomcat 内部 Qwen 占位清理；不改 Multica 的 Qwen Code Agent，也不修改回复指引、`reply.md` 或 PLAN 权限。

## 来源与范围

- 原基线：`06835b8f50606ca3f8bebbb5d07ac8901f831e69`。
- 原接入：`e12a45e59d8bfddf2aba192f163899a9eb290a3b`；原Tomcat依赖：`401b6f0cbd8be8478b5453815450f7ce6ada00e7`。
- 历史目录修复：Multica `3c7b4cde72ab4c8389dcec933e2573855b04cb7e` 和 Tomcat `1f0fbf0c` 基线上的当时已审工作树，记录保留在 `manifest.json.host.catalog_repair`。
- 上轮 adapter：已提交 Multica `5e36721c1329372d272e8c4b72f4ada2e31115a7`，当时配套 Tomcat 源码 `9d7221317e272645128d090bc38151f943d6a2aa`；`host.adapter_refresh` 保留历史来源与回迁边界。本轮未提交的强度改动另见 `host.reasoning_effort`。
- 上轮原 29 项先在精确基线上重放且全部哈希一致，再同步 adapter 与必要宿主接线/测试，达到 31 项。相对当时原包，24 个原目标文件哈希不变；新增 `daemon.go`、`taskfailure/failure.go` 两个补丁目标，并更新原 `agent.go` 的清理契约、`tomcat.go` 和三份测试快照。本轮在这 31 项上接通强度，达到 32 项。不导入整个新版宿主，不包含新版 Pi、桌面、调度或 native CLI 停机确认协议。

## 可复核方法

在声明基线的一次性副本中，从根目录执行两份补丁的 `git apply --check` / `git apply`，然后将 `new-files/` 及 `tests/new-files/` 按原路径复制。**不要对公司工作区盲目覆盖。** 对最终副本的每个目标文件核对清单，而不是仅核对新增文件：

```python
import hashlib, json
from pathlib import Path
bundle = Path('/absolute/path/multica-tomcat-acp-integration')
checkout = Path('/absolute/path/disposable-replayed-baseline')
m = json.loads((bundle / 'manifest.json').read_text())['host']
assert m['name'] == 'multica' and len(m['files']) == 32
for row in m['files']:
    assert hashlib.sha256((checkout / row['path']).read_bytes()).hexdigest() == row['sha256_after_integration'], row['path']
for row in m['patches'] + m['licenses']:
    assert hashlib.sha256((bundle / row['path']).read_bytes()).hexdigest() == row['sha256'], row['path']
```

在该副本执行README的Go guard聚焦命令；需要真实Tomcat时使用明确的本轮产物路径与 `agentintegration` opt-in。构建或哈希成功不等于公司分支测试成功。

## 合并前清理安全补漏（2026-11-09）

- 新增回归在旧包上实测失败：fresh 重试未拿到 session ID 时丢失 `cleanup_unconfirmed`；`runTask` 返回后两个目录模式的 TMPDIR 内容已被删除。红灯任务 `1794212765994-i0awcu` 中 Go 检查退出 1（外层脚本因成功复现而退出 0）。
- 仅修两条既有路径：fresh 结果保留清理不确定性及自身空 ID，合并两次 usage；未确认时将现有临时目录执行锁留在 daemon，防止返回时和并行 GC 删除 TMPDIR，并在原诊断中报告路径。**锁只保证 daemon 存活期；退出后沿用旧宿主 GC 语义，不新增跨重启/孤儿进程保留承诺。** 正常完成仍按原路径释放并回收。
- 修复后的 daemon 聚焦 race 通过：`1794213108417-yj1ze9` exit 0，含原 fresh-retry 反例、两个目录模式的真实 fixture 文件保留、GC/Go finalizer、超出 handoff 和取消回报。仅 `daemon.go`、新增 daemon 测试目标及 runtime patch 的哈希变化，目标仍为 31 项；adapter 本体不变。
- 全新精确重放 `1794213272064-evn22h` exit 0：两份补丁 check/apply、31/31 目标、18 条完整旧/新 Git blob index、FILES 与许可证通过；与实际测试副本逐目标字节相同。manifest SHA-256 `b037a7097e7352fd06cf29b5107fc5a4866190bed64ad813398d72002131d9c9`，本机记录 `.agents/acceptance/merge-multica-20261109/bundle-replay.json`。
- 旧 ZIP 和下表历史结果不包含本次补漏，不能据此声称修复后的包已重新交付或公司分支已部署。

## 历史 adapter 同步证据（2026-11-09，非本轮重跑）

| 检查 | 实际结果 | 证据与边界 |
| --- | --- | --- |
| 原包重放 | 原 29 项在精确基线上哈希全部一致 | `1794208555871-ipv8gi`；只作为更新前基线证据 |
| 最终包精确重放 | 两份补丁 check/apply 成功；31/31 目标、11+7 条完整 index、许可证及 FILES 清单一致 | `1794209746670-a52vdu` exit 0；最终全新副本与实际运行测试的副本逐目标字节相同；末尾仅补默认 daemon 测试中的两目录文件保留回归，运行时代码及两份 patch 字节不变，交付时再次核对全部 31 个哈希；本机记录 `.agents/acceptance/multica-adapter-refresh-20261109/package-check.json` |
| adapter 定向 | 执行/恢复、参数早退四分支、本地超时非终态、配置失败不拒绝恢复通过 | `1794208715072-2xk70x` exit 0；四个测试函数均有实际 RUN/PASS，不把未匹配测试当通过 |
| 四包聚焦 race | agent、daemon、execenv、taskfailure 的目标检查分批通过 | `1794208987410-usjwpc` 的 agent/execenv/taskfailure 通过，daemon 的清理缺失/超时与共享终态检查通过；首次 daemon 包整体 exit 1，修正两处测试夹具后以 `1794209605754-2ie8io` exit 0 补验受影响两项，复用其余有效结果；不是四包全量测试 |
| 真实目录保留 | `runTask`→真实 fixture 进程→结果分类→文件/工作树 defer，两种目录模式均通过 | `1794209975096-ypagsh` exit 0；`in_place` 与 `worktree` 均保留子进程实际写入的文件和 AGENTS.md，worktree 不自动提交/删除，保留原失败原因与错误 |
| 导出类型影响面 | 重放后的整个 Go module `go build -p 2 ./...` 通过 | `1794209018834-2jafsl` exit 0；`go list` 确认 agent 的直接消费者为 CLI、daemon、execenv、handler；没有安装或启动应用 |
| 受控 ACP | 四个子场景通过：四策略、人工等待取消、目录 ID/default、执行/取消/恢复 | `1794209066858-umor5v` exit 0；私有 HOME、loopback provider、假 Key；只启停私有 fixture 服务，不是外部模型或公司分支验收 |
| 格式 | 改动 Go 快照 gofmt 干净，应用后源码与普通文件空白检查通过 | 初始检查 `1794209067299-m2l1ey` exit 0；最终包继续执行相同普通文件与应用后源码检查 |

首次 race 中的两处失败均保留：原包发现测试错误地用物理规范路径比较显式配置路径，改为按旧宿主 `resolveAgentExecutablePath` 的约定核对原路径与 Command；新增取消回报测试误把无 JSON tags 的内部 `TaskCancelAck` 当作 wire，改为捕获真实的 `branch_name/error_message/failure_reason`。依据分别为旧基线 `config.go:836-849`、`agents_probe.go:118-125` 和 `client.go:541-584`。不为迎合测试改生产路径解析或回报协议，也没有删掉诊断断言。

Tomcat 仓库直接对 `.patch` 文本执行 `git diff --check` 会把 unified diff 必需的上下文前缀（空格+tab、空白上下文行）报为空白问题，初始命令 `1794208987533-m68twv` exit 2。保留合法 patch 字节：普通文件用 `git diff --check -- . ':(exclude)**/*.patch'`，补丁则在精确基线实际应用后检查源码、gofmt、完整 index 与哈希；不把原始检查冒称通过。

本轮没有运行迁移数据库、浏览器旅程、Windows/Linux 原生旅程或整个 Multica/Rust 门禁；未测覆盖率，未安装、重启、升版或重打 VSIX/Multica Release。受控策略测试也不等同于真实模型的独立 Review→人工批准→执行全链路。

## 历史目录修复证据（2026-11-06，非本轮重跑）

| 层次 | 实际结果 | 边界 |
| --- | --- | --- |
| 原包重建 | 原23项在精确基线上全部哈希一致 | 不从当前整仓diff生成旧包 |
| 更新包精确重放 | 两份补丁在全新声明基线副本 `git apply --check/apply` 成功，29个目标文件及许可证哈希全部一致 | `1793947610136-268wsk`；相对原包仅一处生产文件修改及一个新增adapter |
| Opus整改后完整index重放 | 两份补丁由声明基线副本以 `git diff --full-index --binary HEAD -- <目标路径>` 重新生成；9+7条index均为40位完整哈希；在另一份全新基线副本apply check/apply成功，29个目标文件内容与哈希及许可证全部一致 | `1793955352036-uzmf8i` 退出0；本次仅index元数据变化，没有更改任何hunk或目标内容 |
| 更新后Go | agent、daemon、execenv三包Tomcat/模型上报聚焦回归通过，退出0 | 任务 `1793947378525-yn8kt5`；旧基线真实代码，不是仅编译当前仓 |
| 当前分支ACP | `TestTomcatSharedServeWithHermesClient` 两子场景通过，退出0 | `1793945998998-atx5ew`；loopback HTTP/假Key/私有HOME，发现前后会话清单一致。原先把既存sessions.json.lock误算成新会话的测试失败已改为前后比较，未忽略lock |
| 当前分支浏览器 | 实际创建页发现→选原ID→UI保存→服务端任务构造→daemon/ACP→本地provider完成；既有失败提示、刷新、手填/default通过 | `1793946867530-af28bb`，1 passed；没有route mock或页面生产改动 |
| 视觉/运行时 | 1440×1000和390×844的目录/零凭据/旧命令提示及保存详情，共7组PNG/ARIA/console已读取；无pageerror/console.error | 20条字体preload warning单列；被警告字体资源实取200 font/woff2，不是模型/脚本加载错误 |

本次完整index整改只更新 `manifest.json.host.patches` 下两份补丁自身的 `sha256`：补丁字节变更后校验值必须同步。`host.files` 的29个目标文件记录、哈希和其他清单内容均保持不变；不能把目标文件哈希与补丁文件哈希混为一谈。本次不改错误卡恢复逻辑，不重复既有五分钟或浏览器验收；Rust `cargo fmt --check` 及 `core::agent_loop::tests::run_basic_test` 的50项检查通过（`1793955322007-ido3t0`，退出0）。

浏览器环境使用独立 `/tmp/tcm-*` HOME、registry、workspaces、PGDATA/socket及非5432回环端口，PostgreSQL17和pgvector0.8.6只复用程序文件；没有连接正式数据库。`make up/env-exec/down` 使用同一封闭环境，非Tomcat CLI探测显式指向不存在的fixture路径。实际Tomcat二进制为本轮debug产物，SHA-256 `1a222edff7ba65a88925adeb8b31cb2cce2b78f4a98b7b572e8157be3e02ffe1`，非已安装副本。

创建的agent保存 `catalog-alias`，执行时本地provider实际收到 `alias-wire`（stream=true）；另一次 `default-wire` 非stream为标题。发现阶段计数为0。目录中的无凭据模型未展示；保存Key后显式刷新立即成功，无daemon重启。缺命令时复用创建页原提示，不新增toast或错误协议。详情页首次/刷新失败展示仍保持原状。

E2E源码面向记录的3c7b4cde7页面/API/测试fixture；旧基线页面没有倒入包，也未声称该浏览器脚本在旧页面上执行。旧基线的adapter、daemon上报和任务默认优先级由Go测试验证。

隔离api、web、daemon已通过同一环境的 `make down` 停止；私有Tomcat `serve stop` 返回对应instance stopped=true（`1793947629664-ijbcr8`），本地provider也已停止。最终清理任务 `1793950077152-m5n6sw` 已核对所有权并停止独立PG60283，删除本轮PGDATA/数据库、HOME/profile/registry/slot、三个临时worktree、nextest工具和缓存；八个隔离端口无监听，主工作树status未改变。证据及复现脚本保留于本机 `.agents/`；清理记录 `.agents/acceptance/tomcat-multica-cleanup.json`。没有按端口盲杀。`make status` 曾把本fixture的web标为mismatch；lsof/ps确认监听PID13596的cwd及父链属于该worktree，正常受管down已清掉该监听，不修改产品生命周期脚本。

最终跨包门禁没有虚称全绿：Docker前置不可用，使用隔离数据库下的既有Go CLI guard替代；Tomcat工具目录文档存在本轮未改的描述漂移，Multica的Codex deadline ready-file断言在原始3c7干净基线亦失败。其余已通过检查、环境/时序失败的补验以及16组最终功能视觉证据见本机 `.agents/acceptance/tomcat-multica-default-acp.md`。这不改变本表中资料包精确重放及目录/ACP聚焦检查的通过结果。

## 未包含与未代签

没有真实Key、公司仓库地址、数据库内容、node_modules、编译产物或宿主日志。本机本轮强度 PNG/ARIA/console 与脱敏旅程在 `.agents/{shots,acceptance}/multica-reasoning/`，不纳入包；上述历史目录修复证据的旧目录是 `.agents/shots/multica-discovery/`。没有安装、发布、修改PATH或重启正式服务；本机macOS结果不代签Windows、公司二开分支或真实网关额度。正式升级必须更新该runtime实际绑定的CLI及daemon，并由用户安排共享服务重启。
