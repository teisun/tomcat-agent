# 原位编辑用户消息

```text
用户气泡 → ComposerSurface（GUI 临时编辑态）
  无原生文件写入 → 直接 Keep
  有原生文件写入 → preview_rewind → Cancel / Keep Files / Revert Files
    → rewind_and_resend → cancel_and_wait → 可选文件恢复
    → superseded + 新 user → rehydrate → start_turn(None)
```

## 契约
- `rewind_and_resend {sessionId,messageId,files:"keep"|"revert",message:{text,segments?,attachments?,userMessageId?}}`。message 复用 Prompt 输入参数，不接收客户端截断索引或文件路径。成功沿用 `{accepted:true}`。
- `preview_rewind {sessionId,messageId}` 是只读查询，返回 `{revertAvailable,revertReason?,revertPaths}`；原因是 `no_baselines | expired | git_head_moved`。
- initialize capability `rewind_and_resend` 同时表示支持预览。旧 CLI 隐藏入口。
- 只编辑已提交、未作废的正常用户轮次；排队 follow-up、steering、系统信号、乐观消息不适用。取消弹框保留编辑稿；点外部/Esc 取消编辑，底部草稿不动。
- 编辑态不持久化。附件操作 `target:edit` 不进入底部 ComposerDraftStore；请求关联只用于异步回包，不是服务端幂等协议。

## 文件恢复边界
`<sessions_dir>/file-baselines/<session_id>/<user_message_id>/baselines.jsonl` 记录 `{path,backup,git_head}`。backup 为路径 SHA256 命名的改前原始字节文件，null 表示原来不存在。每轮首次原生 write/edit/hashline_edit 成功写入留一份；备份失败不阻止写入。只保留成功写入的清单，拒绝/失败工具不产生可回滚路径。

Files 与消息 Revert 共用这些按轮改前备份，但只展示最近一个实际修改过文件的有效轮次；纯问答保留，下一改动轮首次成功修改后整份切换。Files 的 ×／Undo All 只恢复请求指定轮次和路径，不改写聊天或重发模型。成功但无字节／存在性变化的首次写入不发布记录，之后真正修改仍能捕获。相对路径与实际写工具同源；恢复保留备份原权限。现有 `discard_turns` 和整轮 `prune` 保留，Build/Signal/Nudge 的写入归最近 Normal 用户消息，不新增 owner 机制。完整契约见 [session-file-changes](session-file-changes.md)。

恢复选中消息及后续有效轮次里每个路径最早的备份；AI 新建文件逐一删除，不用递归删除工作区。保留期复用 checkpoint.retention_days，HEAD 不同或无备份时禁用 Revert。个别备份缺失跳过。后续人工/终端修改若发生在同一路径会被覆盖，不做内容冲突检查；从未由原生写工具触及的路径不动。后台 Bash 不自动停止。ShadowGit/分隔条 restore 保持原用途，不作为缺备份的替代。

## 历史与失败
沿用 superseded，连同切点之后的 compaction marker/body 作废；前缀 marker 的迟到 body 保留。保留原行使附件引用继续存活。已作废输入不再可编辑。作废及新输入追加使用同一次原子改写，避免 append 失败导致源输入不可再用。索引是派生缓存，涉及 boundary 失效时必须使旧缓存失效，不能只刷新指纹后继续使用旧 boundary。

停稳超时不执行恢复/历史替换，不谎称旧任务已经停止。文件恢复部分失败保留历史与备份，允许重试；新消息提交后模型失败用现有 Retry。Host 接受成功、stale 或带 `committed:true` 的失败后整页重新取权威历史，不能 merge 已缓存旧页。已提交却未能启动新轮次的错误在会话时间线中展示，不依赖旧编辑器继续存在。

## 停止与失败的副作用
写工具的路径授权、敏感信息确认等待可取消；Serve 的确认桥在等待被丢弃时撤销自己登记的请求，并发出 `control_cancel`，迟到回复不再生效。落盘开始后不丢弃写入 future，须等待结果和备份登记完成，之后才可 Revert；compaction 等非提交等待在自身边界响应取消，不用整个推理 future 的取消竞速遮盖正在提交的写操作。CLI 同步 stdin 确认仍要等用户回答，本期未改。

