//! # F2 批量工具端到端小测（read `paths` / edit `files`）
//!
//! 和 `tool_exec_dedup_test` 一样跑真实 `DefaultPrimitiveExecutor` + tempdir，不用 mock ——
//! 批量的价值全在「预算怎么裁、失败怎么隔离、stamp 有没有跟上」这些真实文件系统行为上，
//! mock 掉就什么都验不到了。
//!
//! 用例清单：
//! - read：顺序与分段标注、预算用尽后标 SKIPPED 并给续读提示、`path`/`paths` 互斥
//! - edit：全量预检一次报全、部分落盘、文件内原子、落盘后刷新 ReadStamp

use std::sync::Arc;

use crate::core::agent_loop::tool_exec::execute_tool_full;
use crate::core::agent_loop::types::SubagentType;
use crate::core::agent_loop::ToolCallInfo;
use crate::core::permission::{DefaultPermissionGate, GateConfig, SessionGrants};
use crate::core::tools::pipeline::read_state::ReadFileState;
use crate::core::tools::primitive::{DefaultPrimitiveExecutor, PrimitiveExecutor};
use crate::core::AllowAllConfirmation;
use crate::infra::events::{ToolDisplay, ToolDisplayFileEntry, ToolDisplayFileStatus};
use crate::infra::{PrimitiveConfig, TracingAuditRecorder};

fn make_executor(dir: &std::path::Path) -> Arc<dyn PrimitiveExecutor> {
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: dir.to_path_buf(),
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    )
    .into_arc();
    Arc::new(DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        gate,
    ))
}

fn make_tc(name: &str, args: serde_json::Value) -> ToolCallInfo {
    ToolCallInfo {
        id: format!("tc-{name}"),
        name: name.to_string(),
        arguments: args.to_string(),
    }
}

async fn run(
    primitive: &Arc<dyn PrimitiveExecutor>,
    state: &Arc<ReadFileState>,
    tc: &ToolCallInfo,
) -> crate::core::agent_loop::tool_exec::ToolExecOutcome {
    execute_tool_full(
        primitive,
        &None,
        &None,
        Some(state),
        None,
        None,
        None,
        None,
        None,
        SubagentType::User,
        &tokio_util::sync::CancellationToken::new(),
        tc,
        None,
        None,
    )
    .await
}

fn files_display(
    outcome: &crate::core::agent_loop::tool_exec::ToolExecOutcome,
) -> Vec<ToolDisplayFileEntry> {
    match outcome.display.as_ref() {
        Some(ToolDisplay::Files { files, .. }) => files.clone(),
        other => panic!("expected a Files display, got {other:?}"),
    }
}

fn status_of(entries: &[ToolDisplayFileEntry], suffix: &str) -> Option<ToolDisplayFileStatus> {
    entries
        .iter()
        .find(|e| e.file.ends_with(suffix))
        .unwrap_or_else(|| panic!("no entry ending in {suffix} among {entries:?}"))
        .status
}

// ---------------------------------------------------------------- batch read

#[tokio::test]
async fn batch_read_keeps_request_order_and_labels_each_section() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    for name in ["a.txt", "b.txt", "c.txt"] {
        std::fs::write(root.join(name), format!("content of {name}\n")).unwrap();
    }
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());

    let tc = make_tc(
        "read",
        serde_json::json!({
            "paths": [
                { "path": root.join("c.txt").to_string_lossy() },
                { "path": root.join("a.txt").to_string_lossy() },
                { "path": root.join("b.txt").to_string_lossy() },
            ],
            "line_numbers": false
        }),
    );
    let outcome = run(&primitive, &state, &tc).await;
    assert!(!outcome.is_error, "batch read should succeed");

    let text = &outcome.model_text;
    let c = text.find("content of c.txt").expect("c present");
    let a = text.find("content of a.txt").expect("a present");
    let b = text.find("content of b.txt").expect("b present");
    assert!(c < a && a < b, "sections must follow request order: {text}");
    assert!(text.contains("[1/3]") && text.contains("[2/3]") && text.contains("[3/3]"));

    let entries = files_display(&outcome);
    assert_eq!(entries.len(), 3);
    assert!(entries[0].file.ends_with("c.txt"));
    assert!(
        entries.iter().all(|e| e.range.is_some()),
        "each read entry carries its line range: {entries:?}"
    );
    assert_eq!(
        state.len(),
        3,
        "every file in the batch gets its own read stamp"
    );
}

