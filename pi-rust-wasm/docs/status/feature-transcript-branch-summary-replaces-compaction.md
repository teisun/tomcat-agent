| Owner | Update Time | State | Branch | Cov% |
| :--- | :--- | :--- | :--- | :--- |
| Agent | 2026-04-12 | DONE | feature/transcript-branch-summary-replaces-compaction | — |

### Transcript：BranchSummary 替代 Compaction

**目标**：JSONL 压缩摘要行统一为 `type: branch_summary`；Rust 侧删除 `TranscriptEntry::Compaction` / `CompactionEntry`，扩展 `BranchSummaryEntry` 承载原字段；`set_branch_summary_entry_is_boundary_true` 原地升级 `isBoundary`；对外 `pub use` 改为 `BranchSummaryEntry`。

#### 验收

| 项 | 结果 |
| :--- | :--- |
| `cargo fmt` / `cargo clippy -p pi_wasm --all-targets -- -D warnings` | PASS |
| `cargo test -p pi_wasm` | PASS |

#### 其它

- 本机会话文件 `~/.pi_/agents/main/sessions/1774521308274_47a17397cf2478e2.jsonl` 已备份 `.bak` 并将 `"type":"compaction"` 替换为 `"type":"branch_summary"`（1 处）。

### BLOCKED

| 阻塞项 | 原因 | 预计解决 |
| :--- | :--- | :--- |
| 无 | — | — |
