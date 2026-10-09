# 从上次 Keep 累积的 Files 面板

```text
AI 在 u1 改 A → 中断／继续／Build／Resume → 在 u2 改 B
         └──────────────────────┬─────────────────────┘
                         Files 同时列 A、B
                                 │
                         Keep All：只记时间点
                                 │
                 人工保存 A：不进入新周期清单
                 AI 再改 A：先备份当前内容，再修改
                                 │
               Files：本周期最早改前备份 ↔ 当前磁盘
               Undo：回到 AI 再次动手前；聊天不变
```

## 体验与范围

- Files 从最近一次 Keep All 起累积；没有 Keep 时使用会话内的新格式备份。普通消息、Build、Resume、打断、重试、只读工具及截图不清表。git 提交认可对应文件，但不重置整个范围。
- Keep All 在 Undo All 左侧，无确认框；运行中、操作进行中及空表时禁用。Keep 不改工作区、不复制文件、不写聊天，也不发模型请求。
- 只覆盖原生 write/edit/hashline_edit 成功改变字节或存在性时的记录；失败、拒绝、取消及 no-op 不发布。Shell/MCP/插件单独修改没有此保证。
- 写入仍归属最近 Normal 用户消息，不把 Build/Signal/Nudge 变成新 owner；只回退聊天时旧消息可重新成为 owner，旧页允许继续追加。
- 候选路径来自 AI 写入，但不是字符级归因：文件进入本周期清单之后，人工/终端保存同一文件也参与净比较。Keep 后仅人工修改不会入表；人工先改、AI 再改则以人工版本作为新基线。
- 点击文件打开完整只读基线与真实可编辑文件的 VS Code diff；未保存的编辑可在右侧出现，保存后刷新行数。二进制和超预算文本不伪造完整预览。

```text
┌ ▾ N Files                       [Keep All] [Undo All] ┐
│ src/example.ts                     +12 -4         ×  │
└─────────────────────────────────────────────────────┘
```

## 根因与不变量

旧设计按消息页的 mtime 判断修改先后，并假定较早页不会再写；只回退聊天后继续 Build 会同时破坏这两个前提。旧页新增 C 会让 A 的最早原件选错；Keep 后旧 `seen` 又会让 A 无法重新捕获。

两个固定回归位于 `tomcat/src/core/checkpoint/tests/session_files_test.rs`：`session_file_reactivated_owner_keeps_true_earliest_baseline` 必须仍取原稿，`session_file_reactivated_owner_after_keep_does_not_hide_new_changes` 必须看到再次修改及其新周期基线。

## 架构与接口

### 一份账本，两种读取方式

```text
file-baselines/<session>/
  u1/baselines.jsonl   A@t1(原稿), A@t4(认可后版本)
  u1/sha256-<hash>     按内容复用的改前副本；原权限随记录保存
  u2/baselines.jsonl   B@t2
  keep-t3/            空目录：只代表认可时间 t3

Files：at > t3，再按路径取最早记录 → A@t4
rewind：按消息历史、同页最早原件    → A@t1
```

- `file_baselines.rs::Baseline` 的 `at` 是必需的捕获毫秒时间；同一消息页内每文件每 Keep 周期捕获一次。副本用内容 sha256（加 `sha256-` 名称前缀）流式生成，避免重拍覆盖原稿；相同字节的路径仍各自保存权限。
- `TurnFileBaselines::new` 只从当前周期新格式行恢复 `seen` 和缓存 HEAD；`tool_dispatcher.rs` 在消息 id 或 Keep 改变时重建 tracker。Keep 被回退撤销也会让缓存失效。
- `session_files.rs::scope_rows` 保留同页重复路径的不同捕获，先按 `at` 过滤再选最早原件。页 mtime 不决定合法记录的范围或排序；仅语法损坏且 mtime 明确早于 Keep 的清单可忽略，非法 `at` 等新格式数据错误不吞掉。不扫描聊天正文。
- Keep 仅创建合法时间戳命名的空目录。没有快照、清单、发布锁、顺序头、全局序号或独立索引。仅用毫秒时钟跨越同毫秒边界；时钟回拨不靠伪造计数补救。

| 命令 | 作用 |
|---|---|
| `get_session_files(sessionId)` | 当前累计范围及 `sourceTurnId`（无记录/Keep 则 null） |
| `get_session_file_baseline(sessionId,sourceTurnId,path)` | 当前范围内该文件的完整基线 |
| `restore_session_files(sessionId,sourceTurnId,paths)` | 整体预检确切路径后逐文件恢复 |
| `keep_session_files(sessionId,sourceTurnId)` | 校验起点，创建时间标记并返回新起点 |

