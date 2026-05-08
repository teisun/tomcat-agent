# 调试排查规范

本文档总结从 `estimate_context_chars` 水位漂移排查中提炼的方法论，供后续 bug 排查参考。

由实际案例驱动：TASK-17 上下文管理 L3 截断后水位虚高（53.8%），重启后恢复正常（0.9%）。

**「说人话」辅助**：技术分析段落**先**写证据与结论，**再**可加短段口语归纳；多列对照表 **SHOULD** 末列 **`说人话`**。与 [`ARCHITECTURE_SPEC.md §14.1`](../../openspec/specs/guides/workflow/ARCHITECTURE_SPEC.md) 一致。

---

## 一、排查流程总览

```
观察异常 → 量化偏差 → 建立假设 → 诊断验证 → 定位根因 → 修复
```

每一步都有明确的"完成标准"，不跳步。

---

## 二、第一步：观察异常——先描述现象，不急着看代码

**做什么**：用用户视角记录"什么不对"，用具体数据而不是感觉。

**反面**："水位好像不对" —— 没有定量，无法验证。

**正面**：
- L3 截断后 `usage_ratio=0.443`，但 `messages` 只剩 1 个 turn（48 chars）
- 重启 `tomcat chat` 后同一 session 水位从 53.8% 降到 0.9%
- 日志显示 `user_turns_len=0, user_turns_total_chars=0` 但 `usage_ratio=0.5303`

**关键经验**：

- **"重启后恢复正常"是强信号**：说明问题在内存中的运行时状态，而非持久化数据。初始化路径从持久化数据重算了某个值，运行时路径没有保持这个值的一致性。
- **拿到 2-3 组对比数据**：一组异常、一组正常（重启后）、一组中间状态。三角交叉验证比单点观察更可靠。

---

## 三、第二步：量化偏差——算出"差了多少"

**做什么**：用异常数据反推"幽灵"的大小，缩小排查范围。

**示例**：
```
usage_ratio = 0.5303
budget_tokens = 272000
estimated_tokens = 0.5303 * 272000 ≈ 144250
estimate_context_chars = 144250 * 4 ≈ 577000

user_turns_total_chars = 0
system_text ≈ 1300

幽灵 = 577000 - 0 - 1300 ≈ 575700 chars
```

这个 575K 的量级直接指向"大量 tool results（读文件）的字符数被困在 estimate 中"。如果幽灵只有几百字节，排查方向完全不同。

**关键经验**：

- **量化让你知道该往哪看**：575K 的幽灵不可能来自用户输入（几十字节），只可能来自 tool results（读文件动辄几万字节）。
- **倒推公式**：从输出（usage_ratio）反推输入（estimate_context_chars），再对比已知部分（turns + system），差值就是幽灵。

---

## 四、第三步：建立假设——追踪值的"一生"

**做什么**：找到异常值（`estimate_context_chars`），列出它在代码中**所有被修改的地方**，画出生命周期。

**方法：加法-减法清单**

| 操作 | 方向 | 代码位置 | 时机 |
| :--- | :--- | :--- | :--- |
| `init_context_state` | 赋值 | `context.rs` | 进程启动 |
| `on_message_appended(n)` | +n | `types.rs:178` | 每条新消息 |
| `force_drop_oldest_to_target` | -= turn_chars | `cascade.rs:20` | L3 截断 |
| `apply_boundary` | -= batch_chars, += summary_chars | `types.rs:274` | L2 压缩应用 |
| `invalidate_api_usage` | 不改 estimate | `types.rs:222` | L3/L2 前 |

画完这张表，问自己：**有没有"只加不减"的路径？**

在本案例中：`on_message_appended` 在 agent_loop 推理期间累加了 tail 的字符数，但 L3 只能减 `messages` 中 turn 的字符数。如果 tail 不在 `messages` 里，它的字符数就是"只加不减"的。

**关键经验**：

- **双账本系统必查对齐**：当两个数据结构（数字 vs 列表）各自独立维护同一份"真相"时，任何只改其中一个的操作都可能导致脱节。
- **画加法/减法清单**比通读代码更高效。你不需要理解每行代码的含义，只需要找到所有 `+=` 和 `-=` 的地方。

---

## 五、第四步：诊断验证——用代码证明，不靠猜

**做什么**：在假设涉及的关键点加诊断日志，跑一遍真实场景，用日志数据证实/证伪假设。

**诊断点选择原则**：

