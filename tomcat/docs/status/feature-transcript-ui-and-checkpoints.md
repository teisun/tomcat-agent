| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Nibbles | 2026-07-13 21:12 +0800 | ACTIVE | feature/transcript-ui-and-checkpoints | — |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** Checkpoint 最新一轮消失修复：`setCheckpoints` 不再触发 `rebuildHistoryTimeline`；分隔线改为 GUI 渲染层现算（`checkpointMarkers.ts` + `TranscriptView` checkpoints prop）@2026-07-13
- [✓] **[P0]** Checkpoint 分隔线缺失修复：`count_worktree_files_until` 改用 `git ls-files --cached --others --exclude-standard`，计数口径≡快照，尊重 `.gitignore` 与 DEFAULT_EXCLUDE_RULES @2026-07-13
- [✓] **[P1]** 分层验收：state/provider/webview_provider_flow + GUI checkpointMarkers 单测；shadow_git 忽略目录计数与 ignored-only 语义单测；E2E 库登记 VSCEXT-024/025；不含 CLI 的 `0.1.11` VSIX 已手工复验（gitignore，不入库）@2026-07-13
- [✓] **[P0]** Transcript checkpoint restore 落地：list/restore 协议、三态确认弹层、`revertFiles` 截断对话并可选回滚文件 @2026-07-13
- [✓] **[P0]** 上一轮进行态收尾：`TranscriptView` 仅 live cluster 持有 thinking streaming；`agent_idle` 经 `settleRunningTools` 收敛残留 `running/streaming` 工具卡；GUI/state/provider 分层测试 + E2E-VSCEXT-026 @2026-07-13
- [✓] **[P0]** Sticky reveal 触发加固：`useAutoScroll` 改按 `latestUserMessageId` 变化触发 reveal，护栏 `userMessageCount` 未减少；超一屏后切回 follow-bottom 并显示当前轮 sticky；分层测试 + E2E-VSCEXT-027；ext/gui bump `0.1.12` @2026-07-13

### 🔌 INTERFACE (接口变更)
- Webview store：`session.checkpoints` 与 `timeline` 解耦；timeline 仅含消息/工具项，checkpoint 分隔线由 GUI `injectCheckpointMarkers(timeline, checkpoints)` 现算。
- Checkpoint 文件计数：shadow git 上限计数改走 `git ls-files -z --cached --others --exclude-standard`，与 `git add -A` 快照口径一致；仅改 ignore 文件的一轮不产生新存档（设计行为）。
- AutoScroll：reveal 触发输入由 `lastItemIsLatestUser` 改为 `latestUserMessageId`；`agent_idle` 必须结算残留 running 工具卡。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 真机「新提示词置顶」仍可能失败 | `0.1.12` 已含 `latestUserMessageId` 触发加固，单测绿，但真机仍见不置顶；疑为 webview 运行时/时序，非触发条件本身 | 下一轮证据优先排查 |

### 集成说明
- 本分支目标：Transcript UI + checkpoint restore；本轮收口上一轮 live 动效泄漏，并加固 sticky reveal 触发。
- 验收分层：GUI util / TranscriptView / App / state / provider 单测；E2E 场景登记 VSCEXT-026/027 以分层覆盖替代真机。
- 已知后续：真机 reveal 到顶仍需证据优先定位（可能不是触发帧丢失）。