`sourceTurnId` 保持不透明 id：有 Keep 就是 Keep id，否则为最早有效记录的消息 id。动作必须校验它等于当前起点，过期返回 `unknown_path`，不能静默改用旧范围。客户端不能提供 backup 路径或恢复字节。扩展仍按会话缓存、合并刷新；来源 id 进入 diff URI，避免原件串用。

刷新沿用工具结束、目标保存、窗口聚焦、展开、Keep/恢复结果等入口，不按 token 查询、不增加 watcher。失败保留最后成功摘要并标错；正确接受同源空表。已从列表消失的路径可由聚焦/展开等入口重新比较。

### 提交即认可与恢复

文件在记录 HEAD 与当前 HEAD 之间被提交修改过，就以当前提交版为基线；否则用改前备份。按旧 HEAD 分组查询 `git log`，不能只比两端 blob，否则提交后 revert 会误判。读取使用批量对象存在性检查及逐文件 `cat-file --filters`，避免过滤后长度与 batch 原始大小不一致，并遵守换行/smudge 规则。非 Git、仓库外及不适合 batch 的路径用备份。

| 操作 | Files 行为（路径已在当前周期时） |
|---|---|
| 提交，包括 git revert 产生的新提交 | 当前提交版与磁盘相同则消失，再改后以提交版出现 |
| `git revert --no-commit`、`reset --soft` | 按新磁盘/HEAD 状态重新比较，可能重新出现 |
| 丢弃未提交修改、stash、`reset --hard` | 回到所选基线则消失；stash pop 后有差异可重现 |
| 手动改回原样、Files Undo | 与基线相同则消失 |
| 恢复文件的 rewind/checkpoint | 回退到 Keep 之前时撤销之后的 Keep 标记 |
| 只回退聊天、Retry、Resume | 保留备份和 Keep，不把磁盘未撤回的修改藏掉 |

×/Undo All 仍确认整文件覆盖、新建文件删除、聊天保留；只发可恢复子集并说明跳过数。前台 AI/维护忙或目标编辑器有 dirty buffer 时拒绝恢复；不自动保存或停止后台 Bash/其他会话，不新增冲突票据。

Files 不再因 HEAD 移动禁用撤销；rewind 自己的 `git_head_moved` 闸不变。路径/符号链接/特殊文件检查保留，原子写回并恢复记录权限；精确删除文件而不删父目录。多文件不是事务，中途错误报告具体路径。

## 生命周期与旧数据

- 真实恢复文件的 rewind 用完后删除对应消息目录；checkpoint 截断及 Retry 不清除消息备份。活跃会话保留整套备份，整个会话闲置达到 `retention_days`（默认7天）或记录已删除才清理。
- 撤销 Keep 只删除空标记目录，遗留快照或意外内容不递归删除。原件缺失时显示 `backup_missing`，不借其它记录补位。
- **不兼容旧文件备份**：缺 `at` 的行不用于 Files、tracker、文件 rewind；没有 mtime 兜底、回填或迁移。带快照/清单的旧 Keep 目录不识别。旧数据不主动清理，聊天及工作区文件不删除；旧文件清单和依赖旧原件的撤销能力不保留，新 AI 写入生成新格式记录。

## 参考与推翻条件

- VS Code 的 `chatEditingSession.ts::_getOrCreateModifiedFileEntry/accept/reject` 支持跨请求审阅；`chatEditingCheckpointTimeline.test.ts::navigating between multiple requests` 验证历史导航。其 `chatEditingTextModelChangeService.ts::keep` 更新内存原件；Tomcat 不照搬成磁盘全量快照，而是在下一次写入前捕获。
- Codex 的 `codex-rs/core/src/turn_diff_tracker.rs::TurnDiffTracker` 和 `turn_diff_tracker_tests.rs::accumulates_add_then_update_as_single_add` 体现首次原件与净变化；它的按轮 diff 是回执，不是 Tomcat 常驻累计清单的边界。
- OpenCode 的 `packages/opencode/src/session/summary.ts::computeDiff` 通过私有快照计算单次请求变化；用户 Git HEAD 与私有备份不能混作同一份历史。

记录时间假设系统时钟可排序、同一会话没有多个 AI 同时写入，Keep 走空闲忙闸。时钟回拨、多写者、逐文件 Keep、字符级 AI-only 撤销或事务式整组恢复需要另行设计；不预先恢复已撤掉的全局锁/票据/序号。只在实际性能证据出现后考虑可重建缓存。当前周期缓存 HEAD 的既定取舍仍保留，不改成每文件重读 HEAD。

