# Files 与 Keep：AI 改动的累计审阅、认可与撤销

> 本文讲 Files 面板和 Keep 背后的原理，面向第一次接触这块代码的读者。面板 UI 契约、serve 命令清单与验收分工以扩展侧 [`session-file-changes.md`](../../../tomcat-vscode-ext/docs/architecture/session-file-changes.md) 为准；文档地图见 [`README.md`](./README.md)。

一句话：**Files 列出"从上次 Keep 到现在，AI 改过的文件"，每个文件拿"AI 第一次动它之前的样子"和"磁盘上现在的样子"比；Keep 只是在时间线上画一条线，意思是"之前的都认了，从这里重新算"。**

```text
时间线 ───────────────────────────────────────────────────────────►

  消息 u1           消息 u2                 [Keep]       消息 u3
  AI 改 a.ts        AI 改 a.ts、b.ts          画一条线      AI 改 b.ts
     │                 │                      │             │
     ▼                 ▼                      ▼             ▼
  登记 a(原件v0)      登记 b(原件w0)       建空目录      登记 b(原件w2)
                     a 已登记过，跳过      keep-<时间>    （线后第一次动 b）

  Keep 之前 Files 显示：a（v0 ↔ 现在）、b（w0 ↔ 现在）
  Keep 之后 Files 显示：b（w2 ↔ 现在）      ← 只看线后面的登记
```

## 1. 需求背景

AI 一次任务常常跨好几条消息改文件：普通对话、Build 执行计划、中断后 Resume、Retry 都可能继续写。用户需要一个固定的地方回答三个问题：

- AI 到底改了哪些文件、改了多少？
- 这些改动我认了，怎么一键"翻篇"？（Keep）
- 这个文件改坏了，怎么退回 AI 动手前的样子？（Undo）

所以 Files 的统计范围是"从最近一次 Keep 起累计"。普通消息、Build、Resume、打断、Retry、截图等都不会清空列表，只有 Keep 会。

旧设计按"消息页"的修改时间判断先后，并假设旧消息不会再被写入。但"只回退聊天、再继续 Build"会让旧消息重新变成写入归属，两个假设同时失效，出现过两个 bug：撤销退到了错误的版本；Keep 之后继续修改，Files 漏掉了文件。本方案就是为根治它们而重写的。

**名词小词典**

- **原件（基线）**：AI 在这一轮周期里第一次动某个文件之前，这个文件的内容。Files 和 Undo 都拿它当"改动前"。
- **登记行**：每抓一份原件，就往账本里写一行记录。
- **备份页**：按归属消息 id 命名的目录，里面放这份归属期间的登记行和原件副本。通常归属普通用户消息；压缩后的兜底归属 AI 回复。
- **Keep 线 / 周期**：一次 Keep 在时间线上画一条线；两条线之间叫一个周期。

## 2. 解决方案

### 2.1 方案讲解

#### 第一步：AI 写文件之前先存原件

```text
AI 要写 a.ts
   │
   ├─ 本周期已登记过 a.ts？ ── 是 ──► 直接写，不再登记
   │
   └─ 否 ──► 1. 把 a.ts 现在的内容复制一份当原件
             2. 账本记一行：{ 路径, 原件编号, 当时的 git 提交, 时间 at, 权限 }
             3. 然后才真正写入
```

可以想成"装修前先拍照"：每个房间只在第一次动工前拍一张，之后怎么改都不重拍。

几个细节：

- 只有原生写工具 `write` / `edit` / `hashline_edit` 会登记；Shell、MCP、插件直接改文件不登记。
- 写入失败、被拒绝、被取消、或者写完字节没变（no-op），这一行不落盘。
- AI 新建的文件，原件记为"原来不存在"；Undo 时就把它删掉。

#### 第二步：账本长什么样

```text
file-baselines/<会话id>/
  ├─ u1/  baselines.jsonl     ← 登记行，一行一条
  │       sha256-xxxx         ← 原件副本，按内容命名，相同内容只存一份
  ├─ u2/  baselines.jsonl
  │       sha256-yyyy
  └─ keep-1760000000000/      ← 空目录，就是那条 Keep 线（名字里是毫秒时间）
```

登记行优先归到内存中最近一条普通用户消息的备份页下；上下文里已没有它时，沿用本轮已有归属，再不行就归到本批工具调用所在的 AI 回复。归属在这条 AI 回复落盘之后才挑选，所以整段压缩把内存清到只剩摘要时，第一批写入也有归属。归属选择不倒读聊天记录；有普通用户消息时，Build、系统提示等合成消息不抢归属。rewind 挑选目标起所有消息命名的备份页，因此 AI 回复作为兜底归属时也能还原。