#[tokio::test]
async fn batch_read_marks_overflow_entries_skipped_with_resume_hint() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    // 第一个文件就足以吃光 128 KiB 预算，后面两个必须被显式跳过而不是静默丢掉。
    let big: String = (0..2000)
        .map(|i| format!("{i:0>99}\n"))
        .collect::<Vec<_>>()
        .concat();
    std::fs::write(root.join("big.txt"), &big).unwrap();
    std::fs::write(root.join("small.txt"), "small\n").unwrap();
    std::fs::write(root.join("tiny.txt"), "tiny\n").unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());

    let tc = make_tc(
        "read",
        serde_json::json!({
            "paths": [
                { "path": root.join("big.txt").to_string_lossy() },
                { "path": root.join("small.txt").to_string_lossy(), "offset": 1 },
                { "path": root.join("tiny.txt").to_string_lossy() },
            ],
            "line_numbers": false
        }),
    );
    let outcome = run(&primitive, &state, &tc).await;
    assert!(!outcome.is_error);

    assert!(outcome
        .model_text
        .contains("SKIPPED: output budget exhausted"));
    assert!(
        outcome.model_text.contains("offset=1"),
        "skipped entry must hand back a resume call that keeps its window: {}",
        outcome.model_text
    );

    let entries = files_display(&outcome);
    assert_eq!(status_of(&entries, "big.txt"), None);
    assert_eq!(
        status_of(&entries, "small.txt"),
        Some(ToolDisplayFileStatus::Skipped)
    );
    assert_eq!(
        status_of(&entries, "tiny.txt"),
        Some(ToolDisplayFileStatus::Skipped)
    );
    assert!(entries
        .iter()
        .filter(|e| e.status == Some(ToolDisplayFileStatus::Skipped))
        .all(|e| e.note.as_deref().is_some_and(|n| n.contains("resume:"))));
}

#[tokio::test]
async fn batch_read_reports_per_file_failure_without_failing_the_batch() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("ok.txt"), "ok\n").unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());

    let tc = make_tc(
        "read",
        serde_json::json!({
            "paths": [
                { "path": root.join("missing.txt").to_string_lossy() },
                { "path": root.join("ok.txt").to_string_lossy() },
            ],
            "line_numbers": false
        }),
    );
    let outcome = run(&primitive, &state, &tc).await;
    assert!(!outcome.is_error, "one bad path must not sink the batch");
    assert!(outcome.model_text.contains("FAILED"));
    assert!(outcome.model_text.contains("ok"));

    let entries = files_display(&outcome);
    assert_eq!(
        status_of(&entries, "missing.txt"),
        Some(ToolDisplayFileStatus::Failed)
    );
    assert_eq!(status_of(&entries, "ok.txt"), None);
}

/// 顶层 `path` 与 `paths` 同时给出时按「都要」处理，不罚一次往返。
///
/// 实测里模型会把批量的第一个文件顺手也填进单文件槽位（重复），偶尔也会填一个批量
/// 里没有的文件（多出来的）。两种都不是意图冲突，而读又没有副作用。
#[tokio::test]
async fn batch_read_merges_a_stray_top_level_path_instead_of_rejecting() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    std::fs::write(root.join("a.txt"), "aaa\n").unwrap();
    std::fs::write(root.join("b.txt"), "bbb\n").unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());

    let duplicate = run(
        &primitive,
        &state,
        &make_tc(
            "read",
            serde_json::json!({
                "path": root.join("a.txt").to_string_lossy(),
                "paths": [{ "path": root.join("a.txt").to_string_lossy() }],
                "line_numbers": false
            }),
        ),
    )
    .await;
    assert!(!duplicate.is_error, "{}", duplicate.model_text);
    assert_eq!(files_display(&duplicate).len(), 1, "重复的不该读两遍");

    let extra = run(
        &primitive,
        &state,
        &make_tc(
            "read",
            serde_json::json!({
                "path": root.join("b.txt").to_string_lossy(),
                "paths": [{ "path": root.join("a.txt").to_string_lossy() }],
                "line_numbers": false
            }),
        ),
    )
    .await;
    assert!(!extra.is_error, "{}", extra.model_text);
    let files: Vec<String> = files_display(&extra)
        .iter()
        .map(|e| e.file.clone())
        .collect();
    assert_eq!(files.len(), 2, "多出来的那个不能被丢掉: {files:?}");
    assert!(files[0].ends_with("b.txt"), "{files:?}");
}

// ---------------------------------------------------------------- batch edit