1. **分支判定点**：`if let Some(usage) = ... { A } else { B }` —— 打印走了哪个分支
2. **值变化点**：`estimate_context_chars` 被修改前后 —— 打印前值和后值
3. **"应该发生但可能没发生"的点**：`StreamEvent::Usage` 处理分支 —— 打印证明它从未被命中
4. **数据传递点**：`messages.push` 前 —— 打印消息内容是否完整

**关于日志框架 vs eprintln**：

本案例中发现 `tracing::info!` 由于 `RUST_LOG` 配置问题未输出（`log.level = "warn"` + `EnvFilter::clone()` 可能的实现问题）。`eprintln!` 绕过了整个日志框架，直接写 stderr，100% 可靠。

**经验法则**：
- 生产日志用 tracing
- 临时诊断用 eprintln —— 零依赖、零配置、必定输出
- 验证完毕后必须清理所有诊断代码

**诊断日志输出示例**：
```
[DIAG] estimated_token_count: branch=fallback_chars estimate_context_chars=6326 result=1581
[DIAG] before_push_messages: has_user_msg=false msg_count=1 input_len=2 last_api_usage=false
```

一眼就能确认：`branch=fallback_chars`（从未走 api_usage 分支）、`has_user_msg=false`（UserTurn 缺少用户消息）。

---

## 六、第五步：定位根因——区分"发生时刻"和"显现时刻"

**这是最容易踩的坑**：bug 的症状出现在 A 时刻，但根因在更早的 B 时刻。

**本案例的典型体现**：

| | 发生时刻 | 显现时刻 |
| :--- | :--- | :--- |
| 根因 1 | `openai.rs` 解析 chunk 时不含 usage 字段 | 每次调用 `estimated_token_count()` 走 fallback |
| 根因 2 | L3 rebuild 时 `start_idx = messages.len()` 跳过 tail | 下一轮 chat loop 重建 messages 时 tail 消失，但 estimate 还在 |
| 次要问题 | chat/mod.rs 用 `new_messages`（不含 User）打包 turn | 后续 `build_context_from_state` 重建时丢失用户输入 |

**"下一轮才显现"的 bug 特别难发现**：在 L3 rebuild 那一刻，tail 还在 messages 里，两本账是对的。直到下一轮 `build_context_from_state` 从 turn list 重建 messages 时，tail 才消失。如果只检查 L3 rebuild 的瞬时状态，会误以为没问题。

**关键经验**：

- **追踪到下一个使用点**：不要只看"值被设置的地方"，还要看"值被使用的地方"。本案例中 `estimate_context_chars` 在 L3 时被部分减扣（看起来合理），但在下一轮的 `build_context_from_state` 时才暴露脱节。
- **画时间线**：把涉及的操作按执行顺序排列，标注每一步后两本账的状态，找到第一次脱节的时刻。

---

## 七、常见 bug 模式速查

| 模式 | 特征 | 排查方向 |
| :--- | :--- | :--- |
| 双账本脱节 | 两个数据结构记录同一份信息，某些操作只更新其一 | 列出所有修改点，检查是否成对更新 |
| 只加不减 | 值单调递增，理应有减少的操作但未生效 | 画加法/减法清单，找"谁负责减？减的时候能覆盖到吗？" |
| 重启恢复 | 重启后问题消失 | 初始化路径是正确的，运行时路径有漂移，对比两条路径 |
| 事件从未触发 | 某个 handler 永远不执行 | 从事件源头追：谁产生这个事件？解析器是否包含对应字段？ |
| 采集窗口错位 | `start_idx` / `offset` 设置导致数据被跳过 | 画 messages 数组的索引变化图，标注每次设置 start_idx 的时机 |
| 打包边界遗漏 | 数据从一种表示转入另一种时，窗口外的数据永久丢失 | 找到数据传递操作，检查窗口是否覆盖全部应包含数据 |

---

## 八、修复策略：治本优于治标

排查定位根因后，修复方案的选择同样重要。

### 8.1 区分治标和治本

| 策略 | 手段 | 适用场景 | 风险 |
| :--- | :--- | :--- | :--- |
| **治标** | 在脱节发生后重算/修补（如 L3 后用 `system + sum(msgs) + sum(tail)` 重算 estimate） | 紧急热修复、根因不可改（第三方库） | 掩盖根因；其他触发路径仍可能脱节 |
| **治本** | 修正导致脱节的源头操作（如调整 `start_idx` 让 tail 能被正确加入 `messages`） | 根因在己方代码、有测试覆盖 | 改动链路更长，需更多测试 |