#### 第三步：Files 列表怎么算

```text
读出所有登记行
   │
   ▼
只留时间晚于最近一次 Keep 的行
   │
   ▼
按时间排序，同一个文件只认最早那行（那才是 AI 动手前的真原件）
   │
   ▼
逐个拿"原件"和"磁盘现在的内容"比较
   ├─ 一样 ──► 不显示（比如 AI 改了又改回来）
   └─ 不一样 ─► 显示，并算出 +行 / -行
```

**为什么每行都要自带时间，而不看"它属于哪条消息"？** 重试、Build 续跑可能把新登记写进一条旧消息的备份页，"消息顺序"不等于"真实发生顺序"。每行自带的 `at` 才可靠。旧设计正是在这里栽了跟头。

#### 第四步：Keep 做了什么

Keep 只做一件事：**新建一个空目录 `keep-<当前毫秒>`**。不复制任何文件、不打快照、不改工作区、不写聊天、不发模型请求。

```text
点 Keep 前                          点 Keep 后
┌─ Files ─────────────┐             ┌─ Files ─────────────┐
│ a.ts   +12 -3       │   ──Keep──► │  （空）              │
│ b.ts   +5  -0       │             └─────────────────────┘
└─────────────────────┘
       线前的登记全部不再统计（只是不算了，备份并没有删）
```

**为什么不用打快照？** Keep 之后，AI 下次动某个文件时，第一步会自动把它当时的内容存成新原件。从没被动过的文件本来就不该出现在列表里，提前给所有文件拍照是多余的。

**顺带的好处：保护你的手动修改。**

```text
[Keep] ── 你手动改 a.ts（v1→v2）── AI 改 a.ts（v2→v3）
                                     │
                         AI 动手前抓到的原件是 v2（你改过的版本）
                                     │
                         Undo ──► 回到 v2，你的手动修改保留
```

为了让"Keep 之后同一份归属里也会重新抓原件"，写入追踪器在两种情况下重建：归属消息改变，或者 Keep 发生了变化（新增或被撤销）。同一轮中途压缩掉普通用户消息时沿用已有归属，不会因新工具结果不断换页。

#### 第五步：Undo

Undo 就是把该文件的原件（第三步里"最早那行"）写回磁盘，同时恢复原来的权限；原件是"原来不存在"的就删掉文件。

两个前提：AI 当前空闲；目标文件在编辑器里没有未保存的修改。不满足就拒绝并提示，不会替你自动保存或停止任务。

#### 第六步：git 提交等于认可

```text
AI 改了 a.ts（登记时 git 在提交 X）
        │
你 git commit 了 a.ts（现在 git 在提交 Y）
        │
刷新 Files：发现 a.ts 在 X 到 Y 之间被提交过
        └─► 原件换成"Y 里的 a.ts"来比较
            ├─ 提交后没再改 ──► 一样，从列表消失
            └─ 提交后 AI 又改了 ──► 只显示提交之后的差异
```

你把它提交了，就等于认可了这些改动，不需要再点 Keep。注意提交只认可"被提交的那些文件"，不会重置整个列表。

判断"被提交过"用的是 `git log X...HEAD -- 路径`，而不是只比较两端的文件内容。否则"提交一次、再 revert 提交回去"会因为两端内容相同被误判为"没提交过"。

#### 第七步：回退时 Keep 也要跟着撤销

```text
u1 ── u2 ── [Keep @ t5] ── u3
       ▲
       └─ rewind 到 u2，并且把文件也恢复了
          ⇒ 磁盘回到了 t5 之前的样子，这条线已经没有意义
          ⇒ 删掉 keep-t5 目录，Files 退回按上一次 Keep 来算
```

- 会撤销 Keep 的：rewind 并恢复了文件；恢复 checkpoint 时把文件也恢复了。
- 不会撤销 Keep 的：只截断对话、不动文件，因为磁盘没变，那条线依然成立。

#### 第八步：同一本账，两种读法；备份什么时候删

```text
file-baselines/<会话>/
  u1/  a@t1(原件v0)   a@t6(Keep 后重新抓的 v2)
  u2/  b@t2
  keep-t5/

Files  ：只看 t5 之后，按路径取最早 ──► a@t6   （从 Keep 开始算）
rewind ：按消息找，不管 Keep     ──► a@t1   （能退回最初版本）
```

正因为 rewind 不看 Keep、要能找回最初的版本，所以 **Keep 和 Undo 都不删备份**。备份只在两种情况下删除：

- rewind 恢复了文件并截断目标起的消息：这些消息（不限 user 角色）对应的备份页跟着删；目标前的页不动。只回退聊天、不恢复文件时保留备份。
- 整个会话闲置超过保留期（`checkpoint.retention_days`，默认 7 天），或会话被删除：整个会话的备份一起清理。