/// 先 read 一遍，拿到 edit 需要的 ReadStamp。
async fn seed_read(
    primitive: &Arc<dyn PrimitiveExecutor>,
    state: &Arc<ReadFileState>,
    path: &std::path::Path,
) {
    let tc = make_tc(
        "read",
        serde_json::json!({ "path": path.to_string_lossy(), "line_numbers": false }),
    );
    let outcome = run(primitive, state, &tc).await;
    assert!(
        !outcome.is_error,
        "seed read failed: {}",
        outcome.model_text
    );
}

#[tokio::test]
async fn batch_edit_applies_every_passing_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let one = root.join("one.txt");
    let two = root.join("two.txt");
    std::fs::write(&one, "alpha\nbeta\n").unwrap();
    std::fs::write(&two, "gamma\n").unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());
    seed_read(&primitive, &state, &one).await;
    seed_read(&primitive, &state, &two).await;

    let tc = make_tc(
        "edit",
        serde_json::json!({
            "files": [
                {
                    "path": one.to_string_lossy(),
                    "edits": [
                        { "old_content": "alpha", "new_content": "ALPHA" },
                        { "old_content": "beta", "new_content": "BETA" }
                    ]
                },
                {
                    "path": two.to_string_lossy(),
                    "old_content": "gamma",
                    "new_content": "GAMMA"
                }
            ]
        }),
    );
    let outcome = run(&primitive, &state, &tc).await;
    assert!(!outcome.is_error, "{}", outcome.model_text);
    assert_eq!(std::fs::read_to_string(&one).unwrap(), "ALPHA\nBETA\n");
    assert_eq!(std::fs::read_to_string(&two).unwrap(), "GAMMA\n");

    let entries = files_display(&outcome);
    assert_eq!(entries.len(), 2);
    assert!(entries
        .iter()
        .all(|e| e.status == Some(ToolDisplayFileStatus::Applied)));
    assert!(outcome.model_text.contains("all changes written"));
}

#[tokio::test]
async fn batch_edit_reports_all_precheck_failures_in_one_shot() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let good = root.join("good.txt");
    let unread = root.join("unread.txt");
    let notebook = root.join("nb.ipynb");
    std::fs::write(&good, "keep\n").unwrap();
    std::fs::write(&unread, "untouched\n").unwrap();
    std::fs::write(&notebook, "{}\n").unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());
    seed_read(&primitive, &state, &good).await;

    let tc = make_tc(
        "edit",
        serde_json::json!({
            "files": [
                { "path": unread.to_string_lossy(), "old_content": "untouched", "new_content": "x" },
                { "path": good.to_string_lossy(), "old_content": "keep", "new_content": "KEPT" },
                { "path": notebook.to_string_lossy(), "old_content": "{}", "new_content": "[]" }
            ]
        }),
    );
    let outcome = run(&primitive, &state, &tc).await;
    assert!(!outcome.is_error);

    // 两个失败项在同一次返回里都报出来了，模型不用靠反复试错逐个发现。
    assert!(
        outcome.model_text.contains("NoPriorRead"),
        "{}",
        outcome.model_text
    );
    assert!(
        outcome.model_text.contains("Notebook"),
        "{}",
        outcome.model_text
    );
    // 部分落盘：通过预检的照常写，失败的磁盘内容原样保留。
    assert_eq!(std::fs::read_to_string(&good).unwrap(), "KEPT\n");
    assert_eq!(std::fs::read_to_string(&unread).unwrap(), "untouched\n");
    assert_eq!(std::fs::read_to_string(&notebook).unwrap(), "{}\n");
    assert!(
        outcome.model_text.contains("failed without writing: 2"),
        "结果必须把磁盘状态说死：{}",
        outcome.model_text
    );

    let entries = files_display(&outcome);
    assert_eq!(
        status_of(&entries, "good.txt"),
        Some(ToolDisplayFileStatus::Applied)
    );
    assert_eq!(
        status_of(&entries, "unread.txt"),
        Some(ToolDisplayFileStatus::Failed)
    );
    assert_eq!(
        status_of(&entries, "nb.ipynb"),
        Some(ToolDisplayFileStatus::Failed)
    );
}