## 测试与验收分工

- `tomcat/src/core/checkpoint/tests/{session_files_test,file_baselines_test}.rs`：两个旧页复现、Keep 不复制、人工修改边界、内容复用与权限、旧格式忽略、同毫秒与 Git/revert/保留期。
- `tomcat/src/core/agent_loop/tests/tool_media_batch_test.rs::file_baseline_dispatcher_rebuilds_for_keep_changes_only`：同一真实 agent 下新增/撤销 Keep 均重建 tracker，无变化则复用。
- Rust Serve/rewind/checkpoint 测试与 `tests/serve_session_files.test.ts`：真实接口、消息历史与文件字节，Keep 不发新聊天/模型请求。
- VS Code devhost：原生完整 diff、dirty 保护、Keep 收起与再次出现；`scripts/accept-session-files-ui.mjs`：宽窄矮视口、PNG/ARIA/console/geometry。模拟 host 不代替真实后端；完整验收以活动计划闸门报告为准。


## 工具图片的历史身份契约（阶段1 A；B 之后只保留兼容）

```text
工具截图 / read 图片 → role=user, kind=tool_media → 模型照常看图
                              │
                              └─ 不是你的普通输入
                                  不抢文件归属、不显示用户气泡
                                  不成为 Retry/编辑重发目标

用户自己发的纯图片 → role=user, kind=normal → 保持用户消息行为
```

### 需求背景

`role` 是模型接口要求的传输角色，不足以表达“是谁发的”。旧生产点使用 `user_with_parts`，把工具媒体误标成 Normal；同一张截图因而被文件归属、历史气泡、恢复按钮等逻辑当作用户新输入。

### 解决方案

#### 方案讲解与关键决策

- 所有合成的 user 角色消息必须带非 Normal 的 `kind`，工具媒体使用 `ChatMessage::tool_media(parts)`。
  <small style="color:gray">`tomcat/src/core/llm/types.rs`、`tomcat/src/core/agent_loop/tool_dispatcher.rs`：生产点从普通多模态用户构造器改为 ToolMedia；用户自己的附件入口不变。</small>
- A 只修正身份，不改变三家 API 的请求形状、reasoning replay 窗口和压缩轮边界。
  <small style="color:gray">ToolMedia 的 `is_normal=false`、`is_replay_input=true`、`starts_logical_turn=true`；后三种含义不可混为“真实用户新轮”。</small>
- 历史渲染和恢复查找跳过 ToolMedia，但仍把媒体留在模型上下文。
  <small style="color:gray">`src/ui/webview/state.ts`、`tomcat/src/core/session/append_message_chain.rs`、`tomcat/src/core/session/manager/session_impl.rs`：完整工具结果后带媒体仍可 Resume；以媒体 ID 为 Retry 目标返回 `retry_target_stale`。现有 Resume 不携带用户目标 ID，Retry 才携带。</small>
- 不根据“内容是否只有图片”猜身份，也不自动迁移没有 kind 的旧载体。
  <small style="color:gray">旧版用户纯图片消息也没有 kind，猜测会误伤真实输入。CLI 与扩展需配套升级；历史 ToolMedia 保留为合法兼容形态。</small>

#### UI 效果

```text
改前：工具卡 → 多出一条空白“用户消息” → AI回答
改后：工具卡 ───────────────────────→ AI回答
```

### 参考与推翻条件

本次直接核对了三个兄弟仓库：

- OpenCode：`../opencode/packages/opencode/src/session/message-v2.ts:147` 的 `supportsMediaInToolResult` 与 L301/L389 的转换，把工具 attachments 在需要时转换成模型请求中的合成 user。
- Cline：`../cline/sdk/packages/llms/src/providers/middleware/split-tool-images.ts:407` 的 `splitToolImagesMiddleware.transformParams` 只改出站 prompt；L384 添加合成 user，而不是把传输角色当成人工输入身份。
- Pi：`../pi/packages/ai/test/openai-completions-tool-result-images.test.ts:78` 的 `batches tool-result images after consecutive tool results` 明确验证输出角色为 `user, assistant, tool, tool, user`，图片放在整批工具结果之后。

这些实现支持将“工具媒体属于工具”与“发送时可能用 user 角色”分开。A 曾保留传输形状修正身份；B 已停止新建 ToolMedia，新工具媒体按以下契约归属工具。已有 ToolMedia 继续读取、发送并保持旧轮边界；不迁移旧行。新的合成消息生产点仍必须明确身份，不能只在 UI 隐藏。

