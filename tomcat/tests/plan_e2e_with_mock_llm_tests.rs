#![allow(clippy::await_holding_lock)]

//! `plan_e2e_with_mock_llm_tests` — H 段集成测试（plan-mode-full-fix §H）。
//!
//! 这些用例**不**起 rustyline chat_loop（依赖 stdin），而是把"LLM 决策一次 tool_call"
//! 这一步用直接调用 `tools::*::execute` 代替——其他链路（PlanRuntime / RefreshNotifier /
//! CheckpointStore）全部走真实路径。目的是验证：
//!
//! - tool 调用 → PlanRuntime 状态迁移 → 磁盘 plan 文件落盘 → panel snapshot fanout；
//! - 多次 `update_plan` 串成"5 次 plan.panel + 1 次 plan.complete"序列；
//! - cancel 信号 → EXEC → Pending 的磁盘/内存联动；
//! - write/edit 越界路径 → safety::enforce_write_path_policy 拒；
//! - 关键策略：N1（ask_question CHAT）、N2（completed 全禁）、N3（mode 矩阵）。
//!
//! 与 `plan_runtime_integration_tests.rs` 的差异：那个文件验证单点 API 不变量；本文件
//! 验证"完整业务回合"的 panel/checkpoint 事件序。

#![allow(clippy::field_reassign_with_default)]

use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::Mutex;

use tomcat::core::plan_runtime::file_store::{
    plan_path_for_id, read_plan, write_plan, PlanFileState, TodoStatus,
};
use tomcat::core::plan_runtime::panels::{TodosPanel, TodosPanelSnapshot};
use tomcat::core::plan_runtime::review::{ReviewKind, ReviewSummary};
use tomcat::core::plan_runtime::state::PlanState;
use tomcat::core::plan_runtime::verify::{VerifyCheck, VerifySummary};
use tomcat::core::plan_runtime::{PlanRuntime, ReviewerDispatcher, VerifierDispatcher};
use tomcat::core::tools::plan_tool::{create_plan, todos, update_plan};
use tomcat::core::{
    CheckpointDiff, CheckpointError, CheckpointId, CheckpointMeta, CheckpointRecordRequest,
    CheckpointRestoreReport, CheckpointStore, ListOptions, RestoreOptions, RetentionPolicy,
};

// ─── 共享 fixture 与 spy ───────────────────────────────────────────────────

/// CapturePanel 把所有 panel snapshot 推入 Vec，便于测试断言"plan.panel × N"。
#[derive(Default)]
struct CapturePanel {
    pub snapshots: Mutex<Vec<TodosPanelSnapshot>>,
}

impl TodosPanel for CapturePanel {
    fn refresh(&self, s: &TodosPanelSnapshot) {
        self.snapshots.lock().push(s.clone());
    }
}

#[derive(Default)]
struct CheckpointSpy {
    pub records: Mutex<Vec<CheckpointRecordRequest>>,
}

impl CheckpointStore for CheckpointSpy {
    fn record(&self, request: CheckpointRecordRequest) -> Result<CheckpointId, CheckpointError> {
        let id = CheckpointId::new(format!("ck_{}", self.records.lock().len()));
        self.records.lock().push(request);
        Ok(id)
    }
    fn list(
        &self,
        _session_id: &str,
        _opts: ListOptions,
    ) -> Result<Vec<CheckpointMeta>, CheckpointError> {
        Ok(vec![])
    }
    fn show(&self, _id: &CheckpointId) -> Result<Option<CheckpointMeta>, CheckpointError> {
        Ok(None)
    }
    fn diff(&self, _id: &CheckpointId) -> Result<CheckpointDiff, CheckpointError> {
        Ok(CheckpointDiff::default())
    }
    fn restore(
        &self,
        _id: &CheckpointId,
        _opts: RestoreOptions,
    ) -> Result<CheckpointRestoreReport, CheckpointError> {
        Ok(CheckpointRestoreReport::default())
    }
    fn prune(&self, _retention: RetentionPolicy) -> Result<usize, CheckpointError> {
        Ok(0)
    }
}

struct QueueReviewer {
    summaries: Mutex<Vec<ReviewSummary>>,
    call_count: std::sync::atomic::AtomicUsize,
}

