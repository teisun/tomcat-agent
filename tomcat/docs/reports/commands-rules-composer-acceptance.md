# Commands / Rules / Composer delivery evidence

```text
project .cursor / .agents
  commands + existing skills -> session catalog -> slash groups -> invocation chip
                                                          -> runtime body snapshot -> user
  rules (.md / .mdc, boolean alwaysApply:true) -> User Custom Instructions -> system

editing: occurrence IDs stay in drafts/webview; Serve keeps ordered duplicate parts
layout: controls 10px / content 15px; editor 30vh; attachments remain outside
```

## Scope and review

Implemented the executing plan `plan_commands_rules_cursor_composer_mdc_9f242064` using its revised contracts. Rules use the same strict boolean activation for .md and .mdc. Runtime instruction reads respect hard Deny without granting tool permissions or waiting on confirmation; agent read/load_skill tools retain their gate. Invocation content reuses InputReference, providers reuse to_prompt_text, and archival/provider construction shares one resolved body snapshot. Terminal operations keep their existing text route. No resource database, watcher, prepare/commit RPC, async preparation stage, runtime draft store or JS composer-height measurement was added.

The diff review found and corrected: catalog-only suggestions were not attached; a failed best-effort catalog refresh could misreport a committed fork/terminal action; old tests assumed content-level deduplication or omitted occurrence identity. CLI and extension docs describe the actual compatibility direction. Publish versions remain unchanged (CLI 0.1.64, extension 0.1.78). No commit, install or release was performed. Internal draft schema changed from 2 to 3 as planned.

## Verification ledger

Task IDs refer to this Tomcat session's audited command records. Successful earlier checks were reused; after fixture/assertion corrections only their owning test files were repeated.

| Boundary | Real outcome / evidence |
| --- | --- |
| Mid-work scanner | task `1791012600101-ucntc4`: six initial scanner tests passed; subsequent hard-enumeration bound and Deny-before-content case were included in the final library run. |
| Mid-work backend | task `1791013929789-yt4mls`: instruction catalog/body snapshot/Deny/steer rejection and project-rule refresh tests passed. |
| Rust library (planned R batch) | task `1791015351719-un5yr3`: **2991 passed, 8 failed, 3 ignored**. Initial failures were local HTTP request/timeout cases, not omitted from this ledger. The command exited 101 and its `&&` follow-on steps therefore did not run. |
| Seven of those eight failures | `1791017327164-ici20v` and `1791017497805-uxca2q`: all seven passed with loopback NO_PROXY and proxy environment removed. No source changes were made to obtain those passes. `scutil --proxy` showed enabled system HTTP/HTTPS/SOCKS proxies. |
| Remaining library failure | `core::tools::web_search::tests::web_search_sends_request_model_name_not_id` still times out. Its own EnvGuard explicitly removes NO_PROXY/no_proxy (`tomcat/src/core/tools/web_search/tests.rs:1250-1260`), so a shell override does not survive inside this test. The web_search/http_client modules have an empty diff. **UNVERIFIED: no unmodified-HEAD baseline comparison was run; system-proxy involvement is strongly indicated but this remaining failure is not claimed resolved or proven pre-existing.** Library coverage converges to 2998 passed, 3 ignored, 1 unresolved timeout, not a green whole-library command. |
| Rust integration targets / CLI build / generated protocol | task `1791017797135-m8fi7d`: prompt_size_budget (3) and system_prompt_cwd_priority (1) passed, cargo build --bin tomcat exited 0, check:wire confirmed up-to-date. Build retains a dead-code warning for the legacy build_user_message test helper. |
| Extension unit tests | `1791015351764-e66m3k` covered all original 42 files, with one newly added history fixture failure (wrong `entries` key). `1791015584567-aocx7i` passed the corrected state (115) and slash provider (6) tests; `1791016149515-udoi75` passed four added instruction transport tests. Combined current owning coverage: **576 tests passed**, with the rest of the full-run results unchanged. |
| GUI unit tests | `1791015584567-aocx7i`: 70 files passed, App had two old occurrence-contract assertions fail. `1791016149515-udoi75`: corrected App's 62 tests passed. Combined current owning coverage: **664 tests passed**. Expected jsdom canvas and existing React/Tiptap flushSync warnings are not visual evidence. |
| Focused extension integration | `1791017118949-ilbonv` passed contextReferences (10), real Serve slash (2), and new real Serve instruction (2) cases; three provider-flow assertions omitted occurrence metadata. `1791017658560-2us5dr` passed all 57 corrected provider-flow cases plus lint. Combined focused integration coverage: **71 tests passed**. |
| Actual model requests | New serve_instruction_prompt tests use a real Rust process with controlled OpenAI Chat and Responses endpoints. They inspect system rules and user command bodies, delete the source before retry, verify the original snapshot plus changed next-turn rules, and verify a failed new invocation does not write history. |
| True VS Code host | `1791015584759-6jia83`: seven selected host flows passed (real body-padding removal/alignment, sticky history, selection/history, duplicate drop, picker, @ file/directory); the first new invocation flow had a DOM-capture timeout. `1791017119020-u586zv`: invocation draft reload/send/history passed in a separate real host run. **The initial timeout is retained as a flake with unconfirmed root cause**, not silently erased. This host uses the repository's controlled fake Serve fixture; real Rust semantics are independently proven by the integration cases above. |
| Browser rendering | `1791014713409-2lnq4m`: 90 captures across 9 viewport/font/zoom scenarios, plus 9 question scenarios passed. Captures include PNG, ARIA, console and geometry JSON; no pageerror/console.error. Representative narrow/dark, desktop/light and 200% long-input PNGs were visually inspected through Playwright. |
| Type and whitespace checks | lint passed in `1791017658560-2us5dr`; check:wire passed in `1791017797135-m8fi7d`; git diff --check passed at close-out. |

