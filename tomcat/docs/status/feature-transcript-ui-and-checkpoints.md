| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Nibbles | 2026-07-13 18:30 +0800 | ACTIVE | feature/transcript-ui-and-checkpoints | — |

### ✅ DONE (已完成/进行中)
- [✓] **[P0]** Checkpoint 最新一轮消失修复：`setCheckpoints` 不再触发 `rebuildHistoryTimeline`；分隔线改为 GUI 渲染层现算（`checkpointMarkers.ts` + `TranscriptView` checkpoints prop）@2026-07-13
- [✓] **[P0]** Checkpoint 分隔线缺失修复：`count_worktree_files_until` 改用 `git ls-files --cached --others --exclude-standard`，计数口径≡快照，尊重 `.gitignore` 与 DEFAULT_EXCLUDE_RULES @2026-07-13
- [✓] **[P1]** 分层验收：state/provider/webview_provider_flow + GUI checkpointMarkers 单测；shadow_git 忽略目录计数与 ignored-only 语义单测；E2E 库登记 VSCEXT-024/025；不含 CLI 的 `0.1.11` VSIX 已手工复验（gitignore，不入库）@2026-07-13
- [✓] **[P0]** Transcript checkpoint restore 落地：list/restore 协议、三态确认弹层、`revertFiles` 截断对话并可选回滚文件 @2026-07-13

### 🔌 INTERFACE (接口变更)
- Webview store：`session.checkpoints` 与 `timeline` 解耦；timeline 仅含消息/工具项，checkpoint 分隔线由 GUI `injectCheckpointMarkers(timeline, checkpoints)` 现算。
- Checkpoint 文件计数：shadow git 上限计数改走 `git ls-files -z --cached --others --exclude-standard`，与 `git add -A` 快照口径一致；仅改 ignore 文件的一轮不产生新存档（设计行为）。

### ⚠️ BLOCKED (阻塞/风险)
| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 新提示词置顶 + 上一轮进行态残留 | 已复现并写好整改计划 `fix_sticky_and_live-state`，待实现 | 下一提交 |

### 集成说明
- 本分支目标：Transcript UI + checkpoint restore；本轮收口“最新一轮消失 / 看不到分隔线”两处回归。
- 验收分层：GUI util / state / provider / webview_provider_flow / shadow_git 单测；E2E 场景登记以单元+集成分层覆盖替代真机。
- 已知后续：sticky reveal 到顶与上一轮 live 动效收尾（thinking spinner / Editing…）待下一轮修复。
