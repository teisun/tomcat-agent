本文是 chat 启动期 transcript 恢复的单一事实源。旧文档里与 `init_context_state()` / `read_entries_tail()` / plan fast path / sidecar 相关的描述，若与本文冲突，以本文和代码为准。关联入口：[`context-management.md`](./context-management.md)、[`session-storage.md`](./session-storage.md)、[`tools/checkpoint-resume.md`](./tools/checkpoint-resume.md)。

---

# Chat Resume Hydration

## 1. 目标

目标不是改变 chat 语义，而是把启动恢复从“先扫很多 transcript，再猜需要哪些上下文”改成“先读轻量 metadata，再定点读取真正需要的 slice”。

本方案固定三条原则：

1. `init_context_state()` 继续沿用现有 **file-order + superseded** 语义，不引入 parent-chain ActivePath。
2. 热路径优先少读磁盘：已有 sidecar 时走 **metadata-first + reverse-chunk**。
3. 冷路径优先稳：sidecar 缺失/损坏时用 **forward-stream + O(1) 状态**重建，再回到热路径。

---

## 2. 产物

每个 session 现在有两类文件：

- transcript：`~/.tomcat/.../sessions/<session_id>.jsonl`
- resume sidecar：同目录 sibling 文件 `~/.tomcat/.../sessions/<session_id>.resume-index.json`

sidecar 只保存恢复定位所需的轻量 metadata，不保存完整消息正文。

### 2.1 sidecar 字段

- `schema_version`
- `transcript_size`
- `transcript_mtime_ms`
- `total_entries`
- `last_entry_id`
- `latest_boundary`
- `recent_turn_starts`
- `latest_day_first_entry`
- `latest_plan_event`

其中 anchor 统一保存：

- `entry_id`
- `ordinal`
- `timestamp`
- `entry_kind`

`latest_plan_event` 保存的是可直接恢复成 `PlanEventRef` 的 metadata，不要求把 plan event 本体重新塞回聊天 slice。

---

## 3. Ordinal 口径

`ordinal` 不是物理行号，而是“跳过 header 之后，成功解析出的第几条 entry”。

- header 不计入 ordinal
- blank line 不计入 ordinal
- 损坏 / 未知类型行不计入 ordinal
- 第一条成功解析的 entry ordinal = `0`

### 3.1 说人话小图

```text
物理文件行号:
  1  header
  2  message(u1)
  3  <blank>
  4  message(a1)
  5  totally_unknown_variant   <- 跳过
  6  branch_summary(boundary)

ordinal:
  line 2 -> ordinal 0
  line 4 -> ordinal 1
  line 6 -> ordinal 2
```

这个口径必须在四个角色里保持一致。

---

## 4. 四个角色

### 4.1 Writer

`append_*` 路径负责：

- 把新 entry 写入 transcript
- 给 sidecar 做 O(1) 增量更新
- 维护 `total_entries`
- 追加 / 滚动 `recent_turn_starts`
- 更新 `latest_boundary` / `latest_plan_event`

### 4.2 Rewrite

以下 rewrite 类操作会改写整个 transcript 文件，因此不能只增量修 sidecar：

- `insert_entry_after_message_id`
- `mark_message_entries_after_anchor_superseded`
- `rewrite_message_text_entries_by_id`
- `set_branch_summary_entry_is_boundary_true`
- `remove_branch_summary_entry_by_id`

这些操作现在在 **同一趟 rewrite 后立刻 inline rebuild sidecar**：rewrite 期间已经拿到了新的内存行集合，随后直接复用这些行调用内联重建 helper 写回 sidecar，**不会在 `write_file_atomic(...)` 之后再把 transcript 重新打开全量读一遍**。这样既避免“下次启动才发现索引失效”，也把 rewrite 后的额外读盘压到 0。

### 4.3 Rebuild

当 sidecar 缺失、schema 不对、fingerprint 不匹配时，走 cold rebuild：

- forward-stream 扫整份 transcript
- O(1) 滚动状态重建 metadata
- 不把整文件 `.collect::<Vec<_>>()` 到内存

### 4.4 Reader

热路径 reader 是 **count-driven reverse-chunk**：

- 输入只认 `K = 最后 K 条 entry`
- 不边读边找锚点
- 不在 reverse-chunk 过程中计算全局 ordinal

reader 的职责只是“把最后 K 条成功解析的 entry 读出来”。

---

## 5. 热路径：reverse-chunk tail reader

`read_entries_tail()` 现在不再 `reader.lines().collect::<Vec<_>>()`。

实现改成：

1. 从文件尾开始 `seek`
2. 固定块大小反向读
3. 用 `\n` 分隔完整 JSONL 行
4. 处理跨 chunk 的半行拼接
5. 按“成功解析 entry 数量”计数，到 `cap = K` 即停

### 5.1 关键性质

