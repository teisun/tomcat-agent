use super::super::file_store::{
    apply_close_out_transition, close_out_is_consistent, CloseOutTransition, GreenBuildEvidence,
    TodoItem, TodoKind, TodoStatus, GATE_ACCEPTANCE_TODO_CONTENT, GATE_ACCEPTANCE_TODO_ID,
    GATE_CODE_REVIEW_TODO_CONTENT, GATE_CODE_REVIEW_TODO_ID,
};
use super::sample_frontmatter;

fn frontmatter_with_gates() -> super::super::file_store::PlanFileFrontmatter {
    let mut frontmatter = sample_frontmatter();
    frontmatter.todos = vec![
        TodoItem {
            id: "work".into(),
            content: "implement".into(),
            status: TodoStatus::Completed,
            evidence: Vec::new(),
            kind: TodoKind::Work,
        },
        TodoItem {
            id: GATE_CODE_REVIEW_TODO_ID.into(),
            content: GATE_CODE_REVIEW_TODO_CONTENT.into(),
            status: TodoStatus::Pending,
            evidence: Vec::new(),
            kind: TodoKind::GateCodeReview,
        },
        TodoItem {
            id: GATE_ACCEPTANCE_TODO_ID.into(),
            content: GATE_ACCEPTANCE_TODO_CONTENT.into(),
            status: TodoStatus::Pending,
            evidence: Vec::new(),
            kind: TodoKind::GateAcceptance,
        },
    ];
    frontmatter
}

#[test]
fn every_close_out_transition_keeps_flags_and_gates_consistent() {
    let mut frontmatter = frontmatter_with_gates();
    let transitions = vec![
        CloseOutTransition::ReviewRestarted,
        CloseOutTransition::ReviewAborted,
        CloseOutTransition::ReviewFailed {
            findings: Vec::new(),
        },
        CloseOutTransition::ReviewPassed,
        CloseOutTransition::ReviewExhaustedFailOpen {
            residual_findings: vec!["F01 [P1] tests: accepted".into()],
        },
        CloseOutTransition::AcceptanceStarted,
        CloseOutTransition::AcceptanceFailed,
        CloseOutTransition::AcceptancePassed {
            evidence: vec![GreenBuildEvidence {
                command: "cargo test".into(),
                task_id: "task-1".into(),
                started_at_ms: 0,
                exit_code: 0,
            }],
        },
    ];

    for transition in transitions {
        apply_close_out_transition(&mut frontmatter, transition);
        assert!(
            close_out_is_consistent(&frontmatter),
            "transition must preserve durable consistency: {frontmatter:?}"
        );
    }

    let mut handed_off = frontmatter_with_gates();
    apply_close_out_transition(
        &mut handed_off,
        CloseOutTransition::ReviewHandedOff {
            residual_findings: vec!["F02 [P0] security: fix required".into()],
        },
    );
    assert!(close_out_is_consistent(&handed_off));

    let mut skipped = frontmatter_with_gates();
    apply_close_out_transition(&mut skipped, CloseOutTransition::AllGatesSkipped);
    assert!(close_out_is_consistent(&skipped));
}
