# Multica 资料包校验记录

```text
06835b8f5 精确基线 → 原接入23项 → 目录修复29项
                  → 本轮adapter/清理接线31项 → patch重放 → 全文件SHA-256 → Go回归
```

本文件区分资料包重放、当前分支受控旅程与正式部署；不代替公司分支验收。

## 来源与范围

- 原基线：`06835b8f50606ca3f8bebbb5d07ac8901f831e69`。
- 原接入：`e12a45e59d8bfddf2aba192f163899a9eb290a3b`；原Tomcat依赖：`401b6f0cbd8be8478b5453815450f7ce6ada00e7`。
- 历史目录修复：Multica `3c7b4cde72ab4c8389dcec933e2573855b04cb7e` 和 Tomcat `1f0fbf0c` 基线上的当时已审工作树，记录保留在 `manifest.json.host.catalog_repair`。
- 本轮 adapter：已提交 Multica `5e36721c1329372d272e8c4b72f4ada2e31115a7`，配套 Tomcat 源码 `9d7221317e272645128d090bc38151f943d6a2aa`；`host.adapter_refresh` 记录来源与语义回迁边界。
- 原 29 项先在精确基线上重放且全部哈希一致，再同步 adapter 与必要宿主接线/测试，最终 31 项。相对原包，24 个原目标文件哈希不变；新增 `daemon.go`、`taskfailure/failure.go` 两个补丁目标，并更新原 `agent.go` 的清理契约、`tomcat.go` 和三份测试快照。不导入整个新版宿主，不包含新版 Pi、桌面、调度或 native CLI 停机确认协议。

## 可复核方法

在声明基线的一次性副本中，从根目录执行两份补丁的 `git apply --check` / `git apply`，然后将 `new-files/` 及 `tests/new-files/` 按原路径复制。**不要对公司工作区盲目覆盖。** 对最终副本的每个目标文件核对清单，而不是仅核对新增文件：

```python
import hashlib, json
from pathlib import Path
bundle = Path('/absolute/path/multica-tomcat-acp-integration')
checkout = Path('/absolute/path/disposable-replayed-baseline')
m = json.loads((bundle / 'manifest.json').read_text())['host']
assert m['name'] == 'multica' and len(m['files']) == 31
for row in m['files']:
    assert hashlib.sha256((checkout / row['path']).read_bytes()).hexdigest() == row['sha256_after_integration'], row['path']
for row in m['patches'] + m['licenses']:
    assert hashlib.sha256((bundle / row['path']).read_bytes()).hexdigest() == row['sha256'], row['path']
```

在该副本执行README的Go guard聚焦命令；需要真实Tomcat时使用明确的本轮产物路径与 `agentintegration` opt-in。构建或哈希成功不等于公司分支测试成功。

## 本轮 adapter 同步证据（2026-11-09）

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

没有真实Key、公司仓库地址、数据库内容、node_modules、编译产物或宿主日志。PNG/ARIA/console与脱敏journey记录位于本机 `.agents/shots/multica-discovery/`，不纳入包。没有安装、发布、修改PATH或重启正式服务；本机macOS结果不代签Windows、公司二开分支或真实网关额度。正式升级必须更新该runtime实际绑定的CLI及daemon，并由用户安排共享服务重启。