### 2.2 UI 效果

```text
┌ ▾ 2 Files                       [Keep All] [Undo All] ┐
│ src/a.ts                           +12 -3         ×  │   ← 点文件名看 diff；× 单独撤销
│ src/b.ts                            +5 -0         ×  │
└──────────────────────────────────────────────────────┘
        │ Keep All
        ▼
（列表清空，面板收起；AI 再改文件时重新出现）
```

Keep All 无确认框；运行中、操作进行中或列表为空时禁用。Undo All 和 × 会先弹确认，说明会整文件覆盖、新建的文件会被删除、聊天保持不变。

### 2.3 关键决策清单

- **原件在 AI 写之前抓，每个文件每个周期只抓一次。**
  <small style="color:gray">`file_baselines.rs::TurnFileBaselines::prepare`、`PendingBaseline::commit`；`tool_dispatcher.rs` 只对 `write`/`edit`/`hashline_edit` 生效；失败、拒绝、取消、no-op 不落行。</small>
- **每条登记行自带捕获时间 `at`，范围和排序只看它。**
  <small style="color:gray">`file_baselines.rs::Baseline.at`（毫秒）、`session_files.rs::scope_rows`；改前按备份页 mtime 判断，改后按行时间；缺 `at` 的旧格式行直接忽略，不做迁移或回填。</small>
- **Keep 只是一个空目录 `keep-<毫秒>`，不复制、不快照。**
  <small style="color:gray">`session_files.rs::keep`、`file_baselines.rs::latest_keep`；非空或命名不合法的目录不算 Keep；没有锁、全局序号或独立索引。</small>
- **写入追踪器在"换消息"或"Keep 变化"时重建。**
  <small style="color:gray">`tool_dispatcher.rs` 中 `b.message_id != owner_id || !b.keep_is_current()`；保证 Keep 后同一份归属里也会重新抓原件。</small>
- **新登记的时间严格晚于最近一次 Keep。**
  <small style="color:gray">`file_baselines.rs::timestamp_after`；同一毫秒会等到下一毫秒；时钟回拨直接报错 `baseline_clock_not_advanced`，不伪造时间。</small>
- **所有动作都校验起点，起点过期就拒绝。**
  <small style="color:gray">`sourceTurnId` 有 Keep 时是 Keep id，否则是最早登记所在的消息 id；不一致返回 `unknown_path`，客户端不能指定备份路径或恢复字节。</small>
- **git 提交视为认可。**
  <small style="color:gray">`session_files.rs::effective_baselines`：按旧提交分组跑 `git log`；旧提交找不到（如被 rebase 掉）时整组改用当前提交版比较，当前提交里没有的文件仍用备份；用 `cat-file --filters` 读提交版，遵守换行和 smudge 规则；非 Git 或仓库外的路径仍用备份。</small>
- **恢复了文件的回退会撤销之后的 Keep，只回退聊天的不碰。**
  <small style="color:gray">`file_baselines.rs::discard_keeps_after`，由 `session_impl.rs::rewind_user_message_with_files_restored`（`files_restored=true` 时）和 `cmd_restore.rs` 的恢复文件分支调用；只删空目录，不递归删除。</small>
- **备份只在 rewind 删消息、会话过期或会话删除时清理。**
  <small style="color:gray">`file_baselines.rs::discard_turns`、`prune`（按会话聊天记录文件的修改时间判断闲置）、`discard_session`；Keep、Undo、checkpoint 截断、Retry 都不删备份。</small>

### 2.4 参考实现、适用前提与推翻条件

参考实现（均已在同级目录核对）：

- VS Code：`src/vs/workbench/contrib/chat/browser/chatEditing/chatEditingSession.ts::_getOrCreateModifiedFileEntry` 支持跨请求审阅同一文件；`chatEditingCheckpointTimeline.test.ts` 的 `navigating between multiple requests` 验证历史导航。它在内存里维护原件，Tomcat 不照搬成磁盘全量快照，而是在下一次写入前抓取。
- Codex：`codex-rs/core/src/turn_diff_tracker.rs::TurnDiffTracker` 与 `turn_diff_tracker_tests.rs::accumulates_add_then_update_as_single_add` 体现"首次原件 + 净变化"。但它的 diff 按轮出回执，不是跨轮常驻的累计清单。
- OpenCode：`packages/opencode/src/session/summary.ts::computeDiff` 用私有快照算单次请求变化。用户自己的 git HEAD 和私有备份不能混成一份历史，这是 Tomcat 单独处理"提交即认可"的原因。

适用前提：

