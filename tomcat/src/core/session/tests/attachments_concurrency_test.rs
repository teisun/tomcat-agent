//! Cross-process retention/GC interleavings; no in-process mutex can satisfy these.
use super::{age_blob, minimal_png, setup, AttachmentBlobStore, Duration, ORPHAN_BLOB_GRACE};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::Instant;

struct RetentionChild(Child);

impl RetentionChild {
    fn spawn(root: &Path, sha: &str, mode: &str) -> Self {
        Self(
            Command::new(std::env::current_exe().unwrap())
                .args([
                    "--exact",
                    "core::session::attachments::tests::concurrency::retention_subprocess",
                    "--ignored",
                    "--nocapture",
                    "--test-threads=1",
                ])
                .env("TOMCAT_RETENTION_TEST_ROOT", root)
                .env("TOMCAT_RETENTION_TEST_SHA", sha)
                .env("TOMCAT_RETENTION_TEST_MODE", mode)
                .stdout(Stdio::null())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        )
    }

    fn finish(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "retention child failed: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "retention child did not finish");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for RetentionChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn wait_for_marker(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "missing child marker: {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
#[ignore = "subprocess fixture, invoked with isolated paths by the retention tests"]
fn retention_subprocess() {
    let root = PathBuf::from(std::env::var_os("TOMCAT_RETENTION_TEST_ROOT").expect("child root"));
    let sha = std::env::var("TOMCAT_RETENTION_TEST_SHA").expect("child sha");
    let mode = std::env::var("TOMCAT_RETENTION_TEST_MODE").expect("child mode");
    let store = AttachmentBlobStore::new(&root);
    match mode.as_str() {
        "retain-first" => {
            store
                .retain_pending_batch_inner("child", vec![sha.clone()], |_, _| {
                    // This is the real production check -> marker boundary, with the lock held.
                    std::fs::write(root.join("ready"), [])?;
                    wait_for_marker(&root.join("release"));
                    Ok(())
                })
                .unwrap();
            assert_eq!(store.list_pending("child").unwrap(), vec![sha]);
        }
        "gc-first" => {
            std::fs::write(root.join("ready"), []).unwrap();
            let result = store.retain_pending_batch("child", vec![sha.clone()]);
            std::fs::write(root.join("finished"), []).unwrap();
            assert!(
                matches!(result, Err(crate::AppError::Config(_))),
                "GC won: retain must report the missing blob, not acknowledge a dangling lease"
            );
            assert!(store.list_pending("child").unwrap().is_empty());
        }
        "put" | "get-corrupt" | "mark" | "touch" | "promote" | "clear" | "expire" => {
            std::fs::write(root.join("ready"), []).unwrap();
            match mode.as_str() {
                "put" => assert_eq!(store.put(&minimal_png()).unwrap(), sha),
                "get-corrupt" => assert_eq!(store.get(&sha).unwrap(), None),
                "mark" => assert!(matches!(
                    store.mark_pending("child", &sha),
                    Err(crate::AppError::Config(_))
                )),
                "touch" => store.touch_pending("child", &sha).unwrap(),
                "promote" => store.promote("child", &sha).unwrap(),
                "clear" => {
                    store.clear_session("child").unwrap();
                }
                "expire" => {
                    store.gc_pending(super::PENDING_BLOB_TTL).unwrap();
                }
                _ => unreachable!(),
            }
            std::fs::write(root.join("finished"), []).unwrap();
        }
        _ => panic!("unknown child mode"),
    }
}

#[test]
fn sweep_waits_for_an_independent_retainer_and_keeps_its_blob() {
    let (tmp, store) = setup();
    let bytes = minimal_png();
    let sha = store.put(&bytes).unwrap();
    age_blob(&store, &sha, ORPHAN_BLOB_GRACE + Duration::from_secs(60));
    let mut child = RetentionChild::spawn(tmp.path(), &sha, "retain-first");
    wait_for_marker(&tmp.path().join("ready"));

    std::thread::scope(|scope| {
        let (started_tx, started_rx) = std::sync::mpsc::channel();
        let (collected_tx, collected_rx) = std::sync::mpsc::channel();
        let store = &store;
        let worker = scope.spawn(move || {
            started_tx.send(()).unwrap();
            store.sweep_orphan_blobs(
                || {
                    collected_tx.send(()).unwrap();
                    Ok(HashSet::new())
                },
                ORPHAN_BLOB_GRACE,
            )
        });
        started_rx.recv_timeout(Duration::from_secs(10)).unwrap();
        let premature = collected_rx.recv_timeout(Duration::from_millis(100));
        // Always release the child before asserting, including on a failed exclusion check.
        std::fs::write(tmp.path().join("release"), []).unwrap();
        let report = worker.join().unwrap().unwrap();
        child.finish();
        assert!(
            matches!(premature, Err(std::sync::mpsc::RecvTimeoutError::Timeout)),
            "GC must not collect references while another process is publishing a lease"
        );
        assert_eq!(report.blobs_deleted, 0);
    });
    assert_eq!(store.get(&sha).unwrap(), Some(bytes));
}

#[test]
fn other_retention_participants_cannot_mutate_during_gc() {
    for mode in [
        "put",
        "get-corrupt",
        "mark",
        "touch",
        "promote",
        "clear",
        "expire",
    ] {
        let (tmp, store) = setup();
        let sha = store.put(&minimal_png()).unwrap();
        if mode == "get-corrupt" {
            std::fs::write(store.blobs_dir().join(&sha), b"corrupt bytes").unwrap();
        }
        age_blob(&store, &sha, ORPHAN_BLOB_GRACE + Duration::from_secs(60));
        let mut child = None;
        let report = store
            .sweep_orphan_blobs(
                || {
                    child = Some(RetentionChild::spawn(tmp.path(), &sha, mode));
                    wait_for_marker(&tmp.path().join("ready"));
                    std::thread::sleep(Duration::from_millis(100));
                    assert!(
                        !tmp.path().join("finished").exists(),
                        "{mode} bypassed the GC guard"
                    );
                    Ok(HashSet::new())
                },
                ORPHAN_BLOB_GRACE,
            )
            .unwrap();
        child.as_mut().unwrap().finish();
        assert_eq!(
            report.blobs_deleted, 1,
            "{mode} changed the blob before sweep finished"
        );
        assert!(store.list_pending("child").unwrap().is_empty());
        if mode == "put" {
            assert_eq!(
                store.get(&sha).unwrap(),
                Some(minimal_png()),
                "a later put recreates valid bytes after GC, never during deletion"
            );
        } else {
            assert_eq!(store.get(&sha).unwrap(), None);
        }
    }
}

#[test]
fn independent_retainer_waits_for_gc_and_rejects_a_deleted_blob() {
    let (tmp, store) = setup();
    let sha = store.put(&minimal_png()).unwrap();
    age_blob(&store, &sha, ORPHAN_BLOB_GRACE + Duration::from_secs(60));
    let mut child = None;
    let report = store
        .sweep_orphan_blobs(
            || {
                child = Some(RetentionChild::spawn(tmp.path(), &sha, "gc-first"));
                wait_for_marker(&tmp.path().join("ready"));
                std::thread::sleep(Duration::from_millis(100));
                assert!(
                    !tmp.path().join("finished").exists(),
                    "retain must not publish a lease between the GC mark and deletion"
                );
                Ok(HashSet::new())
            },
            ORPHAN_BLOB_GRACE,
        )
        .unwrap();
    child.as_mut().unwrap().finish();
    assert_eq!(report.blobs_deleted, 1);
    assert_eq!(store.get(&sha).unwrap(), None);
    assert!(store.list_pending("child").unwrap().is_empty());
}
