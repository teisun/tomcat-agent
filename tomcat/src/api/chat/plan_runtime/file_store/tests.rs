//! file_store unit tests — §9.3B（P2）。
//!
//! 落点说明：`mod tests;` 在父文件以 `#[path = "file_store/tests.rs"]` 形式
//! 复用即可（与 plan_runtime/tests.rs 同模式）。

use super::*;
use std::sync::Arc;

fn sample_frontmatter() -> PlanFileFrontmatter {
    PlanFileFrontmatter {
        plan_id: "demo_plan_1".to_string(),
        goal: "为 chat 模式补齐 todos 与 /plan 闭环".to_string(),
        mode: PlanFileMode::Planning,
        session_key: None,
        session_id: None,
        created_at: "2026-05-19T10:00:00+08:00".to_string(),
        schema_version: PLAN_FILE_SCHEMA_VERSION,
        milestones: vec![Milestone {
            id: "m1".into(),
            title: "milestone 1".into(),
            todo_ids: vec!["t1".into(), "t2".into()],
        }],
        todos: vec![
            TodoItem {
                id: "t1".into(),
                content: "step 1".into(),
                status: TodoStatus::Pending,
                milestone_id: Some("m1".into()),
            },
            TodoItem {
                id: "t2".into(),
                content: "step 2".into(),
                status: TodoStatus::InProgress,
                milestone_id: Some("m1".into()),
            },
        ],
        unknown: serde_yaml::Mapping::new(),
    }
}

#[test]
fn plan_file_round_trip_frontmatter() {
    let plan = PlanFile {
        frontmatter: sample_frontmatter(),
        body: "## Goal\n\nThis is the goal.\n".to_string(),
    };
    let text = serialize_plan_file(&plan).expect("serialize");
    assert!(text.starts_with("---\n"));
    assert!(text.contains("plan_id: demo_plan_1"));
    assert!(text.contains("schema_version: 1"));
    let parsed = parse_plan_file(&text).expect("parse");
    assert_eq!(parsed.frontmatter.plan_id, "demo_plan_1");
    assert_eq!(parsed.frontmatter.mode, PlanFileMode::Planning);
    assert_eq!(parsed.frontmatter.todos.len(), 2);
    assert_eq!(parsed.frontmatter.todos[1].status, TodoStatus::InProgress);
    assert_eq!(parsed.body.trim(), "## Goal\n\nThis is the goal.".trim());
}

#[test]
fn plan_file_round_trip_preserves_unknown_keys() {
    let mut fm = sample_frontmatter();
    let mut extra = serde_yaml::Mapping::new();
    extra.insert(
        serde_yaml::Value::String("future_field".into()),
        serde_yaml::Value::String("forward-compat".into()),
    );
    fm.unknown = extra;
    let plan = PlanFile { frontmatter: fm, body: String::new() };
    let text = serialize_plan_file(&plan).unwrap();
    assert!(
        text.contains("future_field: forward-compat"),
        "unknown 字段必须 round-trip：\n{text}"
    );
    let parsed = parse_plan_file(&text).unwrap();
    assert_eq!(
        parsed.frontmatter.unknown.get(&serde_yaml::Value::String("future_field".into())),
        Some(&serde_yaml::Value::String("forward-compat".into()))
    );
}

#[test]
fn plan_file_missing_required_field_returns_error() {
    let yaml_missing_plan_id = "---\ngoal: g\nmode: planning\ncreated_at: t\nschema_version: 1\nmilestones: []\ntodos: []\n---\n";
    let err = parse_plan_file(yaml_missing_plan_id)
        .expect_err("缺 plan_id 应失败");
    matches!(err, PlanError::YamlParse(_) | PlanError::MissingField { .. });

    // 空 plan_id 走 runtime 必填校验
    let yaml_empty_plan_id = "---\nplan_id: \"\"\ngoal: g\nmode: planning\ncreated_at: t\nschema_version: 1\nmilestones: []\ntodos: []\n---\n";
    let err = parse_plan_file(yaml_empty_plan_id)
        .expect_err("空 plan_id 应失败");
    match &err {
        PlanError::MissingField { field } => assert_eq!(field, "plan_id"),
        other => panic!("expected MissingField(plan_id), got {other:?}"),
    }

    let yaml_empty_goal = "---\nplan_id: x\ngoal: \"\"\nmode: planning\ncreated_at: t\nschema_version: 1\nmilestones: []\ntodos: []\n---\n";
    let err = parse_plan_file(yaml_empty_goal).expect_err("空 goal 应失败");
    match &err {
        PlanError::MissingField { field } => assert_eq!(field, "goal"),
        other => panic!("expected MissingField(goal), got {other:?}"),
    }
}

