use super::*;

#[test]
fn plan_path_for_id_rejects_unsafe() {
    let err = plan_path_for_id("../etc/passwd").expect_err("应拒穿越");
    assert!(
        matches!(err, PlanError::InvalidPlanId { .. }),
        "got {err:?}"
    );

    let err = plan_path_for_id("a/b").expect_err("应拒斜杠");
    assert!(
        matches!(err, PlanError::InvalidPlanId { .. }),
        "got {err:?}"
    );
}

#[test]
fn plan_file_path_fixed_under_dot_tomcat() {
    let path = plan_path_for_id("safe_id_1").unwrap();
    let canonical = path.to_string_lossy();
    assert!(
        canonical.contains(".tomcat")
            && canonical.contains("plans")
            && canonical.ends_with("safe_id_1.plan.md"),
        "plan 文件路径必须位于 ~/.tomcat/plans/，实际：{canonical}"
    );
}