#[tokio::test]
async fn batch_edit_is_atomic_within_a_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let target = root.join("atomic.txt");
    let other = root.join("other.txt");
    std::fs::write(&target, "first\nsecond\n").unwrap();
    std::fs::write(&other, "ok\n").unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());
    seed_read(&primitive, &state, &target).await;
    seed_read(&primitive, &state, &other).await;

    let tc = make_tc(
        "edit",
        serde_json::json!({
            "files": [
                {
                    "path": target.to_string_lossy(),
                    "edits": [
                        { "old_content": "first", "new_content": "FIRST" },
                        { "old_content": "nowhere-to-be-found", "new_content": "boom" }
                    ]
                },
                { "path": other.to_string_lossy(), "old_content": "ok", "new_content": "OK" }
            ]
        }),
    );
    let outcome = run(&primitive, &state, &tc).await;

    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "first\nsecond\n",
        "一个段落匹配失败，这个文件的所有段落都不能落盘"
    );
    assert_eq!(
        std::fs::read_to_string(&other).unwrap(),
        "OK\n",
        "同批的其它文件不受牵连"
    );
    let entries = files_display(&outcome);
    assert_eq!(
        status_of(&entries, "atomic.txt"),
        Some(ToolDisplayFileStatus::Failed)
    );
    assert_eq!(
        status_of(&entries, "other.txt"),
        Some(ToolDisplayFileStatus::Applied)
    );
}

#[tokio::test]
async fn batch_edit_refreshes_read_stamp_so_the_next_edit_is_not_stale() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let target = root.join("twice.txt");
    std::fs::write(&target, "one\ntwo\n").unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());
    seed_read(&primitive, &state, &target).await;

    let first = run(
        &primitive,
        &state,
        &make_tc(
            "edit",
            serde_json::json!({
                "files": [{
                    "path": target.to_string_lossy(),
                    "old_content": "one",
                    "new_content": "ONE"
                }]
            }),
        ),
    )
    .await;
    assert!(!first.is_error, "{}", first.model_text);

    // 没有 F2-c 的 stamp 刷新，这一步会被 Stale 挡下 —— 而改动正是上一步我们自己做的。
    let second = run(
        &primitive,
        &state,
        &make_tc(
            "edit",
            serde_json::json!({
                "files": [{
                    "path": target.to_string_lossy(),
                    "old_content": "two",
                    "new_content": "TWO"
                }]
            }),
        ),
    )
    .await;
    assert!(!second.is_error, "{}", second.model_text);
    assert!(
        !second.model_text.contains("Stale"),
        "落盘后必须刷新 ReadStamp：{}",
        second.model_text
    );
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "ONE\nTWO\n");
}

#[tokio::test]
async fn batch_edit_rejects_duplicate_paths_but_tolerates_a_bare_repeated_path() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let target = root.join("dup.txt");
    std::fs::write(&target, "a\n").unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());
    seed_read(&primitive, &state, &target).await;

    let dup = run(
        &primitive,
        &state,
        &make_tc(
            "edit",
            serde_json::json!({
                "files": [
                    { "path": target.to_string_lossy(), "old_content": "a", "new_content": "b" },
                    { "path": target.to_string_lossy(), "old_content": "b", "new_content": "c" }
                ]
            }),
        ),
    )
    .await;
    assert!(dup.is_error);
    assert!(
        dup.model_text.contains("appears more than once in files"),
        "{}",
        dup.model_text
    );
    assert_eq!(
        std::fs::read_to_string(&target).unwrap(),
        "a\n",
        "入参非法时不能有任何落盘"
    );

    // 顶层只是把 files[0] 的路径又写了一遍，没有带上任何自己的编辑段 —— 意图并不含糊，
    // 照做就是了。真歧义（顶层带着 files 里没有的编辑）另有用例覆盖。
    let repeated_path = run(
        &primitive,
        &state,
        &make_tc(
            "edit",
            serde_json::json!({
                "path": target.to_string_lossy(),
                "files": [{ "path": target.to_string_lossy(), "old_content": "a", "new_content": "b" }]
            }),
        ),
    )
    .await;
    assert!(!repeated_path.is_error, "{}", repeated_path.model_text);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "b\n");
}

// -------------------------------------- strict schema 的空壳字段（真实事故回放）
//
// 走 strict schema 的模型会把它没用到的那套字段也一并填上空值。下面两条用例的入参
// 是 2026-07-27 冒烟里 gpt-5.6-sol 实际发出来的形状：批量编辑连续失败 16 次，全部
// 卡在「`path`/`edits` 与 `files` 互斥」和「files[0] 缺少非空的 `path`」上。
// 形态必须由「内容在哪」决定，而不是由「哪个键出现过」决定。