## UI artifacts

- Before: `.agents/shots/commands-composer/before/`
- After: `.agents/shots/commands-composer/after/`
- Questions: `.agents/shots/commands-composer-questions/after/`
- Actual-host first run log artifacts: `/var/folders/1t/xdx9bpn14bb365np42rb36rw0000gn/T/tomcat-vscode-runs/gui-4fSJH8/`
- Actual-host invocation retry: `/var/folders/1t/xdx9bpn14bb365np42rb36rw0000gn/T/tomcat-vscode-runs/gui-bUiuiB/`

Examples of observed geometry: 390x844 after layout has surface left/right 10px, editor/button left 15px, suggestion width 260px, stream right edge 0px. In a 450 CSS-px viewport (simulated 200% zoom), long editor height is 135px (30%) and toolbar remains visible. Before, surface extended 6px past shell's right clip boundary and long input reduced stream height to zero.

## Explicit boundaries / deviations

1. An over-limit directory cannot be globally sorted without unbounded enumeration: read_dir has no sorted API. Discovery now hard-bounds the enumerated batch and sorts it; over-limit results are explicitly partial in diagnostics. Under the bound ordering is deterministic. This prioritizes the approved hard I/O limit over pretending to globally select the lexicographically first entries in arbitrarily large directories.
2. At 280px height with 11 attachments and long text, editor is bounded (84px) and toolbar is usable, but transcript may reach zero remaining height. This is the revised plan's explicit independent-panel policy; this delivery does not claim an always-visible transcript minimum under that extreme combination.
3. Rules globs/intelligent/manual activation and referenced-file expansion are not implemented. No frontmatter/quoted true never auto-activates a rule.
4. A new client reads/migrates old drafts; an old runtime cannot parse newly stored command/skill kinds. Keep the matching runtime for history replay.
5. The remaining web_search timeout and initial real-host DOM timeout are recorded above; no full-suite success is claimed.

## 2026-10-03 整改验收（计划 dbcf4da9）

```text
菜单红灯证据 → 自适应 CSS → CLI / reload / 快照入口清理
                                  ↓
             共享测试、真实 Serve follow_up、99 组浏览器证据
                                  ↓
             宿主整组复现 @ 时序失败 → 回归 / 修复 → 最终连续 3×8/8
```