**本案例的教训**：最初的方案是 L3 rebuild 后重算 `estimate_context_chars`（治标）。用户指出"tail 的 tokens 不就是原来的幽灵吗？正确做法是校正 `start_idx`"——这才抓到了真正的修复方向：让 tail 被正确加入 `messages`（治本），从根本上消除幽灵的产生，而非在幽灵出现后再清扫。

### 8.2 多根因合并修复

当多个根因指向同一个机制缺陷时，优先寻找**一个修改点同时解决多个问题**的方案：

**本案例**：
- 根因 2（幽灵字符）：`start_idx` 跳过 tail → tail 不进 `messages`
- 根因 3（User 消息丢失）：`start_idx` 在 User 消息 push 前设置 → User 不进 new_messages

两者本质相同——`start_idx` 的设置时机/位置不对。合并为 Fix B+C：将 `start_idx` 指向 `context_tail_start`，同时移除提前写 User 消息的冗余操作。一处改动解决两个根因，比各自打补丁更优雅、更不容易遗漏。

### 8.3 修复时检查下游兼容性

改动数据的生产方式后，必须追踪所有消费方：

| 改动 | 需要检查的下游 |
| :--- | :--- |
| `new_messages` 现在包含 User 消息 | `ChatMessage::user` 是否正确构造？ |
| 移除 chat/mod.rs 提前写 User 到 transcript | transcript 重载（`fold_entries_to_messages`）是否仍能正确识别 User turn？ |
| `start_idx` 指向 `context_tail_start` | L3 rebuild 路径中 tail 提取逻辑是否兼容？ |

**经验法则**：对每个改动，问"谁消费这个数据？消费者的假设是否仍然成立？"

---

## 九、测试策略：从可见性约束出发

### 9.1 pub(crate) 边界决定测试方式

集成测试（`tests/` 目录）作为外部 crate 消费者，只能访问 `pub` API。

| 目标 | 策略 |
| :--- | :--- |
| 测试 `pub(crate)` 类型（如 `CompactionResult`） | 放在**同级独立 `tests.rs`** 中作为单元测试（父模块用 `#[cfg(test)] mod tests;` 引入；**禁止**在业务源文件内联 `#[cfg(test)] mod tests { ... }`，依据 [RUST_FILE_LINES_SPEC.md §A](../../openspec/specs/guides/coding/RUST_FILE_LINES_SPEC.md)） |
| 测试外部可见行为（如 L3 后 estimate 一致性） | 通过 `pub` 入口（如 `force_drop_oldest_to_target`）间接触发，在 `tests/` 中写集成测试 |

**本案例的教训**：最初计划直接在集成测试中构造 `CompactionResult`，编译失败（`error[E0603]: module manager is private`）。改为通过 `force_drop_oldest_to_target()` 这个公开 API 间接测试，既避免了暴露内部类型，又验证了真实的 L3 路径。

### 9.2 Clippy 是第一道门禁

`cargo clippy --all-targets -- -D warnings` 必须在 `cargo test` 之前运行。Clippy 能发现的常见问题：

- `manual_abs_diff`：手写绝对差值 → 用 `.abs_diff()`
- `unused_imports`：移除代码后残留的 `use` 语句
- `needless_borrow`：不必要的 `&` 引用

修 Clippy 比修测试快得多，先过 Clippy 再跑测试能节省迭代时间。

---

## 十、诊断代码管理

- 诊断代码（eprintln、临时 info!）必须在验证完毕后**立即清理**，不进入 commit
- 如果需要长期保留的观测点，应转为正式的 tracing 日志（用合适的 target 和 level）
- 诊断日志格式建议统一前缀 `[DIAG]`，方便 grep 清理
- **先提交修复代码，再清理诊断代码**——避免在同一个 commit 中混合功能改动和清理

---

## 十一、排查报告模板

排查完毕后，在计划或 commit 中记录以下信息：

```markdown
### 排查结论

**现象**：（一句话描述用户可见的异常）
**量化**：（异常值 vs 期望值，差值大小）
**根因**：（具体代码位置 + 一句话机制描述）
**验证方法**：（用什么诊断手段确认的）
**修复方案**：（改哪里、怎么改——标注"治标"还是"治本"）
**合并情况**：（是否合并多个根因的修复、为什么可以合并）
**回归风险**：（修复可能影响的其他路径 + 检查清单）
```