- 系统时钟可排序。
- 同一会话同一时间只有一个 AI 在写文件。
- Keep 只在空闲时执行。

出现以下情况时应重新设计，不在现方案上打补丁：

- 同一会话出现多个并发写者，或时钟回拨成为常态。
- 需要逐文件 Keep、字符级"只撤 AI 改动"、或整组事务式恢复。
- 实测发现提交后刷新变慢（目前每个被提交的文件跑一次 `git cat-file`），这时再考虑可重建的缓存。

## 3. 测试用例清单

- **压缩后的登记归属与回退**：整段压缩后内存只剩摘要，第一批就是写入时仍登记新文件，归属本批 AI 回复；无用户消息的 Resume 同样归属 AI 回复，rewind 能还原并删除目标后的备份页、保留目标前的页；本轮中途压缩后保持同一追踪器，同文件不重复登记。
  <small style="color:gray">`tomcat/src/core/agent_loop/tests/tool_media_batch_test.rs`：`file_baseline_owner_survives_compacted_history`、`file_baseline_owner_after_resume_without_user_message`、`file_baseline_owner_sticky_within_run_after_compaction`。</small>

- **跨消息累计与排序**：多条普通消息、Build、Resume 连续改文件，列表不清空；同一文件取最早原件；排序看行时间而不是备份页修改时间；旧消息被重新激活后仍取真原件。
  <small style="color:gray">`tomcat/src/core/checkpoint/tests/session_files_test.rs`：`session_file_normal_turns_accumulate_without_switching`、`session_file_same_path_across_turns_uses_earliest_baseline`、`session_file_scope_order_uses_record_at_not_manifest_mtime`、`session_file_reactivated_owner_keeps_true_earliest_baseline`；`tomcat/src/api/serve/tests/session_files_test.rs::session_files_build_resume_continue_never_reset`。</small>
- **Keep 的行为**：Keep 不复制文件、不碰磁盘、清空列表；Keep 后手动修改再由 AI 修改，原件是手动版本；旧消息被重新激活后，Keep 之后的新改动不会被漏掉；同毫秒边界正确；过期起点被拒绝；新增或撤销 Keep 时追踪器重建。
  <small style="color:gray">`session_files_test.rs`：`session_file_keep_does_not_copy_files`、`session_file_keep_empties_list_without_touching_disk`、`session_file_keep_preserves_manual_edit_before_ai`、`session_file_reactivated_owner_after_keep_does_not_hide_new_changes`、`session_file_keep_millisecond_boundaries`、`session_file_keep_records_absence_and_rejects_stale_source`；`tomcat/src/core/agent_loop/tests/tool_media_batch_test.rs::file_baseline_dispatcher_rebuilds_for_keep_changes_only`。</small>
- **提交即认可**：提交后文件消失、再改只显示提交后差异；提交后 revert 仍按提交处理；新建、删除、换行过滤的文件正确。
  <small style="color:gray">`session_files_test.rs`：`session_file_commit_counts_as_kept_and_rewrite_undo_uses_commit`、`session_file_keep_commit_revert_uses_commit`、`session_file_commit_new_deleted_and_eol_filtered_paths`。</small>
- **回退与清理**：恢复文件的 rewind 和 checkpoint 只撤销目标时间之后的 Keep，只回退聊天时保留；Keep 后重新抓原件不影响 rewind 找回最初版本；活跃会话保留备份、闲置会话被清理。
  <small style="color:gray">`tomcat/src/core/checkpoint/tests/file_baselines_test.rs`：`discard_keeps_after_preserves_earlier_boundaries_and_message_backups`、`file_baseline_recapture_after_keep_preserves_original_for_rewind`、`prune_keeps_active_sessions_and_drops_idle_ones`；`tomcat/src/api/serve/tests/rewind_and_resend_test.rs::rewind_only_reverses_keep_when_disk_returns_before_it`；`tomcat/src/api/chat/commands/tests/cmd_restore_test.rs::restore_core_discards_keeps_only_when_reverting_files`。</small>
- **旧格式数据**：缺 `at` 的旧行不进入 Files、追踪器和 rewind。
  <small style="color:gray">`session_files_test.rs::session_file_legacy_rows_without_at_are_ignored`；`file_baselines_test.rs::file_baseline_legacy_rows_do_not_seed_tracker_or_rewind`。</small>
- **端到端与界面**：真实 serve 接口下 Keep 不发新聊天或模型请求；面板按钮禁用、确认框与结果回显。
  <small style="color:gray">`tomcat-vscode-ext/tests/serve_session_files.test.ts`；`tomcat-vscode-ext/gui/src/components/SessionFilesDock.test.tsx`。</small>
