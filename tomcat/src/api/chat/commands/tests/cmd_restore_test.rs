use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::json;
use serial_test::serial;

use crate::api::chat::commands::cmd_restore::{
    effective_restore_paths, other_session_restore_conflicts, restore_core, run as run_restore,
};
use crate::api::chat::ChatContext;
use crate::core::session::transcript::TranscriptEntry;
use crate::{
    AppConfig, CheckpointDiff, CheckpointError, CheckpointId, CheckpointKind, CheckpointMeta,
    CheckpointRecordRequest, CheckpointRestoreReport, CheckpointStore, ListOptions, RestoreOptions,
    RetentionPolicy,
};

struct EnvGuard {
    key: &'static str,
    prev: Option<OsString>,
}

impl EnvGuard {
    fn set(key: &'static str, value: impl Into<OsString>) -> Self {
        let prev = std::env::var_os(key);
        // SAFETY: test-scoped env mutation guarded by serial + home_env_lock.
        unsafe { std::env::set_var(key, value.into()) };
        Self { key, prev }
    }
}

impl Drop for EnvGuard {
    fn drop(&mut self) {
        match self.prev.take() {
            Some(prev) => {
                // SAFETY: restore original env during test teardown.
                unsafe { std::env::set_var(self.key, prev) };
            }
            None => {
                // SAFETY: clear test-only env during teardown.
                unsafe { std::env::remove_var(self.key) };
            }
        }
    }
}

struct CurrentDirGuard {
    _lock: crate::test_support::TestLockGuard<'static>,
    previous: PathBuf,
}

impl CurrentDirGuard {
    fn set(path: &Path) -> Self {
        let lock = crate::test_support::cwd_lock().lock().unwrap();
        let previous = std::env::current_dir().expect("current_dir");
        std::env::set_current_dir(path).expect("set_current_dir");
        Self {
            _lock: lock,
            previous,
        }
    }
}

impl Drop for CurrentDirGuard {
    fn drop(&mut self) {
        let _ = std::env::set_current_dir(&self.previous);
    }
}

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

enum DiffSpyResult {
    Paths(Vec<PathBuf>),
    Empty,
}

struct DiffSpyStore {
    diff: DiffSpyResult,
}

impl CheckpointStore for DiffSpyStore {
    fn record(&self, _request: CheckpointRecordRequest) -> Result<CheckpointId, CheckpointError> {
        Ok(CheckpointId::null())
    }

    fn list(
        &self,
        _session_id: &str,
        _opts: ListOptions,
    ) -> Result<Vec<CheckpointMeta>, CheckpointError> {
        Ok(Vec::new())
    }

    fn show(&self, _id: &CheckpointId) -> Result<Option<CheckpointMeta>, CheckpointError> {
        Ok(None)
    }