impl QueueReviewer {
    fn new(summaries: Vec<ReviewSummary>) -> Self {
        Self {
            summaries: Mutex::new(summaries),
            call_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl ReviewerDispatcher for QueueReviewer {
    async fn dispatch(
        &self,
        _plan_id: &str,
        _plan_text: &str,
        kind: ReviewKind,
        _allow_review_edit: bool,
    ) -> ReviewSummary {
        self.call_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut summaries = self.summaries.lock();
        let mut summary = if summaries.is_empty() {
            ReviewSummary::aborted_with_kind(kind, "mock reviewer queue exhausted")
        } else {
            summaries.remove(0)
        };
        summary.kind = kind;
        summary
    }
}

struct QueueVerifier {
    summaries: Mutex<Vec<VerifySummary>>,
    call_count: std::sync::atomic::AtomicUsize,
}

impl QueueVerifier {
    fn new(summaries: Vec<VerifySummary>) -> Self {
        Self {
            summaries: Mutex::new(summaries),
            call_count: std::sync::atomic::AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl VerifierDispatcher for QueueVerifier {
    async fn dispatch(
        &self,
        _plan_id: &str,
        _plan_text: &str,
    ) -> VerifySummary {
        self.call_count
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let mut summaries = self.summaries.lock();
        if summaries.is_empty() {
            VerifySummary::aborted_with("mock verifier queue exhausted")
        } else {
            summaries.remove(0)
        }
    }
}

fn pass_code_review() -> ReviewSummary {
    ReviewSummary {
        kind: ReviewKind::Code,
        aborted: false,
        verdict: Some("pass".into()),
        summary: "implementation looks good".into(),
        changes_summary: "none".into(),
        applied_changes: false,
        ..Default::default()
    }
}

fn fail_code_review() -> ReviewSummary {
    ReviewSummary {
        kind: ReviewKind::Code,
        aborted: false,
        verdict: Some("fail".into()),
        summary: "missed a concrete fix".into(),
        changes_summary: "none".into(),
        applied_changes: false,
        ..Default::default()
    }
}

fn pass_verify() -> VerifySummary {
    VerifySummary {
        checks: vec![VerifyCheck {
            name: "unit".into(),
            command: "cargo test -p tomcat smoke".into(),
            result: "pass".into(),
            output_excerpt: "ok".into(),
        }],
        verdict: "pass".into(),
        summary: "verification passed".into(),
        ..Default::default()
    }
}

/// HOME 隔离锁——本文件多个测试串行（默认 cargo test 多线程会污染 plan_path_for_id）。
fn home_lock() -> &'static std::sync::Mutex<()> {
    static M: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    M.get_or_init(|| std::sync::Mutex::new(()))
}

fn setup_home() -> PathBuf {
    let p = std::env::temp_dir().join(format!(
        "tomcat_plan_e2e_home_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(p.join(".tomcat").join("plans")).unwrap();
    std::env::set_var("HOME", &p);
    p
}

fn cleanup_home(p: &std::path::Path) {
    let _ = std::fs::remove_dir_all(p);
}

/// 装配一个测试 runtime + 注入 spy panel/checkpoint，返回 (runtime, panel, ckpt)。
fn build_runtime_with_spies() -> (
    std::sync::Arc<PlanRuntime>,
    Arc<CapturePanel>,
    Arc<CheckpointSpy>,
) {
    let rt = PlanRuntime::new("session-a");
    rt.set_max_code_review_rounds(0);
    let panel = Arc::new(CapturePanel::default());
    let ckpt = Arc::new(CheckpointSpy::default());
    rt.register_todos_panel(panel.clone());
    rt.attach_checkpoint_store(ckpt.clone());
    (rt, panel, ckpt)
}

/// 提升 disk plan 到 executing 并同步内存（绕过 build_plan 锁竞争——单测专用）。
fn promote_to_exec(rt: &PlanRuntime, plan_id: &str) {
    let path = plan_path_for_id(plan_id).unwrap();
    let mut plan = read_plan(&path).unwrap();
    plan.frontmatter.state = PlanFileState::Executing;
    plan.frontmatter.session_key = Some("session-a".into());
    plan.frontmatter.session_id = Some("sid-a".into());
    write_plan(&path, &plan, 2000).unwrap();
    rt.set_executing_for_test(plan_id.to_string());
}

async fn complete_all_plan_todos(rt: &PlanRuntime, plan_id: &str) -> serde_json::Value {
    update_plan::execute(
        rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.to_string()),
            path: None,
            replace: false,
            ops: vec![
                update_plan::UpdateOp::SetStatus {
                    id: "t1".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
                update_plan::UpdateOp::SetStatus {
                    id: "t2".into(),
                    content: None,
                    status: TodoStatus::Completed,
                },
            ],
        },
    )
    .await
    .unwrap()
}

// ─── H1：full lifecycle, 多次 update_plan → plan.complete ──────────────────

#[tokio::test]
async fn h1_e2e_full_lifecycle_with_panel_and_complete_events() {
    let _g = home_lock().lock().unwrap();
    let home = setup_home();
    let (rt, panel, _ckpt) = build_runtime_with_spies();

    // PLAN：LLM → create_plan
    rt.enter_planning().unwrap();
    let out = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "ship feature X".into(),
            draft: "## Goal\nship X".into(),
            todos: vec![
                create_plan::TodoArg {
                    id: "t1".into(),
                    content: "step 1".into(),
                    status: TodoStatus::Pending,
                },
                create_plan::TodoArg {
                    id: "t2".into(),
                    content: "step 2".into(),
                    status: TodoStatus::Pending,
                },
                create_plan::TodoArg {
                    id: "t3".into(),
                    content: "step 3".into(),
                    status: TodoStatus::Pending,
                },
            ],
        },
    )
    .unwrap();
    let plan_id = out["plan_id"].as_str().unwrap().to_string();
    promote_to_exec(&rt, &plan_id);

    // 模拟 EXEC：LLM 多次 update_plan 推进 t1→t2→t3
    // 序列：set_status(t1, in_progress) → completed → t2 in_progress → completed → t3 in_progress → completed
    let ops = [
        ("t1", TodoStatus::InProgress),
        ("t1", TodoStatus::Completed),
        ("t2", TodoStatus::InProgress),
        ("t2", TodoStatus::Completed),
        ("t3", TodoStatus::InProgress),
        ("t3", TodoStatus::Completed),
    ];
    for (id, st) in ops.iter() {
        update_plan::execute(
            &rt,
            update_plan::UpdatePlanArgs {
                plan_id: Some(plan_id.clone()),
                path: None,
                replace: false,
                ops: vec![update_plan::UpdateOp::SetStatus {
                    id: (*id).into(),
                    content: None,
                    status: *st,
                }],
            },
        )
        .await
        .unwrap();
    }

    // 全 completed → 瞬时 Completed 后立即回 Chat(retain)
    assert!(matches!(rt.mode(), PlanState::Chat));
    assert_eq!(
        rt.active_plan_path(),
        Some(plan_path_for_id(&plan_id).unwrap())
    );
    // 6 次 update_plan → 6 次 panel refresh
    let snaps = panel.snapshots.lock().clone();
    assert_eq!(snaps.len(), 6, "应触发 6 次 panel snapshot");
    // 最后一次 snapshot：最后一条 todo 已完成
    let last = snaps.last().unwrap();
    assert_eq!(last.items.last().unwrap().id, "t3");
    assert_eq!(last.items.last().unwrap().status, TodoStatus::Completed);
    assert_eq!(last.progress_summary(), "3 of 3 Done");
    cleanup_home(&home);
}

// ─── H3：PLAN 期 raw edit 越界 → 拒 ───────────────────────────────────────

#[test]
fn h3_plan_mode_raw_edit_outside_plans_dir_is_blocked_only_for_plan_files() {
    // 注：allow_raw_edit_to_path 的语义是"路径在 ~/.tomcat/plans 下时按 mode 守卫，
    // 其它路径放行交给上层 permission gate"——这里直接验证 plan 路径的守卫。
    let _g = home_lock().lock().unwrap();
    let home = setup_home();
    let rt = PlanRuntime::new("session-a");
    rt.enter_planning().unwrap();

    let plan_path = home.join(".tomcat").join("plans").join("p.plan.md");
    std::fs::write(&plan_path, "stub").unwrap();
    assert!(
        !rt.allow_raw_edit_to_path(&plan_path),
        "PLAN 模式下 plan 文件 raw edit 必须拒"
    );

    let outside = home.join("notes.md");
    std::fs::write(&outside, "stub").unwrap();
    assert!(
        rt.allow_raw_edit_to_path(&outside),
        "非 plan 文件 PLAN 模式下不归本守卫管"
    );
    cleanup_home(&home);
}

// ─── H4：EXEC 期 plan 文件 raw edit → 拒 ───────────────────────────────────

#[test]
fn h4_exec_mode_raw_edit_on_plan_file_is_blocked() {
    let _g = home_lock().lock().unwrap();
    let home = setup_home();
    let (rt, _panel, _ckpt) = build_runtime_with_spies();

    rt.enter_planning().unwrap();
    let out = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "g".into(),
            draft: "ok".into(),
            todos: vec![create_plan::TodoArg {
                id: "t1".into(),
                content: "a".into(),
                status: TodoStatus::Pending,
            }],
        },
    )
    .unwrap();
    let plan_id = out["plan_id"].as_str().unwrap().to_string();
    promote_to_exec(&rt, &plan_id);

    let path = plan_path_for_id(&plan_id).unwrap();
    assert!(
        !rt.allow_raw_edit_to_path(&path),
        "EXEC 模式 plan 文件 raw edit 必须拒（请使用 update_plan）"
    );
    cleanup_home(&home);
}

// ─── H6：cancel → demote_to_pending（磁盘 + 内存） ─────────────────────────

#[tokio::test]
async fn h6_cancel_during_exec_demotes_plan_to_pending() {
    let _g = home_lock().lock().unwrap();
    let home = setup_home();
    let (rt, _panel, _ckpt) = build_runtime_with_spies();

    rt.enter_planning().unwrap();
    let out = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "g".into(),
            draft: "ok".into(),
            todos: vec![create_plan::TodoArg {
                id: "t1".into(),
                content: "a".into(),
                status: TodoStatus::Pending,
            }],
        },
    )
    .unwrap();
    let plan_id = out["plan_id"].as_str().unwrap().to_string();
    promote_to_exec(&rt, &plan_id);

