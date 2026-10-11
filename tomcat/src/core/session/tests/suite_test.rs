//! Real process interleavings for session sidecar publication and deletion.
use std::path::{Path, PathBuf};
use std::process::{Child, Command};
use std::time::{Duration, Instant};

use crate::core::agent_loop::SubagentType;
use crate::core::session::manager::{CompactionResult, MessageAppendSink, SessionManager};
use crate::core::session::preheat_cache::{preheat_cache_path, write_preheat_cache};
use crate::core::session::resume_index::{rebuild_resume_index, resume_index_path};
use crate::core::session::subagent_transcript::JsonlFileAppendSink;
use crate::core::session::tool_display_sidecar::{append_tool_display, tool_display_sidecar_path};
use crate::core::session::user_message_sidecar::{
    ensure_user_message_sidecar, user_message_sidecar_path,
};
use crate::infra::config::lock::acquire_config_lock;
use crate::infra::platform::write_file_atomic;
use crate::AppError;

struct Worker(Child);
impl Drop for Worker {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
impl Worker {
    fn finish(&mut self) {
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(status) = self.0.try_wait().unwrap() {
                assert!(status.success(), "session I/O worker failed: {status}");
                return;
            }
            assert!(Instant::now() < deadline, "session I/O worker timed out");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn spawn(root: &Path, sid: &str, mode: &str, late: bool) -> Worker {
    Worker(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "core::session::tests::suite_test::session_io_worker",
                "--ignored",
                "--nocapture",
            ])
            .env("TOMCAT_TEST_IO_ROOT", root)
            .env("TOMCAT_TEST_IO_SID", sid)
            .env("TOMCAT_TEST_IO_MODE", mode)
            .env("TOMCAT_TEST_IO_LATE", if late { "1" } else { "0" })
            .spawn()
            .unwrap(),
    )
}

fn wait_until(worker: &mut Worker, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(
            worker.0.try_wait().unwrap().is_none(),
            "worker exited before the barrier"
        );
        assert!(
            Instant::now() < deadline,
            "worker failed to reach the barrier"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn artifact(transcript: &Path, mode: &str) -> PathBuf {
    match mode {
        "display" => tool_display_sidecar_path(transcript),
        "user" => user_message_sidecar_path(transcript),
        "preheat" => preheat_cache_path(transcript),
        "resume" => resume_index_path(transcript),
        "subagent" => transcript.to_path_buf(),
        _ => panic!("unknown I/O fixture mode {mode}"),
    }
}

fn write_artifact(transcript: &Path, mode: &str, value: &str) -> Result<(), AppError> {
    match mode {
        "display" => append_tool_display(
            transcript,
            value,
            &chrono::Utc::now().to_rfc3339(),
            &serde_json::from_value(serde_json::json!({
                "kind": "file", "file": "src/lib.rs", "added": 1, "removed": 0,
                "diff": [{"tag": "add", "newLine": 1, "text": value}]
            }))?,
        ),
        "user" => ensure_user_message_sidecar(transcript).map(|_| ()),
        "resume" => rebuild_resume_index(transcript).map(|_| ()),
        "preheat" => write_preheat_cache(
            transcript,
            &CompactionResult {
                summary_text: value.into(),
                covered_start_id: "u1".into(),
                covered_end_id: "a1".into(),
                covered_count: 2,
                transcript_compaction_entry_id: Some("marker".into()),
                estimated_covered_tokens_before: None,
                estimated_summary_tokens: None,
                estimated_tokens_saved: None,
                preheat_elapsed_ms: 0,
            },
        ),
        "subagent" => JsonlFileAppendSink::new(
            transcript.to_path_buf(),
            "child",
            SubagentType::Verifier,
            "fixture",
            "parent",
        )
        .append_custom_entry(serde_json::json!({"event": value})),
        _ => panic!("unknown I/O fixture mode {mode}"),
    }
}

#[test]
#[ignore = "subprocess fixture; invoked by the interleaving tests with an isolated TempDir"]
fn session_io_worker() {
    let root = PathBuf::from(std::env::var_os("TOMCAT_TEST_IO_ROOT").expect("fixture root"));
    let sid = std::env::var("TOMCAT_TEST_IO_SID").expect("fixture session id");
    let mode = std::env::var("TOMCAT_TEST_IO_MODE").expect("fixture mode");
    let late = std::env::var("TOMCAT_TEST_IO_LATE").unwrap() == "1";
    let manager = SessionManager::new(root.clone());
    let transcript = manager.transcript_path(&sid);
    std::fs::write(
        root.join(if late { "late-ready" } else { "ready" }),
        b"ready",
    )
    .unwrap();
    if mode == "delete" {
        let outcome = manager.delete_session(&sid).unwrap();
        assert!(outcome.warnings.is_empty(), "{:?}", outcome.warnings);
    } else {
        let result = write_artifact(&transcript, &mode, "fresh");
        if late {
            if mode == "preheat" {
                result.unwrap();
            } else {
                assert!(
                    matches!(result, Err(AppError::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound),
                    "{result:?}"
                );
            }
            assert!(
                !artifact(&transcript, &mode).exists(),
                "late writer recreated deleted bytes"
            );
        } else {
            result.unwrap();
        }
    }
    std::fs::write(root.join(if late { "late-done" } else { "done" }), b"done").unwrap();
}

#[test]
fn independent_process_sidecar_writers_survive_atomic_replacement() {
    for mode in ["display", "user", "preheat", "resume", "subagent"] {
        let temp = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(temp.path().into());
        let entry = manager.new_current_session(None).unwrap();
        let transcript = manager.transcript_path(&entry.session_id);
        manager
            .append_message(serde_json::json!({"role": "user", "content": "old"}))
            .unwrap();
        write_artifact(&transcript, mode, "old").unwrap();
        let target = artifact(&transcript, mode);
        let old_artifact = std::fs::read(&target).unwrap();
        let fresh_id = manager
            .append_message(serde_json::json!({"role": "user", "content": "source-updated"}))
            .unwrap();
        let guard = acquire_config_lock(&target).unwrap();
        if mode != "subagent" {
            // Seed a stale derived cache, never roll back the authoritative transcript.
            write_file_atomic(&target, &old_artifact).unwrap();
        }
        let snapshot = std::fs::read(&target).unwrap();
        let mut worker = spawn(temp.path(), &entry.session_id, mode, false);
        wait_until(&mut worker, || temp.path().join("ready").exists());
        let until = Instant::now() + Duration::from_millis(300);
        while !temp.path().join("done").exists() && Instant::now() < until {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            !temp.path().join("done").exists(),
            "{mode} did not join its publication lock"
        );
        write_file_atomic(&target, &snapshot).unwrap();
        drop(guard);
        worker.finish();
        let body = std::fs::read_to_string(target).unwrap();
        let expected = match mode {
            "resume" => fresh_id.as_str(),
            "user" => "source-updated",
            _ => "fresh",
        };
        assert!(
            body.contains(expected),
            "{mode}: acknowledged work was lost: {body}"
        );
    }
}

#[test]
fn independent_process_deletion_waits_for_sidecars_and_rejects_late_publication() {
    for mode in ["display", "user", "preheat", "resume"] {
        let temp = tempfile::tempdir().unwrap();
        let manager = SessionManager::new(temp.path().into());
        let entry = manager.new_current_session(None).unwrap();
        let transcript = manager.transcript_path(&entry.session_id);
        manager
            .append_message(serde_json::json!({"role": "user", "content": "old"}))
            .unwrap();
        write_artifact(&transcript, mode, "old").unwrap();
        let target = artifact(&transcript, mode);
        let guard = acquire_config_lock(&target).unwrap();
        let snapshot = std::fs::read(&target).unwrap();
        let mut deleter = spawn(temp.path(), &entry.session_id, "delete", false);
        wait_until(&mut deleter, || !transcript.exists());
        assert!(
            !temp.path().join("done").exists(),
            "deletion did not wait for {mode}"
        );
        // An already-started publication can finish after the parent unlink, but deletion
        // must wait for it and then remove the result before acknowledging completion.
        write_file_atomic(&target, &snapshot).unwrap();
        drop(guard);
        deleter.finish();
        assert!(!target.exists(), "{mode} survived acknowledged deletion");
        assert!(manager
            .get_session_by_id(&entry.session_id)
            .unwrap()
            .is_none());
        let mut late = spawn(temp.path(), &entry.session_id, mode, true);
        late.finish();
        assert!(!transcript.exists());
        assert!(!target.exists());
    }
}