    fn diff(&self, _id: &CheckpointId) -> Result<CheckpointDiff, CheckpointError> {
        match &self.diff {
            DiffSpyResult::Paths(paths) => Ok(CheckpointDiff {
                text: String::new(),
                changed_paths: paths.clone(),
            }),
            DiffSpyResult::Empty => Ok(CheckpointDiff::default()),
        }
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

#[test]
#[serial(env_lock)]
fn effective_restore_paths_defaults_to_current_session_changed_paths() {
    if !git_available() {
        return;
    }

    const API_ENV: &str = "TOMCAT_CMD_RESTORE_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");
    let session_id = ctx
        .session_runtime
        .session
        .current_session_id()
        .expect("current_session_id")
        .expect("session id");

    std::fs::write(workspace.path().join("a.txt"), "v1-a").unwrap();
    let checkpoint_a = ctx
        .scope_services
        .checkpoint_store
        .record(CheckpointRecordRequest {
            session_id: session_id.clone(),
            turn_id: "turn-1".to_string(),
            kind: CheckpointKind::Manual {
                label: "first".to_string(),
            },
            message_anchor: None,
            notes: None,
        })
        .expect("record checkpoint a");

    std::fs::write(workspace.path().join("b.txt"), "v1-b").unwrap();
    let _checkpoint_b = ctx
        .scope_services
        .checkpoint_store
        .record(CheckpointRecordRequest {
            session_id: session_id.clone(),
            turn_id: "turn-2".to_string(),
            kind: CheckpointKind::Manual {
                label: "second".to_string(),
            },
            message_anchor: None,
            notes: None,
        })
        .expect("record checkpoint b");

    let meta_a = ctx
        .scope_services
        .checkpoint_store
        .show(&checkpoint_a)
        .expect("show checkpoint a")
        .expect("checkpoint a meta");
    let changed_paths = meta_a
        .notes
        .as_ref()
        .and_then(|notes| notes.get("changedPaths"))
        .and_then(serde_json::Value::as_array)
        .expect("changedPaths in notes");
    assert_eq!(changed_paths.len(), 1);
    assert_eq!(changed_paths[0].as_str(), Some("a.txt"));

    let narrowed = effective_restore_paths(&ctx, &checkpoint_a, &meta_a, &[]);
    assert_eq!(narrowed.paths, vec![PathBuf::from("a.txt")]);
    assert_eq!(narrowed.warning, None);

    let explicit =
        effective_restore_paths(&ctx, &checkpoint_a, &meta_a, &[PathBuf::from("manual.txt")]);
    assert_eq!(explicit.paths, vec![PathBuf::from("manual.txt")]);
    assert_eq!(explicit.warning, None);
}

#[test]
#[serial(env_lock)]
fn effective_restore_paths_falls_back_to_checkpoint_diff() {
    const API_ENV: &str = "TOMCAT_CMD_RESTORE_DIFF_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    let mut ctx = ChatContext::from_config(cfg).expect("chat context should be created");
    ctx.scope_services.checkpoint_store = Arc::new(DiffSpyStore {
        diff: DiffSpyResult::Paths(vec![PathBuf::from("fallback.txt")]),
    });
    let checkpoint_id = CheckpointId::new("ck_fallback");
    let meta = CheckpointMeta {
        id: checkpoint_id.clone(),
        session_id: "session-1".to_string(),
        turn_id: "turn-1".to_string(),
        kind: CheckpointKind::Manual {
            label: "manual".to_string(),
        },
        git_commit: Some("deadbeef".to_string()),
        message_anchor: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        notes: None,
    };

    let narrowed = effective_restore_paths(&ctx, &checkpoint_id, &meta, &[]);
    assert_eq!(narrowed.paths, vec![PathBuf::from("fallback.txt")]);
    assert_eq!(narrowed.warning, None);
}

#[test]
#[serial(env_lock)]
fn effective_restore_paths_warns_when_auto_narrowing_fails() {
    const API_ENV: &str = "TOMCAT_CMD_RESTORE_WARN_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    let mut ctx = ChatContext::from_config(cfg).expect("chat context should be created");
    ctx.scope_services.checkpoint_store = Arc::new(DiffSpyStore {
        diff: DiffSpyResult::Empty,
    });
    let checkpoint_id = CheckpointId::new("ck_warn");
    let meta = CheckpointMeta {
        id: checkpoint_id.clone(),
        session_id: "session-1".to_string(),
        turn_id: "turn-1".to_string(),
        kind: CheckpointKind::Manual {
            label: "manual".to_string(),
        },
        git_commit: Some("deadbeef".to_string()),
        message_anchor: None,
        created_at: chrono::Utc::now().to_rfc3339(),
        notes: None,
    };

    let narrowed = effective_restore_paths(&ctx, &checkpoint_id, &meta, &[]);
    assert!(
        narrowed.paths.is_empty(),
        "should continue with full-tree restore"
    );
    assert!(
        narrowed.warning.is_some(),
        "auto narrowing failure should only warn"
    );
}

#[test]
#[serial(env_lock)]
fn other_session_restore_conflicts_detect_overlapping_paths() {
    if !git_available() {
        return;
    }

    const API_ENV: &str = "TOMCAT_CMD_RESTORE_CONFLICT_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");

