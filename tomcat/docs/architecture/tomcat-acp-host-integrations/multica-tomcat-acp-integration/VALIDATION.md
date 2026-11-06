# Multica 资料包校验记录

```text
06835b8f5 精确基线 → 原接入23项 → 本次adapter目录能力/必要测试
                  → 合并后29项 → 两份patch重放 → 全文件SHA-256 → Go回归
```

本文件区分资料包重放、当前分支受控旅程与正式部署；不代替公司分支验收。

## 来源与范围

- 原基线：`06835b8f50606ca3f8bebbb5d07ac8901f831e69`。
- 原接入：`e12a45e59d8bfddf2aba192f163899a9eb290a3b`；原Tomcat依赖：`401b6f0cbd8be8478b5453815450f7ce6ada00e7`。
- 目录修复：Multica `3c7b4cde72ab4c8389dcec933e2573855b04cb7e` 的已审工作树；Tomcat `1f0fbf0c` 基线上的本次工作树，均未伪称已提交。`manifest.json.host.catalog_repair` 记录修复来源，逐文件SHA-256标识实际内容。
- 原接入先在精确基线上重建并核对23个哈希，再只叠加目录adapter和必要测试。原daemon/API/execenv/UI生产文件不带入后来版本；本轮新增生产差异只有 `models.go` 的Tomcat分支及新增 `tomcat_models.go`。用户启动器、正式凭据、配置与运行产物不在包内。

## 可复核方法

在声明基线的一次性副本中，从根目录执行两份补丁的 `git apply --check` / `git apply`，然后将 `new-files/` 及 `tests/new-files/` 按原路径复制。**不要对公司工作区盲目覆盖。** 对最终副本的每个目标文件核对清单，而不是仅核对新增文件：

```python
import hashlib, json
from pathlib import Path
bundle = Path('/absolute/path/multica-tomcat-acp-integration')
checkout = Path('/absolute/path/disposable-replayed-baseline')
m = json.loads((bundle / 'manifest.json').read_text())['host']
assert m['name'] == 'multica' and len(m['files']) == 29
for row in m['files']:
    assert hashlib.sha256((checkout / row['path']).read_bytes()).hexdigest() == row['sha256_after_integration'], row['path']
for row in m['patches'] + m['licenses']:
    assert hashlib.sha256((bundle / row['path']).read_bytes()).hexdigest() == row['sha256'], row['path']
```

在该副本执行README的Go guard聚焦命令；需要真实Tomcat时使用明确的本轮产物路径与 `agentintegration` opt-in。构建或哈希成功不等于公司分支测试成功。

## 本机已取得的证据（2026-11-06）

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
