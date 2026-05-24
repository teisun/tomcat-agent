use super::super::resume::{compute_resume_plan, ResumePlan};

#[test]
fn resume_plan_always_continue() {
    assert!(matches!(
        compute_resume_plan(None, &[]),
        ResumePlan::Continue
    ));
}