#[tokio::test]
async fn batch_edit_ignores_the_empty_edits_placeholder_next_to_files() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let one = root.join("one.txt");
    let two = root.join("two.txt");
    std::fs::write(&one, "alpha\n").unwrap();
    std::fs::write(&two, "gamma\n").unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());
    seed_read(&primitive, &state, &one).await;
    seed_read(&primitive, &state, &two).await;

    let tc = make_tc(
        "edit",
        serde_json::json!({
            "edits": [{ "old_content": "", "new_content": "", "replace_all": false }],
            "new_content": "",
            "old_content": "",
            "files": [
                {
                    "path": one.to_string_lossy(),
                    "edits": [{ "old_content": "alpha", "new_content": "ALPHA", "replace_all": false }],
                    "old_content": "",
                    "new_content": "",
                    "replace_all": false
                },
                {
                    "path": two.to_string_lossy(),
                    "edits": [{ "old_content": "gamma", "new_content": "GAMMA", "replace_all": false }],
                    "old_content": "",
                    "new_content": "",
                    "replace_all": false
                }
            ]
        }),
    );
    let outcome = run(&primitive, &state, &tc).await;
    assert!(!outcome.is_error, "{}", outcome.model_text);
    assert_eq!(std::fs::read_to_string(&one).unwrap(), "ALPHA\n");
    assert_eq!(std::fs::read_to_string(&two).unwrap(), "GAMMA\n");
}

/// 模型把整批编辑原样抄进顶层槽位时不算歧义，照批量做。
///
/// 2026-07-27 冒烟第二次撞上的形状：`files` 里两个文件，顶层 `edits` 是这两段的逐字
/// 拷贝，`path` 填第一个文件。同一个意图写了两遍，不是两套意图。
#[tokio::test]
async fn batch_edit_tolerates_the_top_level_copy_of_the_same_intent() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let one = root.join("one.txt");
    let two = root.join("two.txt");
    std::fs::write(&one, "alpha\n").unwrap();
    std::fs::write(&two, "gamma\n").unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());
    seed_read(&primitive, &state, &one).await;
    seed_read(&primitive, &state, &two).await;

    let tc = make_tc(
        "edit",
        serde_json::json!({
            "path": one.to_string_lossy(),
            "edits": [
                { "old_content": "alpha", "new_content": "ALPHA", "replace_all": false },
                { "old_content": "gamma", "new_content": "GAMMA", "replace_all": false }
            ],
            "files": [
                {
                    "path": one.to_string_lossy(),
                    "edits": [{ "old_content": "alpha", "new_content": "ALPHA", "replace_all": false }]
                },
                {
                    "path": two.to_string_lossy(),
                    "edits": [{ "old_content": "gamma", "new_content": "GAMMA", "replace_all": false }]
                }
            ]
        }),
    );
    let outcome = run(&primitive, &state, &tc).await;
    assert!(!outcome.is_error, "{}", outcome.model_text);
    assert_eq!(std::fs::read_to_string(&one).unwrap(), "ALPHA\n");
    assert_eq!(std::fs::read_to_string(&two).unwrap(), "GAMMA\n");
}

/// 顶层真有一段别处没有的编辑时仍然拒绝：edit 会落盘，两套意图必须问清楚。
#[tokio::test]
async fn batch_edit_still_rejects_a_top_level_edit_that_files_does_not_contain() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let one = root.join("one.txt");
    let other = root.join("other.txt");
    std::fs::write(&one, "alpha\n").unwrap();
    std::fs::write(&other, "delta\n").unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());
    seed_read(&primitive, &state, &one).await;
    seed_read(&primitive, &state, &other).await;

    let tc = make_tc(
        "edit",
        serde_json::json!({
            "path": other.to_string_lossy(),
            "old_content": "delta",
            "new_content": "DELTA",
            "files": [{
                "path": one.to_string_lossy(),
                "edits": [{ "old_content": "alpha", "new_content": "ALPHA", "replace_all": false }]
            }]
        }),
    );
    let outcome = run(&primitive, &state, &tc).await;
    assert!(outcome.is_error);
    assert!(
        outcome.model_text.contains("is not present in `files`"),
        "{}",
        outcome.model_text
    );
    assert_eq!(
        std::fs::read_to_string(&one).unwrap(),
        "alpha\n",
        "入参有歧义时不能有任何落盘"
    );
}

