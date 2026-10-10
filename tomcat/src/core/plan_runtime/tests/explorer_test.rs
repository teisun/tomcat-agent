use super::*;

fn report(id: &str, body: &str) -> ExplorerReport {
    ExplorerReport {
        id: id.to_string(),
        aborted: false,
        report: body.to_string(),
        turns_used: 3,
        turns_limit: 64,
        stop_reason: "completed".into(),
        child_session_id: "child-1".into(),
    }
}

const GOOD_REPORT: &str = "## Findings\n- `src/a.rs:10-42` — parses the wire frame.\n\n## Conclusion\nIt is parsed in a.rs.\n\n## Open questions\n- none\n";

#[test]
fn explorer_tools_are_read_only() {
    for forbidden in ["write", "edit", "delete", "update_plan", "dispatch_agent"] {
        assert!(
            !EXPLORER_ALLOWED_TOOLS.contains(&forbidden),
            "explorer must not expose {forbidden}"
        );
    }
    for allowed in ["read", "search_files", "list_dir", "bash"] {
        assert!(EXPLORER_ALLOWED_TOOLS.contains(&allowed));
    }
}

#[test]
fn contract_check_accepts_a_well_formed_report() {
    assert!(contract_violations(GOOD_REPORT).is_empty());
}

#[test]
fn contract_check_flags_missing_sections_and_pasted_code() {
    let missing = contract_violations("just some prose");
    assert_eq!(missing.len(), 2, "{missing:?}");

    let pasted = format!(
        "{GOOD_REPORT}\n```rust\nfn a() {{}}\nfn b() {{}}\nfn c() {{}}\nfn d() {{}}\n```\n"
    );
    let issues = contract_violations(&pasted);
    assert_eq!(issues.len(), 1, "{issues:?}");
    assert!(issues[0].contains("code block"), "{issues:?}");

    // 只开不闭的围栏同样要被抓到，否则少写三个反引号就能绕过检查。
    let unclosed = format!("{GOOD_REPORT}\n```rust\na\nb\nc\nd\n");
    assert_eq!(contract_violations(&unclosed).len(), 1);

    // 一两行的短片段是允许的，说明"某个具体 token"时需要。
    let short_snippet = format!("{GOOD_REPORT}\n```\nmax_rounds = 8\n```\n");
    assert!(contract_violations(&short_snippet).is_empty());
}

#[test]
fn rendered_reports_keep_task_ids_and_surface_failures() {
    let cause = "spawn 失败";
    let rendered = render_reports(&[
        report("webview", GOOD_REPORT),
        ExplorerReport::aborted_with("rust-api", cause),
    ]);

    assert!(rendered.contains("=== [1/2] webview ==="), "{rendered}");
    assert!(rendered.contains("=== [2/2] rust-api ==="), "{rendered}");
    assert!(
        rendered.contains(&format!("ABORTED: {cause}")),
        "{rendered}"
    );
}

#[test]
fn rendered_reports_warn_when_the_contract_is_violated() {
    let rendered = render_reports(&[report("loose", "no sections here")]);
    assert!(rendered.contains("contract warning"), "{rendered}");
    assert!(rendered.contains("no sections here"), "{rendered}");
}

#[test]
fn prompt_is_self_contained_and_names_the_task() {
    let task = ExplorerTask {
        id: "webview-paste".into(),
        prompt: "Where is the paste event handled?".into(),
    };
    let prompt = build_explorer_prompt(&task, Some(std::path::Path::new("/repo")));
    assert!(prompt.contains("webview-paste"), "{prompt}");
    assert!(
        prompt.contains("Where is the paste event handled?"),
        "{prompt}"
    );
    assert!(prompt.contains("/repo"), "{prompt}");
}

struct SlowExplorer {
    delay: std::time::Duration,
}

#[async_trait::async_trait]
impl crate::core::plan_runtime::ExplorerDispatcher for SlowExplorer {
    async fn dispatch(&self, task: &ExplorerTask) -> ExplorerReport {
        tokio::time::sleep(self.delay).await;
        report(&task.id, GOOD_REPORT)
    }
}

#[tokio::test]
async fn dispatch_explorers_runs_tasks_in_parallel_and_keeps_input_order() {
    let rt = crate::core::plan_runtime::PlanRuntime::new("sess-explorer");
    rt.attach_explorer(std::sync::Arc::new(SlowExplorer {
        delay: std::time::Duration::from_millis(120),
    }));
    let tasks: Vec<ExplorerTask> = ["a", "b", "c"]
        .iter()
        .map(|id| ExplorerTask {
            id: (*id).to_string(),
            prompt: format!("investigate {id}"),
        })
        .collect();

    let started = std::time::Instant::now();
    let reports = rt.dispatch_explorers(&tasks).await.expect("dispatch 成功");
    let elapsed = started.elapsed();

    assert_eq!(
        reports.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec!["a", "b", "c"],
        "报告顺序必须与入参一致，否则主 Agent 对不回问题"
    );
    // 串行要 360ms；给足余量后仍应远小于串行耗时。
    assert!(
        elapsed < std::time::Duration::from_millis(300),
        "三个任务应并行执行，实际耗时 {elapsed:?}"
    );
}

#[tokio::test]
async fn dispatch_explorers_without_a_dispatcher_is_an_explicit_error() {
    let rt = crate::core::plan_runtime::PlanRuntime::new("sess-explorer-none");
    let err = rt
        .dispatch_explorers(&[ExplorerTask {
            id: "a".into(),
            prompt: "q".into(),
        }])
        .await
        .expect_err("未注入 dispatcher 时不得静默返回空结论");
    assert!(
        err.to_string().contains(
            "dispatch_agent is unavailable: explorer sub-Agent dispatcher was not supplied"
        ),
        "err={err}"
    );
}