#[test]
fn plan_file_schema_version_v1_locked() {
    let yaml = "---\nplan_id: x\ngoal: g\nmode: planning\ncreated_at: t\nschema_version: 2\nmilestones: []\ntodos: []\n---\n";
    let err = parse_plan_file(yaml).expect_err("schema_version=2 应被拒");
    assert!(
        matches!(err, PlanError::SchemaVersion { actual: 2, expected: 1 }),
        "expected SchemaVersion(2,1), got {err:?}"
    );
}

#[test]
fn plan_file_rejects_multiple_in_progress_on_write() {
    let mut fm = sample_frontmatter();
    // 第二个 in_progress
    fm.todos.push(TodoItem {
        id: "t3".into(),
        content: "另一个 in_progress".into(),
        status: TodoStatus::InProgress,
        milestone_id: Some("m1".into()),
    });
    let plan = PlanFile { frontmatter: fm, body: String::new() };
    let err = serialize_plan_file(&plan).expect_err("应拒多个 in_progress");
    assert!(
        matches!(err, PlanError::MultipleInProgress { count: 2 }),
        "got {err:?}"
    );
}

#[test]
fn plan_file_rejects_duplicate_todo_ids_on_write() {
    let mut fm = sample_frontmatter();
    fm.todos.push(TodoItem {
        id: "t1".into(),
        content: "dup".into(),
        status: TodoStatus::Pending,
        milestone_id: None,
    });
    let plan = PlanFile { frontmatter: fm, body: String::new() };
    let err = serialize_plan_file(&plan).expect_err("应拒重复 id");
    match &err {
        PlanError::DuplicateTodoId { id } => assert_eq!(id, "t1"),
        other => panic!("expected DuplicateTodoId, got {other:?}"),
    }
}

#[test]
fn plan_file_frontmatter_delim_missing_returns_error() {
    let err = parse_plan_file("not yaml at all\n").expect_err("无 --- 应失败");
    assert!(
        matches!(err, PlanError::FrontmatterDelimMissing),
        "got {err:?}"
    );

    // 缺结尾 ---
    let err = parse_plan_file("---\nplan_id: x\n").expect_err("缺结尾 --- 应失败");
    assert!(matches!(err, PlanError::FrontmatterDelimMissing), "got {err:?}");
}

#[test]
fn plan_path_for_id_rejects_unsafe() {
    let err = plan_path_for_id("../etc/passwd").expect_err("应拒穿越");
    assert!(matches!(err, PlanError::InvalidPlanId { .. }), "got {err:?}");

    let err = plan_path_for_id("a/b").expect_err("应拒斜杠");
    assert!(matches!(err, PlanError::InvalidPlanId { .. }), "got {err:?}");
}

#[test]
fn plan_file_path_fixed_under_dot_tomcat() {
    let path = plan_path_for_id("safe_id_1").unwrap();
    let canonical = path.to_string_lossy();
    assert!(
        canonical.contains(".tomcat") && canonical.contains("plans") && canonical.ends_with("safe_id_1.plan.md"),
        "plan 文件路径必须位于 ~/.tomcat/plans/，实际：{canonical}"
    );
}

// ─── write_plan / advisory lock ─────────────────────────────────────────────