    let first_session_id = ctx
        .session_runtime
        .session
        .current_session_id()
        .expect("current_session_id")
        .expect("first session id");
    std::fs::write(workspace.path().join("shared.txt"), "v1").unwrap();
    let _checkpoint_a = ctx
        .scope_services
        .checkpoint_store
        .record(CheckpointRecordRequest {
            session_id: first_session_id.clone(),
            turn_id: "turn-a".to_string(),
            kind: CheckpointKind::Manual {
                label: "first".to_string(),
            },
            message_anchor: None,
            notes: None,
        })
        .expect("record checkpoint for first session");

    let second = ctx
        .session_runtime
        .session
        .new_current_session(Some(workspace.path().to_string_lossy().to_string()))
        .expect("second session");
    std::fs::write(workspace.path().join("shared.txt"), "v2").unwrap();
    let _checkpoint_b = ctx
        .scope_services
        .checkpoint_store
        .record(CheckpointRecordRequest {
            session_id: second.session_id.clone(),
            turn_id: "turn-b".to_string(),
            kind: CheckpointKind::Manual {
                label: "second".to_string(),
            },
            message_anchor: None,
            notes: None,
        })
        .expect("record checkpoint for second session");

    let conflicts = other_session_restore_conflicts(
        &ctx,
        Some(first_session_id.as_str()),
        &[PathBuf::from("shared.txt")],
    );
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].session_id, second.session_id);
    assert_eq!(conflicts[0].paths, vec![PathBuf::from("shared.txt")]);
}

#[test]
#[serial(env_lock)]
fn other_session_restore_conflicts_include_other_scopes() {
    if !git_available() {
        return;
    }

    const API_ENV: &str = "TOMCAT_CMD_RESTORE_SCOPE_CONFLICT_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");

    let current_session_id = ctx
        .session_runtime
        .session
        .current_session_id()
        .expect("current_session_id")
        .expect("current session id");
    std::fs::write(workspace.path().join("shared.txt"), "v1").unwrap();
    ctx.scope_services
        .checkpoint_store
        .record(CheckpointRecordRequest {
            session_id: current_session_id.clone(),
            turn_id: "turn-current".to_string(),
            kind: CheckpointKind::Manual {
                label: "current".to_string(),
            },
            message_anchor: None,
            notes: None,
        })
        .expect("record checkpoint for current scope");

    let current_key = ctx
        .session_runtime
        .session
        .current_session_key()
        .to_string();
    let claw_key = crate::session_key_for(crate::SessionMode::Claw, workspace.path());
    let code_key = crate::session_key_for(crate::SessionMode::Code, workspace.path());
    let other_key = if current_key == claw_key {
        code_key
    } else {
        claw_key
    };
    let other_manager = crate::SessionManager::new_scoped(
        ctx.session_runtime.session.sessions_dir().to_path_buf(),
        other_key,
    );
    let other_session = other_manager
        .new_current_session(Some(workspace.path().to_string_lossy().to_string()))
        .expect("other scope session");
    std::fs::write(workspace.path().join("shared.txt"), "v2").unwrap();
    ctx.scope_services
        .checkpoint_store
        .record(CheckpointRecordRequest {
            session_id: other_session.session_id.clone(),
            turn_id: "turn-other-scope".to_string(),
            kind: CheckpointKind::Manual {
                label: "other-scope".to_string(),
            },
            message_anchor: None,
            notes: None,
        })
        .expect("record checkpoint for other scope");

    let conflicts = other_session_restore_conflicts(
        &ctx,
        Some(current_session_id.as_str()),
        &[PathBuf::from("shared.txt")],
    );
    assert_eq!(conflicts.len(), 1);
    assert_eq!(conflicts[0].session_id, other_session.session_id);
    assert_eq!(conflicts[0].paths, vec![PathBuf::from("shared.txt")]);
}