本节是本轮最终代码的结果；上面的原始失败和警告记录保留，不用新结果覆盖历史。本轮没有重跑无关全仓套件，没有改发布版本、提交、安装或发布；上一轮 71 个已暂存文件保持暂存状态，本轮整改是其上的未暂存增量。

### 交付与共享验证

| 整改/边界 | 真实结果与记录 |
| --- | --- |
| 菜单改前红灯 | `1791036703121-yx0eh4` exit1，320×600/18px 字号，正文 180px=30vh；long-menu 顶部 **-27px**，被新增断言抓到。`.agents/shots/commands-composer/remediation/red/large-font-long-menu.*` 保留四类证据。它不是原稿推算的 -52px：本次真实菜单内容高 336px。 |
| 菜单改后与两条线 | `1791037510760-6h6yg1` exit0，最终 Composer 上的 9 视口 **99 组** PNG/ARIA/console/geometry，包括每视口 long-menu。320×600 long-menu 顶部 **7px**、底部309px≤surface.top316px−7px、高302px；390×280单行菜单高 **135.156px**（旧76px）、顶部7px。脚本确认菜单存在、正文确达滚动上限、矮面板不浪费空间，topbar/stream 两条线也有断言。 |
| 原生共享批次 | `1791037510626-a93gt1` exit0：system_prompt_test **36**、CLI commands **104**、serve commands_test **94**、project_instructions **7**，合计 **241** 个通过；prompt_cache_real_llm_tests `--no-run` 只编译成功（不消耗模型凭据），最终 `cargo build --bin tomcat` 成功。完整日志无 `warning:`，原 build_user_message dead-code 包装已删除。 |
| CLI / reload | 同一原生批次中，新的 `/help` 文案、空格 list ID/含单引号重名 ID 的生产格式化输出回放、真实 `/reload` 计数/相对资源前缀/Deny 来源原文/独立21条→20+1断言均通过。未改解析语法或权限判断。 |
| GUI / 配置 | `1791037510685-2cbk1q` exit0：Composer、SlashCommandMenu、slashMenu、App、App.sessionFrames 合计 **139** 个通过；layoutInsets **7** 个通过（非默认字符串4/24回退10/15），extension+GUI lint通过。既有 React/Tiptap flushSync 测试警告保留，不把DOM单测当视觉验收。 |
| 真 Serve 快照 | `1791038446321-6zjjkg` exit0：**3** 个真实 Rust Serve 集成用例通过，包含新增的忙态 follow_up。断言 queued:true、改文件时仅1个非标题模型请求、实际出队的第二个模型请求仍含旧正文且不含新正文；另两个 Chat/Responses prompt与retry用例也通过。未新增同目的Rust队列检查。 |
| 新索引回归 | 宿主失败调查发现 `findFiles` 等待期间 create/delete 会被旧列表回写清掉失效标记。`1791039008392-i9x6zv` 改前新回归 exit1，返回空结果；`1791039246131-etzqt0` 修复后 ContextSearch **12**、provider-flow **57**、lint通过。revision 防止旧快照成为有效缓存，最多为当前查询重建一次，不引入RPC静默重试。 |
| 最终宿主 | `1791039805500-yp836h` exit0，相关代码全部完成之后，同一固定过滤条件连续 **3 次各 8/8**、无目标跳过、每轮退出0。`1791040239697-iefmer` 核对三轮相同的8个测试名及结果；记录为 **本轮整组3/3，原调用标签DOM超时未复现、根因未确认**。 |

**本轮范围内的复现与处理：**第一序列 `1791038536683-iy1ybc` 为8/8、8/8、7/8，最后失败在新建 `@` 文件得到空匹配；第二序列 `1791039389947-a5ajvh` 为8/8、7/8，失败在新建 `@` 目录得到空匹配。这两组日志分别保存在 `remediation/host/` 和 `remediation/host-after-index-fix/`，没有删掉或混入最终3/3。除索引失效竞态的确定性回归修复外，文件和目录测试共用真实文件创建事件屏障、发现断言及失败诊断：不能在 VS Code 尚未观测到 Node 写入时就查询。最终三轮都记录了这两个 fixture 的确认日志。原调用标签 DOM 失联与本轮空匹配不是同一种证据，不把前者写成根因已修复。