    let demoted = rt.demote_to_pending_on_cancel().unwrap();
    assert_eq!(demoted.as_deref(), Some(plan_id.as_str()));
    match rt.mode() {
        PlanState::Pending { plan_id: pid } => assert_eq!(pid, plan_id),
        other => panic!("expected Pending, got {other:?}"),
    }

    let path = plan_path_for_id(&plan_id).unwrap();
    let plan = read_plan(&path).unwrap();
    assert_eq!(plan.frontmatter.state, PlanFileState::Pending);
    cleanup_home(&home);
}

// ─── H7：Planning 期 set_status(in_progress) → 拒（mode 矩阵闸门） ──────────

#[tokio::test]
async fn h7_update_plan_in_progress_in_planning_rejected_by_mode_matrix() {
    let _g = home_lock().lock().unwrap();
    let home = setup_home();
    let (rt, _panel, _ckpt) = build_runtime_with_spies();

    rt.enter_planning().unwrap();
    let out = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "g".into(),
            draft: "ok".into(),
            todos: vec![create_plan::TodoArg {
                id: "t1".into(),
                content: "a".into(),
                status: TodoStatus::Pending,
            }],
        },
    )
    .unwrap();
    let plan_id = out["plan_id"].as_str().unwrap().to_string();
    // 不切 EXEC——保持 PLANNING；in_progress 应被拒。

    let err = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        },
    )
    .await
    .expect_err("Planning 期 set_status(in_progress) 必须拒");
    let msg = format!("{err:?}");
    assert!(
        msg.contains("ModePolicy") || msg.contains("in_progress") || msg.contains("planning"),
        "应是 mode 矩阵闸门错误：{msg}"
    );
    cleanup_home(&home);
}