- **count-driven**：只认“最后 K 条 entry”
- **anchor-agnostic**：不知道 boundary / turn / plan 是谁
- **bounded bytes**：hot path 只读足够多的尾部字节，不扫整文件

### 5.2 额外基础 API

为测试和后续扩展保留了按 ordinal 的 slice reader：

- `read_entries_range_by_ordinal_with_stats(path, start, end)`

这条 API 是正向 streaming 读区间，不是热路径默认实现。

---

## 6. 侧车 metadata 如何选锚点

恢复时需要的不是“所有历史”，而是足以让旧逻辑 `compute_fold_start()` / `filter_messages_by_day()` 产出相同结果的最小前缀。

### 6.1 先选 turn/day 下界

先算：

- `today_anchor = latest_day_first_entry`，但只在 `latest_day == today()` 时生效
- `recent_turn_anchor = recent_turn_starts` 里“最近第 10 个 user-turn 起点”；若不足 10 个，取最老那个

然后：

```text
turn_window_start = min(today_anchor, recent_turn_anchor)
```

这里的 `min` 是按 ordinal 取更早的那个。

### 6.2 再和 boundary 合并

如果存在 `latest_boundary`，则不能读到 boundary 之前：

```text
slice_start = max(latest_boundary, turn_window_start)
```

这里的 `max` 也是按 ordinal。

说人话：

- “今天的第一条 entry”负责保住“今天全部消息”
- “最近第 10 个 user turn”负责保住跨午夜回填
- “latest boundary”负责保证不会把被边界淘汰的旧前缀再带回来

---

## 7. count-based 匹配，不是边读边匹配

最容易误解的点：tail reader 不会“拿着 sidecar 锚点，一边 reverse-chunk 一边找”。

真正机制是三步：

### 7.1 指纹门控

先用 sidecar 自己的 fingerprint 判断“这份索引还能不能信”：

- `schema_version`
- `transcript_size`
- `transcript_mtime_ms`
- `last_entry_id`

不通过就先 rebuild sidecar。

### 7.2 算 K

一旦 `slice_start.ordinal` 确定，就有：

```text
K = total_entries - slice_start_ordinal
```

这意味着“最后 K 条 entry”理论上一定覆盖目标锚点。

### 7.3 读后 edge 校验

读出最后 K 条后，再检查最老那条 entry 是否等于 `slice_start`：

- 优先比 `entry_id`
- 没有 id 时退化到 `entry_kind + timestamp`

如果 edge 不符，说明索引与文件顺序漂移，直接：

- rebuild sidecar
- fallback 到 `Full`

所以这是 **count-based + edge verification**，不是“边读边匹配”。

---

## 8. 冷路径：做法 A

sidecar 缺失 / 损坏 / schema 不兼容时，固定采用做法 A：

```text
forward-stream rebuild sidecar  +  reverse-chunk(K) targeted read
```

不用 `lines().collect()`，也不用“forward-stream + 大 ring buffer 一次把筛选结果留出来”做热路径。

### 8.1 为什么热冷分离

- **热路径** 要最少 I/O：reverse-chunk 更合适
- **冷路径** 本来就必须扫整份 transcript：forward-stream 更稳，峰值常驻更小

### 8.2 ASCII

```text
               sidecar 缺失 / 损坏
                       │
                       ▼
          forward-stream rebuild metadata
          (bytes ~ whole transcript, O(1) state)
                       │
                       ▼
          choose slice_start / compute K
                       │
                       ▼
            reverse-chunk read last K entries
                       │
                       ▼
               fold + filter + hydrate
```

---

## 9. Plan fast path

`latest_plan_event` 不再绑定 `MAX_PLAN_SCAN = 5000` 的 transcript 尾扫。

现在有两条路径：

- `Full` 模式：沿用旧行为，从读取到的 entry slice 里反向扫描 plan event
- `Tail/Auto` 热路径：直接从 sidecar 的 `latest_plan_event` 取 `PlanEventRef`

注意：

- plan event 可以落在聊天 slice 之外
- 这不会把聊天 slice 扩大
- plan runtime 后续根据 `PlanEventRef.path` 去附着 `.plan.md`

也就是说，plan fast path 恢复的是“指针”，不是把 plan 文本塞回聊天上下文。

---

## 10. 启动期运行时总图

```text
append/rewrite side
────────────────────────────────────────────────────────────────

append_entry / append_message / append_custom / ...
        │
        ├─ append transcript JSONL
        └─ sidecar O(1) incremental update

rewrite operators
        │
        ├─ rewrite transcript atomically
        └─ inline rebuild sidecar from in-memory lines


read/restore side
────────────────────────────────────────────────────────────────

chat --resume / init_context_state
        │
        ├─ mode=full  ──────────────► reverse-chunk(read_cap=5000) ─► scan plan in slice
        │
        └─ mode=auto/tail
              │
              ├─ load+validate sidecar
              │     └─ bad? forward-stream rebuild
              │
              ├─ choose anchors:
              │     latest_boundary
              │     latest_day_first_entry
              │     recent_turn_starts[-10]
              │
              ├─ count-based K = total_entries - slice_start.ordinal
              │
              ├─ reverse-chunk(last K entries)
              │
              ├─ edge verify oldest entry == slice_start
              │     └─ mismatch? rebuild + fallback Full
              │
              └─ latest_plan_event from sidecar
```