#[test]
#[serial(env_lock)]
fn restore_keeps_other_session_owned_paths_untouched() {
    if !git_available() {
        return;
    }

    const API_ENV: &str = "TOMCAT_CMD_RESTORE_ISOLATION_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");

    let session_a = ctx
        .session_runtime
        .session
        .current_session_id()
        .expect("current session id")
        .expect("session a");
    std::fs::write(workspace.path().join("a.txt"), "a1").unwrap();
    let checkpoint_a = ctx
        .scope_services
        .checkpoint_store
        .record(CheckpointRecordRequest {
            session_id: session_a.clone(),
            turn_id: "turn-a1".to_string(),
            kind: CheckpointKind::Manual {
                label: "session-a".to_string(),
            },
            message_anchor: None,
            notes: None,
        })
        .expect("checkpoint a");

    let session_b = ctx
        .session_runtime
        .session
        .new_current_session(Some(workspace.path().to_string_lossy().to_string()))
        .expect("session b")
        .session_id;
    std::fs::write(workspace.path().join("b.txt"), "b1").unwrap();
    ctx.scope_services
        .checkpoint_store
        .record(CheckpointRecordRequest {
            session_id: session_b.clone(),
            turn_id: "turn-b1".to_string(),
            kind: CheckpointKind::Manual {
                label: "session-b".to_string(),
            },
            message_anchor: None,
            notes: None,
        })
        .expect("checkpoint b");

    std::fs::write(workspace.path().join("a.txt"), "a2").unwrap();
    std::fs::write(workspace.path().join("b.txt"), "b2").unwrap();

    ctx.session_runtime
        .session
        .switch_current_to_session_id(&session_a)
        .expect("switch back to session a");
    let outcome = run_restore(&ctx, checkpoint_a.to_string(), Vec::new(), false);
    assert!(matches!(
        outcome,
        crate::api::chat::commands::parse::ChatCommandOutcome::Handled
    ));

    assert_eq!(
        std::fs::read_to_string(workspace.path().join("a.txt")).unwrap(),
        "a1",
        "restore 应只回滚当前会话记录过的 a.txt"
    );
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("b.txt")).unwrap(),
        "b2",
        "restore 不应误伤其他会话独占修改的 b.txt"
    );
}

#[test]
#[serial(home_env_lock)]
fn restore_core_without_reverting_files_truncates_transcript_only() {
    if !git_available() {
        return;
    }

    const API_ENV: &str = "TOMCAT_CMD_RESTORE_CORE_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");
    let session = ctx
        .session_runtime
        .session
        .current_session_id()
        .expect("current session id")
        .expect("session id");

    let _user_before = ctx
        .session_runtime
        .session
        .try_append_message_to_session(
            &session,
            json!({
                "role": "user",
                "content": "before checkpoint"
            }),
        )
        .expect("append user before checkpoint");
    let assistant_anchor = ctx
        .session_runtime
        .session
        .try_append_message_to_session(
            &session,
            json!({
                "role": "assistant",
                "content": "anchor reply"
            }),
        )
        .expect("append assistant anchor");

    std::fs::write(workspace.path().join("keep.txt"), "current-worktree").unwrap();
    let checkpoint_id = ctx
        .scope_services
        .checkpoint_store
        .record(CheckpointRecordRequest {
            session_id: session.clone(),
            turn_id: "turn-restore-core".to_string(),
            kind: CheckpointKind::TurnEnd,
            message_anchor: Some(assistant_anchor.clone()),
            notes: Some(json!({
                "changedPaths": ["keep.txt"]
            })),
        })
        .expect("checkpoint");

    let superseded_user_id = ctx
        .session_runtime
        .session
        .try_append_message_to_session(
            &session,
            json!({
                "role": "user",
                "content": "prompt after checkpoint"
            }),
        )
        .expect("append superseded user");
    let superseded_assistant_id = ctx
        .session_runtime
        .session
        .try_append_message_to_session(
            &session,
            json!({
                "role": "assistant",
                "content": "answer after checkpoint"
            }),
        )
        .expect("append superseded assistant");

    let report = restore_core(&ctx, checkpoint_id.clone(), false, false).expect("restore core");

    assert_eq!(report.changed_paths, vec!["keep.txt".to_string()]);
    assert!(!report.revert_files);
    assert!(report.transcript_truncated);
    assert_eq!(
        std::fs::read_to_string(workspace.path().join("keep.txt")).unwrap(),
        "current-worktree",
        "transcript-only restore 不应改动工作区文件"
    );

    let entries = ctx
        .session_runtime
        .session
        .get_entries_for_session(&session, 64)
        .expect("session entries");

    let superseded_ids = entries
        .iter()
        .filter_map(|entry| match entry {
            TranscriptEntry::Message(message)
                if message
                    .message
                    .get("superseded")
                    .and_then(serde_json::Value::as_bool)
                    == Some(true) =>
            {
                message.id.clone()
            }
            _ => None,
        })
        .collect::<Vec<_>>();
    assert!(superseded_ids.contains(&superseded_user_id));
    assert!(superseded_ids.contains(&superseded_assistant_id));
    assert!(entries.iter().any(|entry| matches!(
        entry,
        TranscriptEntry::Custom(custom)
            if custom.extra.get("customType").and_then(serde_json::Value::as_str)
                == Some("checkpoint.restore")
    )));
}