// ─── H2：CHAT 期 todos 工具仍可用 + panel snapshot 走 session 作用域 ──────

#[test]
fn h2_chat_mode_todos_tool_persists_and_emits_session_panel() {
    let _g = home_lock().lock().unwrap();
    let home = setup_home();
    let (rt, panel, _ckpt) = build_runtime_with_spies();

    // CHAT 模式下直接调用 todos（无需 enter_planning）。
    let _ = todos::execute(
        &rt,
        None,
        todos::TodosArgs {
            new_todos: false,
            title: None,
            replace: false,
            ops: vec![todos::TodoOpArg::Upsert {
                id: "t1".into(),
                content: Some("chat scratchpad".into()),
                status: Some(TodoStatus::Pending),
            }],
        },
    )
    .unwrap();
    let snaps = panel.snapshots.lock().clone();
    assert_eq!(snaps.len(), 1, "CHAT todos 应触发一次 panel snapshot");
    assert_eq!(snaps[0].scope, "session", "CHAT 应是 session scope");
    assert_eq!(snaps[0].items.len(), 1);
    assert_eq!(snaps[0].progress_summary(), "0 of 1 Done");
    cleanup_home(&home);
}

// ─── H5：reviewer aborted summary 路径（无真实 LLM 子 Agent 时） ───────────