---

## 11. Fallback、kill switch 与 trace

### 11.1 config

`[context]` 新增：

- `resume_hydration_mode = "auto" | "full" | "tail"`
- `resume_lazy_threshold = 2000`（默认）

其中：

- `auto`：entry 数超过阈值才走 sidecar + targeted hydrate
- `full`：强制旧路径
- `tail`：强制 metadata-first + targeted hydrate

### 11.2 trace

设置 `TOMCAT_RESUME_TRACE=1` 时，启动期向 stderr 输出单行：

```text
TOMCAT_RESUME_TRACE mode=Tail entries_scanned=... bytes_scanned=... boundary_hit=... plan_source=... fallback=... elapsed_ms=...
```

字段含义：

- `mode`: `Full | Tail`
- `entries_scanned`: 本次恢复涉及的 entry 读取量
- `bytes_scanned`: 本次恢复涉及的字节读取量
- `boundary_hit`: 本次 slice 内是否命中 boundary
- `plan_source`: `scan | sidecar | none`
- `fallback`: `none | rebuild | full+rebuild | threshold_full | config_full`
- `elapsed_ms`: `init_context_state()` 总耗时

### 11.3 回滚手段

出现问题时可直接切回：

```toml
[context]
resume_hydration_mode = "full"
```

或环境变量：

```bash
TOMCAT__CONTEXT__RESUME_HYDRATION_MODE=full
```

---

## 12. sidecar 失效成本

sidecar 并不脆弱到“没意义”：

- append 是主流路径，O(1) 增量维护
- rewrite 本来就 O(file)，且现在直接复用 rewrite 已持有的内存行集合做 sidecar 重建，不会额外再读一遍 transcript
- 真正缺失 / 损坏时，最坏也只是回到一次 cold rebuild 的 O(file) 成本
- edge mismatch 还会兜底 `Full + rebuild`，不会偷偷返回错上下文

所以 sidecar 不是“永远正确的真相”，而是“有验证和回退保护的加速器”。

---

## 13. 验证与基线

### 13.1 自动化测试

已覆盖：

- transcript 真 tail / ordinal slice reader
- sidecar 增量维护 / schema 与 fingerprint 重建 / rewrite inline rebuild
- `init_context_state()` 在大 session 下的 `Auto` vs `Full` parity
- plan fast path
- delete sidecar 后自动 rebuild
- kill switch `Full`
- CLI `--resume` trace / dangling tool-call heal / corrupt sidecar rebuild

对应测试文件：

- `src/core/session/tests/transcript_read_test.rs`
- `src/core/session/tests/resume_index_test.rs`
- `src/core/session/manager/tests/hydrate_test.rs`
- `tests/resume_hydration_tests.rs`
- `tests/resume_hydration_cli_e2e.rs`
- `tests/resume_hydration_perf.rs`（manual / ignored）

### 13.2 2026-06-07 本地 perf 基线

来自 `cargo test --test resume_hydration_perf -- --ignored --nocapture`：

#### 冷重建基线（sidecar 缺失，做法 A）

| scale | bytes_scanned | entries_scanned | elapsed_ms |
| --- | ---: | ---: | ---: |
| 10k | 1,305,094 | 10,026 | 444 |
| 50k | 6,345,094 | 50,026 | 1,366 |
| 200k | 25,445,095 | 200,026 | 5,606 |

#### 热路径（sidecar 已存在，boundary session）

| scale | bytes_scanned |
| --- | ---: |
| 10k | 131,072 |
| 50k | 131,072 |
| 200k | 131,072 |

#### kill switch 对比（50k boundary session）

| mode | bytes_scanned |
| --- | ---: |
| `Tail/Auto` hot path | 131,072 |
| `Full` kill switch | 655,360 |

结论：

- cold rebuild 仍然线性，但常驻内存是 streaming 的 O(1) 状态
- hot path 已经不再随 transcript 全文件线性增长
- kill switch 明显更保守，适合作为灰度回滚手段

---

## 14. 旧文档如何引用

- `context-management.md` 只保留“上下文恢复语义 + 入口”，把 resume hydration 细节回链到本文
- `session-storage.md` 只保留 transcript / sidecar 文件布局与生命周期
- `checkpoint-resume.md` 只保留“启动期 hydrate 的产品语义”，把具体读路径回链到本文

别再把 `MAX_PLAN_SCAN`、sidecar schema、reverse-chunk 实现细节分散写回旧文档。