#[test]
#[serial(home_env_lock)]
fn restore_core_without_reverting_files_skips_cross_session_conflict_warnings() {
    if !git_available() {
        return;
    }

    const API_ENV: &str = "TOMCAT_CMD_RESTORE_DONT_REVERT_WARNINGS_TEST_KEY";

    let _home_lock = crate::test_support::home_env_lock().lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let workspace = tempfile::tempdir().unwrap();
    let work_dir = tempfile::tempdir().unwrap();
    let _home_guard = EnvGuard::set("HOME", home.path().as_os_str().to_os_string());
    let _api_guard = EnvGuard::set(API_ENV, "stub");
    let _cwd_guard = CurrentDirGuard::set(workspace.path());

    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(work_dir.path().to_string_lossy().to_string());
    cfg.llm.api_key_env = Some(API_ENV.to_string());
    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");
    let session_a = ctx
        .session_runtime
        .session
        .current_session_id()
        .expect("current session id")
        .expect("session a");

    let _user_before = ctx
        .session_runtime
        .session
        .try_append_message_to_session(
            &session_a,
            json!({
                "role": "user",
                "content": "before checkpoint"
            }),
        )
        .expect("append user before checkpoint");
    let assistant_anchor = ctx
        .session_runtime
        .session
        .try_append_message_to_session(
            &session_a,
            json!({
                "role": "assistant",
                "content": "anchor reply"
            }),
        )
        .expect("append assistant anchor");

    std::fs::write(workspace.path().join("shared.txt"), "current-worktree").unwrap();
    let checkpoint_id = ctx
        .scope_services
        .checkpoint_store
        .record(CheckpointRecordRequest {
            session_id: session_a.clone(),
            turn_id: "turn-restore-core".to_string(),
            kind: CheckpointKind::TurnEnd,
            message_anchor: Some(assistant_anchor),
            notes: Some(json!({
                "changedPaths": ["shared.txt"]
            })),
        })
        .expect("checkpoint");

    ctx.session_runtime
        .session
        .try_append_message_to_session(
            &session_a,
            json!({
                "role": "user",
                "content": "prompt after checkpoint"
            }),
        )
        .expect("append superseded user");

    let session_b = ctx
        .session_runtime
        .session
        .new_current_session(Some(workspace.path().to_string_lossy().to_string()))
        .expect("session b")
        .session_id;
    ctx.scope_services
        .checkpoint_store
        .record(CheckpointRecordRequest {
            session_id: session_b,
            turn_id: "turn-b".to_string(),
            kind: CheckpointKind::Manual {
                label: "other-session".to_string(),
            },
            message_anchor: None,
            notes: Some(json!({
                "changedPaths": ["shared.txt"]
            })),
        })
        .expect("other-session checkpoint");

    ctx.session_runtime
        .session
        .switch_current_to_session_id(&session_a)
        .expect("switch back to session a");

    let report = restore_core(&ctx, checkpoint_id, false, false).expect("restore core");

    assert_eq!(report.changed_paths, vec!["shared.txt".to_string()]);
    assert!(
        report.warnings.is_empty(),
        "transcript-only restore should skip cross-session conflict scanning"
    );
}