fn temp_plans_dir() -> std::path::PathBuf {
    let p = std::env::temp_dir().join(format!(
        "tomcat_plan_test_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    std::fs::create_dir_all(&p).unwrap();
    p
}

#[test]
fn write_plan_writes_and_reads_back_atomically() {
    let dir = temp_plans_dir();
    let path = dir.join("demo_plan_1.plan.md");
    let plan = PlanFile {
        frontmatter: sample_frontmatter(),
        body: "## Goal\n\nbody text\n".into(),
    };
    write_plan(&path, &plan, 2000).unwrap();
    let parsed = read_plan(&path).unwrap();
    assert_eq!(parsed.frontmatter.plan_id, "demo_plan_1");
    assert_eq!(parsed.body.trim(), "## Goal\n\nbody text".trim());

    // 没有 .tmp 残留（rename 成功）
    let entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    let leftover_tmp: Vec<_> = entries.iter().filter(|s| s.contains(".tmp.")).collect();
    assert!(
        leftover_tmp.is_empty(),
        "tmp 文件不应残留：{leftover_tmp:?}"
    );
}

#[test]
fn write_plan_overwrites_existing_atomically() {
    let dir = temp_plans_dir();
    let path = dir.join("demo_plan_1.plan.md");
    let mut plan = PlanFile {
        frontmatter: sample_frontmatter(),
        body: "## V1\n".into(),
    };
    write_plan(&path, &plan, 2000).unwrap();

    plan.body = "## V2 overwritten\n".into();
    write_plan(&path, &plan, 2000).unwrap();

    let parsed = read_plan(&path).unwrap();
    assert!(parsed.body.contains("V2 overwritten"));
}

#[test]
fn write_plan_atomic_rename_recovers_when_tmp_leftover_exists() {
    // 预置 stale tmp 文件，模拟上次崩溃；新一轮 write_plan 应能成功
    // （`.<name>.tmp.<seq>` 命名按 seq 隔离，不会撞）。
    let dir = temp_plans_dir();
    let path = dir.join("demo_plan_1.plan.md");
    std::fs::write(
        dir.join(".demo_plan_1.plan.md.tmp.999"),
        "stale junk",
    )
    .unwrap();
    let plan = PlanFile {
        frontmatter: sample_frontmatter(),
        body: "## fresh\n".into(),
    };
    write_plan(&path, &plan, 2000).unwrap();
    let parsed = read_plan(&path).unwrap();
    assert_eq!(parsed.frontmatter.plan_id, "demo_plan_1");
    // stale tmp 仍可能存在（我们故意没清），但 final 文件正确
    assert!(path.exists());
}

#[test]
fn plan_file_lock_timeout_returns_lock_busy() {
    let dir = temp_plans_dir();
    let path = dir.join("demo_plan_1.plan.md");
    let lock_path = path.with_file_name("demo_plan_1.plan.md.lock");
    std::fs::create_dir_all(&dir).unwrap();

    // 预占锁：开后台线程持锁 500ms 后释放
    let lock_path_clone = lock_path.clone();
    let holder = Arc::new(std::sync::Mutex::new(()));
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let started_signal = Arc::clone(&started);
    let handle = std::thread::spawn(move || {
        let f = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path_clone)
            .unwrap();
        f.lock_exclusive().unwrap();
        started_signal.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(600));
        let _ = FileExt::unlock(&f);
        let _ = holder;
    });
    while !started.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(5));
    }
    // 锁被持有；本线程用 100ms timeout 抢锁 → 应 LockBusy
    let plan = PlanFile {
        frontmatter: sample_frontmatter(),
        body: String::new(),
    };
    let start = Instant::now();
    let err = write_plan(&path, &plan, 100).expect_err("应 LockBusy");
    let elapsed = start.elapsed();
    assert!(
        matches!(err, PlanError::LockBusy { waited_ms } if waited_ms >= 80),
        "got {err:?}"
    );
    // 验证 timeout 大致兑现（不大于 500ms，远小于持锁的 600ms）
    assert!(elapsed < Duration::from_millis(500), "elapsed={:?}", elapsed);
    handle.join().unwrap();
}

#[test]
fn plan_file_lock_is_exclusive_serialized_via_lock() {
    // 同一文件并发 write_plan，第二个等到第一个 release 才能成功
    let dir = temp_plans_dir();
    let path = dir.join("demo_plan_1.plan.md");
    let plan1 = PlanFile {
        frontmatter: sample_frontmatter(),
        body: "## A\n".into(),
    };
    let plan2 = {
        let mut fm = sample_frontmatter();
        fm.todos[0].status = TodoStatus::Completed;
        PlanFile { frontmatter: fm, body: "## B\n".into() }
    };

    let path1 = path.clone();
    let path2 = path.clone();
    let t1 = std::thread::spawn(move || write_plan(&path1, &plan1, 5000));
    let t2 = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(10));
        write_plan(&path2, &plan2, 5000)
    });
    t1.join().unwrap().unwrap();
    t2.join().unwrap().unwrap();

    let parsed = read_plan(&path).unwrap();
    // 两次都写入；最终是其中之一（线程调度决定），不应损坏
    assert!(parsed.body.contains("## A") || parsed.body.contains("## B"));
}