### 测试用例清单

- `tomcat/src/core/agent_loop/tests/tool_media_batch_test.rs`：内存与落盘 kind、批量工具顺序、截图前后写入归属、真实用户纯图片防误伤。
- `tomcat/src/core/llm/tests/types_test.rs`：serde/from_persisted 与身份/窗口谓词。
- `tomcat/src/core/session/tests/append_message_chain_test.rs`、`tomcat/src/api/serve/tests/rewind_and_resend_test.rs`：Resume 尾部识别、Retry/rewind 拒绝工具媒体、普通输入不被媒体遮蔽。
- `tomcat/src/core/session/tests/user_message_sidecar_test.rs`、`tomcat/src/core/compaction/tests/machine_block_test.rs`：工具媒体不进入用户原话。
- 三家 wire 测试验证内容相同的 Normal/ToolMedia 请求 JSON 等价：`tomcat/src/core/llm/tests/openai_provider_test.rs`、`tomcat/src/core/llm/openai_responses/tests/openai_responses_test.rs`、`tomcat/src/core/llm/anthropic/wire.rs`。
- `src/ui/webview/tests/state.test.ts` 与 `src/test/suite/support/hostE2eScenario.ts::assertWebviewPersistedMessageKindFlow`：工具媒体不渲染、真实图片渲染、出错可 Resume、真实 webview 刷新后不多出用户气泡。

## 工具图片归属与预览（B）

```text
工具结果 = 文字 + 图片/PDF
  ├─ provider内存：内联part，不自动上传file_id
  ├─ transcript：tool文字 + 图片blob引用（PDF沿用文件part）
  ├─ live事件：media引用；刷新历史：同一图片引用
  └─ 出站：Anthropic/Responses原生；Chat只在发送时批末拆user

卡片：[Read photo.png] [图片 1] ▸
展开：固定占位 → 补好缩略图 → 点击才进入原图预览
```

### 需求背景

工具截图必须与产生它的工具一一对应；只贴身份标签虽避免假用户气泡，却无法在工具卡重载后继续看图。用户进一步选择保留file_id实现、正常工具附件不走上传，避免当前中转站上传404阻断。

### 解决方案与边界

- dispatcher复用现有AttachmentBlobStore与archival/provider双份写法；主/子agent共用agent级图片仓库。GC将subagent-sessions纳入引用扫描，缓存键与主会话分开；不加新数据库、租约或发布协议。
- 工具消息本身不启动新的逻辑轮；replay先按内部历史计算，再拼临时wire媒体。旧ToolMedia仍保留历史语义。压缩与持久化文字重写保留图片引用，摘要仍能读取工具文字。
- `ToolExecutionEnd.media`是带类型的引用数组，无图片则省略；扩展实时/历史都使用共享附件解析。附件ID用toolCallId和媒体序号，避免实时数组无文字而历史数组含文字造成ID漂移。
- ToolRow有图默认折叠，标题旁codicon-file-media与数量；展开复用AttachmentStrip。图片不可用显示既有不可用提示；无thumb不拿fullUri顶替。nextThumbnailTarget纳入工具，仍一次一张、按sha去重。
- 预览纯函数纳入`Tool images · <tool>`分组，普通工具、单工具上下文与ThinkingGroup都透传点击回调；预览filmstrip也不使用原图兜底。
- 内联大小上限、用户模型配置与file_id兼容能力不扩大；raw探针的上传404如实保留。具体API矩阵见`tomcat/src/core/llm/README.md`。若原生API契约发生变化，先补探针再调整集中策略，不靠重复请求猜能力。

### 测试用例清单

- Rust：`tool_media_batch_test.rs`、`tool_exec/media.rs`、`api/serve/tests/attachment_test.rs`覆盖工具归属、inline/引用双份、恢复、子agent、无上传与get_messages不带bytes；schema夹具验证media字段。
- GC/压缩：`manager/tests/attachment_test.rs`、`compaction/tests/layer0_cleanup_test.rs`；三家wire和replay测试覆盖原生/拆分/保留file_id转换、旧载体、非视觉与DeepSeek/Kimi回放。
- 前端：`state.test.ts`、`ToolRow.test.tsx`、`thumbnailBackfill.test.ts`、`imagePreviewSections.test.ts`验证稳定身份、占位、不加载原图、补缩略图及预览分组。
- 真实VS Code：`hostE2eScenario.ts::assertWebviewToolImageThumbnailFlow`验证缩略图→原图→重载，保存PNG/ARIA/console；真实三API模型使用同一e2e_5验证识图、文件归属及tool引用落盘。
