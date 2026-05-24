use super::{sample_frontmatter, temp_plans_dir};
use super::super::file_store::{read_plan, write_plan, PlanError, PlanFile, TodoStatus};
use fs2::FileExt;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

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

    let entries: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().to_string())
        .collect();
    let leftover_tmp: Vec<_> = entries.iter().filter(|name| name.contains(".tmp.")).collect();
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
    let dir = temp_plans_dir();
    let path = dir.join("demo_plan_1.plan.md");
    std::fs::write(dir.join(".demo_plan_1.plan.md.tmp.999"), "stale junk").unwrap();
    let plan = PlanFile {
        frontmatter: sample_frontmatter(),
        body: "## fresh\n".into(),
    };
    write_plan(&path, &plan, 2000).unwrap();
    let parsed = read_plan(&path).unwrap();
    assert_eq!(parsed.frontmatter.plan_id, "demo_plan_1");
    assert!(path.exists());
}

#[test]
fn plan_file_lock_timeout_returns_lock_busy() {
    let dir = temp_plans_dir();
    let path = dir.join("demo_plan_1.plan.md");
    let lock_path = path.with_file_name("demo_plan_1.plan.md.lock");
    std::fs::create_dir_all(&dir).unwrap();

    let lock_path_clone = lock_path.clone();
    let holder = Arc::new(std::sync::Mutex::new(()));
    let started = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let started_signal = Arc::clone(&started);
    let handle = std::thread::spawn(move || {
        let file = std::fs::OpenOptions::new()
            .create(true)
            .read(true)
            .write(true)
            .truncate(false)
            .open(&lock_path_clone)
            .unwrap();
        file.lock_exclusive().unwrap();
        started_signal.store(true, Ordering::Relaxed);
        std::thread::sleep(Duration::from_millis(600));
        let _ = FileExt::unlock(&file);
        let _ = holder;
    });
    while !started.load(Ordering::Relaxed) {
        std::thread::sleep(Duration::from_millis(5));
    }
    let plan = PlanFile {
        frontmatter: sample_frontmatter(),
        body: String::new(),
    };
    let start = Instant::now();
    let err = write_plan(&path, &plan, 100).expect_err("应 LockBusy");
    let elapsed = start.elapsed();
    assert!(
        matches!(err, PlanError::LockBusy { waited_ms, .. } if waited_ms >= 80),
        "got {err:?}"
    );
    assert!(elapsed < Duration::from_millis(500), "elapsed={:?}", elapsed);
    handle.join().unwrap();
}

#[test]
fn plan_file_lock_is_exclusive_serialized_via_lock() {
    let dir = temp_plans_dir();
    let path = dir.join("demo_plan_1.plan.md");
    let plan1 = PlanFile {
        frontmatter: sample_frontmatter(),
        body: "## A\n".into(),
    };
    let plan2 = {
        let mut frontmatter = sample_frontmatter();
        frontmatter.todos[0].status = TodoStatus::Completed;
        PlanFile {
            frontmatter,
            body: "## B\n".into(),
        }
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
    assert!(parsed.body.contains("## A") || parsed.body.contains("## B"));
}