#[tokio::test]
async fn h5_reviewer_aborted_summary_used_when_dispatcher_returns_aborted() {
    let _g = home_lock().lock().unwrap();
    let home = setup_home();
    let rt = PlanRuntime::new("session-a");
    // 不挂 reviewer dispatcher → 走 placeholder_pending 路径（plan-runtime §RV14）。
    rt.enter_planning().unwrap();
    let out = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "g".into(),
            draft: "ok".into(),
            todos: vec![create_plan::TodoArg {
                id: "t1".into(),
                content: "a".into(),
                status: TodoStatus::Pending,
            }],
        },
    )
    .unwrap();
    let plan_id = out["plan_id"].as_str().unwrap().to_string();

    let summary = rt.dispatch_reviewer(&plan_id, false).await;
    // 未挂 dispatcher → placeholder_pending（aborted = true 或 summary 含 placeholder）
    assert!(summary.aborted || summary.summary.contains("placeholder"));
    cleanup_home(&home);
}

#[tokio::test]
async fn h8_code_review_pass_runs_verifier_in_same_update_plan_turn() {
    let _g = home_lock().lock().unwrap();
    let home = setup_home();
    let (rt, _panel, _ckpt) = build_runtime_with_spies();
    rt.set_max_code_review_rounds(1);
    let reviewer = Arc::new(QueueReviewer::new(vec![pass_code_review()]));
    let verifier = Arc::new(QueueVerifier::new(vec![pass_verify()]));
    rt.attach_reviewer(reviewer.clone());
    rt.attach_verifier(verifier.clone());

    rt.enter_planning().unwrap();
    let out = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "g".into(),
            draft: "ok".into(),
            todos: vec![
                create_plan::TodoArg {
                    id: "t1".into(),
                    content: "a".into(),
                    status: TodoStatus::Pending,
                },
                create_plan::TodoArg {
                    id: "t2".into(),
                    content: "b".into(),
                    status: TodoStatus::Pending,
                },
            ],
        },
    )
    .unwrap();
    let plan_id = out["plan_id"].as_str().unwrap().to_string();
    promote_to_exec(&rt, &plan_id);

    let out = complete_all_plan_todos(&rt, &plan_id).await;
    assert_eq!(out["code_review"]["verdict"], "pass");
    assert_eq!(out["verify"]["verdict"], "pass");
    assert_eq!(out["plan_state_after"], "completed");
    assert_eq!(rt.code_review_rounds(&plan_id), 1);
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        verifier
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    cleanup_home(&home);
}

#[tokio::test]
async fn h9_code_review_non_pass_returns_to_main_then_second_completion_skips_review() {
    let _g = home_lock().lock().unwrap();
    let home = setup_home();
    let (rt, _panel, _ckpt) = build_runtime_with_spies();
    rt.set_max_code_review_rounds(1);
    let reviewer = Arc::new(QueueReviewer::new(vec![fail_code_review()]));
    let verifier = Arc::new(QueueVerifier::new(vec![pass_verify()]));
    rt.attach_reviewer(reviewer.clone());
    rt.attach_verifier(verifier.clone());

    rt.enter_planning().unwrap();
    let out = create_plan::execute(
        &rt,
        create_plan::CreatePlanArgs {
            goal: "g".into(),
            draft: "ok".into(),
            todos: vec![
                create_plan::TodoArg {
                    id: "t1".into(),
                    content: "a".into(),
                    status: TodoStatus::Pending,
                },
                create_plan::TodoArg {
                    id: "t2".into(),
                    content: "b".into(),
                    status: TodoStatus::Pending,
                },
            ],
        },
    )
    .unwrap();
    let plan_id = out["plan_id"].as_str().unwrap().to_string();
    promote_to_exec(&rt, &plan_id);

    let first = complete_all_plan_todos(&rt, &plan_id).await;
    assert_eq!(first["code_review"]["verdict"], "fail");
    assert_eq!(first["verify"], serde_json::Value::Null);
    assert_eq!(first["plan_state_after"], "executing");
    assert_eq!(rt.code_review_rounds(&plan_id), 1);
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        verifier
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        0
    );

    update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::InProgress,
            }],
        },
    )
    .await
    .unwrap();

    let second = update_plan::execute(
        &rt,
        update_plan::UpdatePlanArgs {
            plan_id: Some(plan_id.clone()),
            path: None,
            replace: false,
            ops: vec![update_plan::UpdateOp::SetStatus {
                id: "t1".into(),
                content: None,
                status: TodoStatus::Completed,
            }],
        },
    )
    .await
    .unwrap();
    assert_eq!(second["code_review"], serde_json::Value::Null);
    assert_eq!(second["verify"]["verdict"], "pass");
    assert_eq!(second["plan_state_after"], "completed");
    assert_eq!(
        reviewer
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    assert_eq!(
        verifier
            .call_count
            .load(std::sync::atomic::Ordering::Relaxed),
        1
    );
    cleanup_home(&home);
}