/// 模型也会拿字面量 `"placeholder"` 去填它用不到的槽位。
#[tokio::test]
async fn batch_edit_ignores_a_literal_placeholder_segment() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let one = root.join("one.txt");
    std::fs::write(&one, "alpha\n").unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());
    seed_read(&primitive, &state, &one).await;

    let tc = make_tc(
        "edit",
        serde_json::json!({
            "path": "",
            "edits": [{ "old_content": "placeholder", "new_content": "placeholder", "replace_all": false }],
            "files": [{
                "path": one.to_string_lossy(),
                "edits": [{ "old_content": "alpha", "new_content": "ALPHA", "replace_all": false }]
            }]
        }),
    );
    let outcome = run(&primitive, &state, &tc).await;
    assert!(!outcome.is_error, "{}", outcome.model_text);
    assert_eq!(std::fs::read_to_string(&one).unwrap(), "ALPHA\n");
}

#[tokio::test]
async fn single_edit_ignores_the_empty_files_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let target = root.join("only.txt");
    std::fs::write(&target, "alpha\n").unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());
    seed_read(&primitive, &state, &target).await;

    let tc = make_tc(
        "edit",
        serde_json::json!({
            "path": target.to_string_lossy(),
            "edits": [{ "old_content": "alpha", "new_content": "ALPHA", "replace_all": false }],
            "files": [{
                "path": "",
                "edits": [{ "old_content": "", "new_content": "", "replace_all": false }],
                "old_content": "",
                "new_content": "",
                "replace_all": false
            }]
        }),
    );
    let outcome = run(&primitive, &state, &tc).await;
    assert!(!outcome.is_error, "{}", outcome.model_text);
    assert_eq!(std::fs::read_to_string(&target).unwrap(), "ALPHA\n");
}

#[tokio::test]
async fn single_read_ignores_the_empty_paths_placeholder() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let file = root.join("solo.txt");
    std::fs::write(&file, "one\ntwo\n").unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());

    let tc = make_tc(
        "read",
        serde_json::json!({
            "path": file.to_string_lossy(),
            "paths": [{ "path": "" }],
            "line_numbers": false
        }),
    );
    let outcome = run(&primitive, &state, &tc).await;
    assert!(!outcome.is_error, "{}", outcome.model_text);
    assert!(outcome.model_text.contains("one"), "{}", outcome.model_text);
}

// ------------------------------------------------------- F3 区间包含与失效

#[tokio::test]
async fn read_dedup_hits_when_the_window_falls_inside_an_earlier_read() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let file = root.join("wide.txt");
    let body: String = (1..=200).map(|i| format!("line {i}\n")).collect();
    std::fs::write(&file, body).unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());

    let wide = make_tc(
        "read",
        serde_json::json!({
            "path": file.to_string_lossy(), "offset": 100, "limit": 50, "line_numbers": false
        }),
    );
    let first = run(&primitive, &state, &wide).await;
    assert!(first.model_text.contains("line 100"));

    // 更窄的窗口落在已读区间里：内容就在上下文里，不该再读一遍。
    let narrow = make_tc(
        "read",
        serde_json::json!({
            "path": file.to_string_lossy(), "offset": 110, "limit": 10, "line_numbers": false
        }),
    );
    let second = run(&primitive, &state, &narrow).await;
    assert!(
        second.model_text.contains("earlier read covered L100-149"),
        "{}",
        second.model_text
    );

    // 越界的窗口有一半没读过，必须真读。
    let overlapping = make_tc(
        "read",
        serde_json::json!({
            "path": file.to_string_lossy(), "offset": 140, "limit": 30, "line_numbers": false
        }),
    );
    let third = run(&primitive, &state, &overlapping).await;
    assert!(
        third.model_text.contains("line 160"),
        "部分重叠不能命中去重: {}",
        third.model_text
    );
}

#[tokio::test]
async fn read_stamp_dies_with_the_tool_result_it_came_from() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let file = root.join("evicted.txt");
    std::fs::write(&file, "alpha\nbeta\n").unwrap();
    let primitive = make_executor(&root);
    let state = Arc::new(ReadFileState::new());

    let tc = make_tc(
        "read",
        serde_json::json!({ "path": file.to_string_lossy(), "line_numbers": false }),
    );
    run(&primitive, &state, &tc).await;
    assert_eq!(state.len(), 1);

    // 结果被落盘/换成占位符之后，"参考上一次读取结果" 指向的东西已经不存在了。
    assert_eq!(state.invalidate_tool_call(&tc.id), 1);

    let again = run(&primitive, &state, &tc).await;
    assert!(
        again.model_text.contains("alpha"),
        "结果被驱逐后必须真读，而不是回一句和上次一样: {}",
        again.model_text
    );
}