`cancel_and_wait` 超时保留句柄、不强制 abort，故与 shutdown/cleanup 的超时语义不同。停止本身不清 steering/follow-up；只有 `rewind_user_message` 提交成功之后才清空已作废的后续队列，未提交失败保留排队输入。它在全局 FIFO 中等待，最坏等待停稳与 checkpoint flush 约 6 秒；实测 p95 超过 1 秒或多会话卡顿反馈时再考虑会话级任务。

## 附件的归档与模型版本
`InputImageRef` 使用 `blob_sha/mime_type/detail` 与可选 `provider_sha/filename` 表示一张图。SVG 原图保留在归档，模型收到 webview 转出的 PNG；hydrate 与 GC 直接读 part，不持久化整份前端 DTO 的旁路数组。首次发送和 hydrate 都仅在 provider SHA 不同于原图时按 PNG 解释，同一 SHA 保留原 MIME（例如 JPEG）。

Reference 历史投影将 `provider_sha` 改名为 `providerSha`，保留 `blob_sha` 用于缺失附件的不可用展示；Inline 投影物化原始图，移除新增归档字段维持原形状。普通旧记录没有这两个可选字段仍可读取；未发布的 `submitted_attachments` 格式不作兼容。Codex `protocol/src/models.rs::ContentItem`、opencode `session/message.ts::FilePart`、Continue `core/index.d.ts::ImageMessagePart` 都把图片元数据放在 part 内；若非图片也需要转换版本，再评估通用 rendition，不提前抽象。

## 判定与 UI 边界
气泡和执行命令共用 `SessionManager::is_rewind_candidate` 与排队 id 查询；前缀消息链完整性仍是后端提交前的防御性校验。捕获与预览认同一个 transcript header cwd，没有 cwd 不猜进程目录，只提供 Keep。

文件写入证据倒序扫描一次，供气泡 ↶ 和确认框共用；无写气泡保持原内边距，键盘聚焦入口保留。编辑时底部只禁止发送，停止按钮仍可用；两个 Composer 均要求有正文，未新增仅附件发送行为。

本期保留：编辑草稿 GUI 本地、Host obsoleteIds 快照、现有 target 校验与顺序工具调度；出现不同语义的 target 或并行写入再重评。未提交备份副本清理已取消；确认等待期间手改同一文件后再允许写入，仍可能被写入/回退覆盖。取样时机与确认后新鲜度复查不在本期范围。

## 本地调研与推翻条件
- VS Code：`vscode/src/vs/workbench/contrib/chat/browser/widget/chatWidget.ts` 的 `clickedRequest/createInput` 复用完整 ChatInputPart；`chatEditingSession.ts::_acceptStreamingEditsStart` 首次文件编辑记录 baseline。
- Continue：`continue/gui/src/pages/gui/Chat.tsx` 复用 ContinueInputBox；不照搬 `sessionSlice.ts::submitEditorAndInitAtIndex` 在 GUI 截断权威历史。
- Codex：`codex/codex-rs/app-server/src/request_processors/thread_processor.rs::thread_revert_response` 服务端停止再回退；其回退不恢复文件，不照搬进入编辑前先破坏历史的时序。
- 修订计划另对照 Cline `SdkController::editMessageAndRegenerate`、cc-fork `fileHistoryTrackEdit` 与 opencode `session/revert.ts`。

如明确要求跨窗口并发编辑/断连自动重发，重新评估并发及请求回执；如要求 reload 保留修改稿，再引入编辑草稿持久化；如要求终端副作用回滚，应使用现有整轮 checkpoint 而不是混入单文件基线；如用户要求保护 AI 写后手改，再评估写后 hash/确认提示。当前不引入这些机制。

## 验证
Rust 定向测试使用 rewind_and_resend / preview_rewind / file_baseline 前缀；共享历史变更回归 restore、Retry、resume hydration。集成捕获实际模型 messages 与临时工作区文件。UI 复用现有 VS Code devhost harness，验证完整输入功能、取消边界、两实例隔离、宽窄布局，并留 PNG、ARIA 与 console 证据。