### 可复制命令与产物

从仓库根执行的原生批次（临时代理环境仅用于本机 mock，未改系统代理）：

```sh
NO_PROXY=127.0.0.1,localhost no_proxy=127.0.0.1,localhost \
HTTP_PROXY= HTTPS_PROXY= ALL_PROXY= http_proxy= https_proxy= all_proxy= sh -c '
cargo test --manifest-path tomcat/Cargo.toml --lib system_prompt_test &&
cargo test --manifest-path tomcat/Cargo.toml --lib api::chat::commands::tests &&
cargo test --manifest-path tomcat/Cargo.toml --lib api::serve::tests::commands_test &&
cargo test --manifest-path tomcat/Cargo.toml --lib core::project_instructions::tests &&
cargo test --manifest-path tomcat/Cargo.toml --test prompt_cache_real_llm_tests --no-run &&
cargo build --manifest-path tomcat/Cargo.toml --bin tomcat'
```

固定宿主命令，在 `tomcat-vscode-ext/` 执行；最终成功证据是以下同一组连续三次，不是单项拼接：

```sh
TOMCAT_E2E_GREP='instruction invocation chip|default body padding|sticky user prompts|editor selections to the webview|repeated dropped file|smart picker|@ file search|@ directory search' npm run test:e2e:webview-devhost
```

- 最终完整宿主日志：`.agents/shots/commands-composer/remediation/host-final/run-1.log`、`run-2.log`、`run-3.log`。
- 对应独立宿主产物目录：`/var/folders/1t/xdx9bpn14bb365np42rb36rw0000gn/T/tomcat-vscode-runs/gui-mjxWTo/`、`gui-3NnW42/`、`gui-6PAAck/`。
- 最终浏览器产物：`.agents/shots/commands-composer/remediation/after/`；窄屏/桌面、大字号长文和280px面板 PNG 已检查，对应ARIA、console和geometry已读取。生产渲染脚本未发现pageerror/console.error；单独HTTP查看PNG时的favicon404是产物查看器请求，不是生产页面控制台记录。
- 最终浏览器命令：`node tomcat-vscode-ext/scripts/accept-resource-slash-ui.mjs --out .agents/shots/commands-composer/remediation/after`。
- 真实Serve命令：`npm --prefix tomcat-vscode-ext run test:integration -- tests/serve_instruction_prompt.test.ts`。
- 索引修复复核：在 `tomcat-vscode-ext/` 运行 `npx vitest run src/ui/webview/tests/contextSearch.test.ts && npm run lint && npm run test:integration -- tests/webview_provider_flow.test.ts`。

### 兼容性说明与重新评估条件

1. 菜单公式只承诺本次视口矩阵和 composer 贴着面板底部的布局。若输入框下面新增元素，或顶部剩余空间不足40px，必须重新评估定位/高度；不承诺所有尺寸永不越界，也不预先指定未来一定采用 anchor positioning。
2. 保留原有280px高+11附件时历史区可能为0的独立面板边界，没有重做附件/Todo/问题面板预算。
3. skill hover 显示 `Skill · 完整路径`，command 显示 `Command · 路径`，这是显示信息更完整，不改调用语义。
4. CLI 和VSCode保存同一种指令引用快照，但补充文本分隔不要求字节相同：CLI使用 `\n\n` 前缀另起一段，VSCode用composer中的普通空格/文本。
5. 已收 occurrence ID 集合按会话保留到webview卸载，不在发送时清掉；UUID唯一，保留集合阻止迟到消息复活，没有新增清理逻辑。
6. `web_search` 仍是**未解决、疑似系统代理相关、未做未修改代码的对照**；本轮未排查、未改该模块，旧证据等级不升级，不声称全仓测试已绿。
7. 执行锁解除、当前计划成为completed后，已用文件工具同步 `/Users/yankeben/.tomcat/plans/plan_commands_rules_cursor_composer_mdc_9f242064.plan.md` 的上述三处设计说明（hover的设计及测试文字均同步）；未改旧计划frontmatter或失败证据，Cursor原始整改文件保持不动。
