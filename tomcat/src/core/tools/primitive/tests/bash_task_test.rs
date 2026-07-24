//! `BashTaskRegistry` 后台 spawn / read_output / stop / list 行为单测。
//!
//! 仅触达 `crate::core::tools::primitive::{BashTaskRegistry, BashTaskStatus}`
//! 公共导出；与 [`crate::core::tools::primitive::bash_task`] 模块文档保持一致。

use std::sync::Arc;

use crate::core::permission::{
    BashAstChecker, DefaultPermissionGate, GateConfig, PathRule, PathRuleMode, SessionGrants,
};
use crate::core::tools::contract::confirmation::AllowAllConfirmation;
use crate::core::tools::primitive::bash_task::BackgroundBashGuard;
use crate::core::tools::primitive::{BashTaskRegistry, BashTaskStatus};
use crate::infra::audit::TracingAuditRecorder;

fn make_guard(
    workspace_root: &std::path::Path,
    path_rules: Vec<PathRule>,
    bash_forbidden: Vec<String>,
    bash_ast: BashAstChecker,
) -> BackgroundBashGuard {
    let guard = Arc::new(DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir: workspace_root.to_path_buf(),
            workspace_roots: vec![workspace_root.to_path_buf()],
            agent_trail_readonly_dirs: vec![],
            user_path_rules: path_rules,
            user_bash_forbidden: bash_forbidden,
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    ));
    BackgroundBashGuard::new(
        "__agent__",
        guard,
        Arc::new(AllowAllConfirmation),
        Arc::new(TracingAuditRecorder),
        bash_ast,
    )
}

#[tokio::test]
async fn spawn_then_read_then_stop_then_list() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = BashTaskRegistry::new(dir.path().join("tool-results"));

    // 起一个会持续输出的后台任务（每 100ms 一行）。
    let ticket = reg
        .spawn(
            "i=0; while [ $i -lt 50 ]; do echo line-$i; i=$((i+1)); sleep 0.1; done".to_string(),
            None,
            None,
        )
        .await
        .expect("spawn");

    // 等几行写出来再拉。
    tokio::time::sleep(std::time::Duration::from_millis(350)).await;

    let chunk1 = reg.read_output(&ticket.task_id, None).await.expect("read1");
    assert!(chunk1.start_offset == 0, "首读 since=None → start=0");
    assert!(
        chunk1.next_offset > 0,
        "应有字节读出，实际 = {}",
        chunk1.next_offset
    );
    assert!(!chunk1.finished, "Running 期间 finished=false");
    assert!(
        chunk1.content.contains("line-0"),
        "内容应含 line-0，实际 = {:?}",
        chunk1.content
    );

    // 续读：since=next_offset → 之间又有新行。
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    let chunk2 = reg
        .read_output(&ticket.task_id, Some(chunk1.next_offset))
        .await
        .expect("read2");
    assert_eq!(chunk2.start_offset, chunk1.next_offset);

    // stop 后，list 中状态应为 Stopped。
    reg.stop(&ticket.task_id).await.expect("stop");
    // 给 wait 任务一点点时间让 child reap 完成（status 不会被覆盖）。
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    let infos = reg.list();
    assert_eq!(infos.len(), 1);
    assert_eq!(
        infos[0].status,
        BashTaskStatus::Stopped,
        "stop 后 status 必须为 Stopped，不被 wait 任务回退覆盖"
    );

    // 再读一次：finished=true，exit_code=Some(-1)（Stopped）。
    let chunk_final = reg
        .read_output(&ticket.task_id, Some(0))
        .await
        .expect("read3");
    assert!(chunk_final.finished, "Stopped 后 finished=true");
    assert_eq!(chunk_final.exit_code, Some(-1));
}

#[tokio::test]
async fn natural_finish_marks_finished_with_exit_code() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = BashTaskRegistry::new(dir.path().join("tool-results"));
    let ticket = reg
        .spawn("echo hi; exit 7".to_string(), None, None)
        .await
        .expect("spawn");
    // 等子进程自然结束。
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let infos = reg.list();
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].status, BashTaskStatus::Finished { exit_code: 7 });
    let chunk = reg.read_output(&ticket.task_id, None).await.expect("read");
    assert!(chunk.finished);
    assert_eq!(chunk.exit_code, Some(7));
    assert!(chunk.content.contains("hi"));
}

#[tokio::test]
async fn spawn_empty_argv_uses_shell_mode() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = BashTaskRegistry::new(dir.path().join("tool-results"));
    let ticket = reg
        .spawn("echo bg-empty-argv".to_string(), Some(vec![]), None)
        .await
        .expect("spawn");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let infos = reg.list();
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].status, BashTaskStatus::Finished { exit_code: 0 });
    let chunk = reg.read_output(&ticket.task_id, None).await.expect("read");
    assert!(chunk.finished);
    assert_eq!(chunk.exit_code, Some(0));
    assert!(chunk.content.contains("bg-empty-argv"));
}

#[tokio::test]
async fn spawn_shell_launcher_command_merges_with_argv() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = BashTaskRegistry::new(dir.path().join("tool-results"));
    let ticket = reg
        .spawn(
            "sh -c".to_string(),
            Some(vec!["printf bg-shell-launch-ok".to_string()]),
            None,
        )
        .await
        .expect("spawn");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let infos = reg.list();
    assert_eq!(infos.len(), 1);
    assert_eq!(infos[0].status, BashTaskStatus::Finished { exit_code: 0 });
    let chunk = reg.read_output(&ticket.task_id, None).await.expect("read");
    assert!(chunk.finished);
    assert_eq!(chunk.exit_code, Some(0));
    assert!(chunk.content.contains("bg-shell-launch-ok"));
}

#[tokio::test]
async fn read_output_unknown_task_id_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = BashTaskRegistry::new(dir.path().join("tool-results"));
    let err = reg
        .read_output("not-exist", None)
        .await
        .expect_err("应当 not found");
    assert!(format!("{}", err).contains("not found"));
}

// ─── P1（bash background monitor）追加 ─────────────────────────────────────

/// `wait_for_finish` 不会因为持续新输出而提前返回；只有终态才会结束等待。
#[tokio::test]
async fn wait_for_finish_ignores_new_output_until_terminal() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = std::sync::Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let ticket = reg
        .spawn(
            "i=0; while [ $i -lt 30 ]; do echo line-$i; i=$((i+1)); sleep 0.1; done".to_string(),
            None,
            None,
        )
        .await
        .expect("spawn");

    tokio::time::sleep(std::time::Duration::from_millis(250)).await;
    let pending = tokio::time::timeout(
        std::time::Duration::from_millis(350),
        reg.wait_for_finish(&ticket.task_id),
    )
    .await;
    assert!(
        pending.is_err(),
        "持续新输出时 wait_for_finish 不应提前返回，实际 = {:?}",
        pending
    );

    reg.stop(&ticket.task_id).await.expect("stop");
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        reg.wait_for_finish(&ticket.task_id),
    )
    .await
    .expect("wait_for_finish 应在 stop 后返回")
    .expect("wait_for_finish err");
}

/// 任务自然结束 → wait_for_finish 返回。
#[tokio::test]
async fn wait_for_finish_returns_on_natural_exit() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = std::sync::Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let ticket = reg
        .spawn("echo done; exit 0".to_string(), None, None)
        .await
        .expect("spawn");
    tokio::time::timeout(
        std::time::Duration::from_secs(2),
        reg.wait_for_finish(&ticket.task_id),
    )
    .await
    .expect("timeout")
    .expect("err");
    let info = reg.list();
    assert!(matches!(info[0].status, BashTaskStatus::Finished { .. }));
}

/// subscribe_lifecycle：同一 task 的 finished 事件**只发一次**（验证
/// stop+wait 双触发收敛于 lifecycle_emitted guard）。
#[tokio::test]
async fn subscribe_lifecycle_emits_once_per_task() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = BashTaskRegistry::new(dir.path().join("tool-results"));
    let mut rx = reg.subscribe_lifecycle();
    let ticket = reg
        .spawn(
            "i=0; while [ $i -lt 50 ]; do echo line-$i; i=$((i+1)); sleep 0.05; done".to_string(),
            None,
            None,
        )
        .await
        .expect("spawn");
    // 主动 stop → 触发翻 Stopped → emit lifecycle 一次。wait 任务后续 child.wait
    // 返回时再尝试 emit 但被 lifecycle_emitted guard 挡掉。
    reg.stop(&ticket.task_id).await.expect("stop");

    // 第一条事件必须能在 1s 内拿到。
    let first = tokio::time::timeout(std::time::Duration::from_secs(1), rx.recv())
        .await
        .expect("第一条 lifecycle 必须在 1s 内到")
        .expect("recv");
    assert_eq!(first.task_id, ticket.task_id);
    assert!(matches!(first.final_status, BashTaskStatus::Stopped));

    // 给 wait 任务一段时间，确认它**不**重复发。
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let second = tokio::time::timeout(std::time::Duration::from_millis(50), rx.recv()).await;
    assert!(
        second.is_err(),
        "lifecycle 不应重复 emit；实际收到 {:?}",
        second.ok()
    );
}

/// tail_log 取末尾 ≤ N 字节，UTF-8 lossy。
#[tokio::test]
async fn tail_log_returns_suffix() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = BashTaskRegistry::new(dir.path().join("tool-results"));
    let ticket = reg
        .spawn(
            "for i in 1 2 3 4 5; do echo line-$i; done".to_string(),
            None,
            None,
        )
        .await
        .expect("spawn");
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    let tail = reg.tail_log(&ticket.task_id, 4096).await;
    assert!(
        tail.contains("line-5"),
        "tail 应包含末尾行，实际 = {:?}",
        tail
    );
    // 截断到极小：仍应是有效 UTF-8 lossy 字符串。
    let small = reg.tail_log(&ticket.task_id, 10).await;
    assert!(
        small.len() <= 10,
        "tail_log(max=10) 长度 {} 应不超过 10",
        small.len()
    );
}

#[tokio::test]
async fn spawn_denied_by_path_preflight_has_no_task_side_effect() {
    let dir = tempfile::tempdir().expect("tempdir");
    let denied_root = dir.path().join("denied");
    std::fs::create_dir_all(&denied_root).unwrap();
    let reg =
        BashTaskRegistry::new(dir.path().join("tool-results")).with_background_guard(make_guard(
            dir.path(),
            vec![PathRule::new(
                denied_root.to_string_lossy(),
                PathRuleMode::Deny,
            )],
            vec![],
            BashAstChecker::default(),
        ));
    let err = reg
        .spawn(
            format!("ls {}", denied_root.join("secret.txt").display()),
            None,
            Some(dir.path().to_path_buf()),
        )
        .await
        .expect_err("denied path should fail before spawn");
    assert!(err.to_string().contains("deny") || err.to_string().contains("拒绝"));
    assert!(
        reg.list().is_empty(),
        "preflight deny must not register a task"
    );
}

#[tokio::test]
async fn spawn_denied_by_bash_ast_has_no_task_side_effect() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg =
        BashTaskRegistry::new(dir.path().join("tool-results")).with_background_guard(make_guard(
            dir.path(),
            vec![],
            vec![],
            BashAstChecker::new(true, vec![], vec!["rm".to_string()]),
        ));
    let err = reg
        .spawn(
            "git --version && rm -rf ./danger".to_string(),
            None,
            Some(dir.path().to_path_buf()),
        )
        .await
        .expect_err("ast deny should fail before spawn");
    assert!(err.to_string().contains("AstDeny"));
    assert!(reg.list().is_empty(), "AST deny must not register a task");
}

#[tokio::test]
async fn spawn_denied_by_bash_policy_has_no_task_side_effect() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg =
        BashTaskRegistry::new(dir.path().join("tool-results")).with_background_guard(make_guard(
            dir.path(),
            vec![],
            vec![r"\becho\b".to_string()],
            BashAstChecker::default(),
        ));
    let err = reg
        .spawn(
            "echo should-not-run".to_string(),
            None,
            Some(dir.path().to_path_buf()),
        )
        .await
        .expect_err("forbidden bash should fail before spawn");
    assert!(err.to_string().contains("forbidden") || err.to_string().contains("拒绝"));
    assert!(
        reg.list().is_empty(),
        "policy deny must not register a task"
    );
}

#[tokio::test]
async fn runtime_preview_is_bounded_by_bytes_and_tracks_offsets() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = BashTaskRegistry::new(dir.path().join("tool-results"));
    let ticket = reg
        .spawn("python3 -c 'print(\"x\" * 70000)'".to_string(), None, None)
        .await
        .expect("spawn");
    reg.wait_for_finish(&ticket.task_id).await.expect("finish");
    let preview = reg.runtime_preview(&ticket.task_id).expect("preview");
    assert!(preview.output.len() <= 64 * 1024);
    assert!(preview.truncated);
    assert!(preview.start_offset > 0);
    assert!(preview.next_offset >= 70_000);
    assert!(preview.sequence > 1);
    let full = reg
        .read_output(&ticket.task_id, None)
        .await
        .expect("full log");
    assert!(
        full.content.len() > preview.output.len(),
        "durable log must not use preview limit"
    );
}

#[tokio::test]
async fn runtime_preview_is_bounded_by_lines_and_streams_are_visible() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = BashTaskRegistry::new(dir.path().join("tool-results"));
    let mut rx = reg.subscribe_output();
    let ticket = reg
        .spawn(
            "i=0; while [ $i -lt 1100 ]; do echo out-$i; echo err-$i >&2; i=$((i+1)); done"
                .to_string(),
            None,
            None,
        )
        .await
        .expect("spawn");
    reg.wait_for_finish(&ticket.task_id).await.expect("finish");
    let preview = reg.runtime_preview(&ticket.task_id).expect("preview");
    assert!(preview.output.lines().count() <= 1000);
    assert!(preview.truncated);
    let mut stdout = false;
    let mut stderr = false;
    while let Ok(event) = rx.try_recv() {
        if event.task_id != ticket.task_id {
            continue;
        }
        stdout |= event.stream == crate::core::tools::primitive::BashOutputStream::Stdout
            && !event.output.is_empty();
        stderr |= event.stream == crate::core::tools::primitive::BashOutputStream::Stderr
            && !event.output.is_empty();
    }
    assert!(stdout && stderr, "both pump streams must be observable");
}

#[tokio::test]
async fn completion_event_follows_last_output_and_log_flush() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = BashTaskRegistry::new(dir.path().join("tool-results"));
    let mut rx = reg.subscribe_output();
    let ticket = reg
        .spawn("printf final-byte".to_string(), None, None)
        .await
        .expect("spawn");
    reg.wait_for_finish(&ticket.task_id).await.expect("finish");
    let mut saw_output = false;
    loop {
        let event = rx.recv().await.expect("event");
        if event.task_id != ticket.task_id {
            continue;
        }
        if event.completed {
            assert!(saw_output, "completion must follow output");
            break;
        }
        saw_output |= event.output.contains("final-byte");
    }
    let full = reg.read_output(&ticket.task_id, None).await.expect("full");
    assert!(full.content.contains("final-byte"));
}

#[tokio::test]
async fn lifecycle_waits_for_final_preview_barrier() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = BashTaskRegistry::new(dir.path().join("tool-results"));
    let mut lifecycle = reg.subscribe_lifecycle();
    let ticket = reg
        .spawn_tracked_with_preview_barrier("printf final-preview".to_string(), None, None, true)
        .await
        .expect("spawn");

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(30), lifecycle.recv())
            .await
            .is_err(),
        "lifecycle must not overtake the bridge flush acknowledgement"
    );
    let info = reg.get_info(&ticket.task_id).expect("task");
    assert_eq!(info.status, BashTaskStatus::DrainingOutput);

    reg.acknowledge_preview_flushed(&ticket.task_id)
        .expect("ack");
    let event = tokio::time::timeout(std::time::Duration::from_secs(1), lifecycle.recv())
        .await
        .expect("lifecycle timeout")
        .expect("lifecycle");
    assert_eq!(event.task_id, ticket.task_id);
    assert_eq!(
        event.final_status,
        BashTaskStatus::Finished { exit_code: 0 }
    );
}

#[tokio::test]
async fn stop_finishes_only_after_output_drain_and_preview_barrier() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = BashTaskRegistry::new(dir.path().join("tool-results"));
    let mut output = reg.subscribe_output();
    let mut lifecycle = reg.subscribe_lifecycle();
    let ticket = reg
        .spawn_tracked_with_preview_barrier(
            "printf before-stop; sleep 30".to_string(),
            None,
            None,
            true,
        )
        .await
        .expect("spawn");
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    reg.stop(&ticket.task_id).await.expect("stop request");

    assert!(
        tokio::time::timeout(
            std::time::Duration::from_millis(50),
            reg.wait_for_finish(&ticket.task_id)
        )
        .await
        .is_err(),
        "wait_for_finish must remain blocked until bridge acknowledgement"
    );
    assert!(
        tokio::time::timeout(std::time::Duration::from_millis(20), lifecycle.recv())
            .await
            .is_err(),
        "stop must not publish lifecycle before drain"
    );

    let mut saw_output = false;
    let mut saw_completion = false;
    while let Ok(event) = output.try_recv() {
        if event.task_id == ticket.task_id {
            saw_output |= event.output.contains("before-stop");
            saw_completion |= event.completed;
        }
    }
    assert!(
        saw_output && saw_completion,
        "pumps and completion marker must drain first"
    );

    reg.acknowledge_preview_flushed(&ticket.task_id)
        .expect("ack");
    reg.wait_for_finish(&ticket.task_id).await.expect("finish");
    let event = lifecycle.recv().await.expect("lifecycle");
    assert_eq!(event.final_status, BashTaskStatus::Stopped);
}

#[tokio::test]
async fn stderr_preview_offsets_match_durable_combined_log() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = BashTaskRegistry::new(dir.path().join("tool-results"));
    let ticket = reg
        .spawn("printf error-byte >&2".to_string(), None, None)
        .await
        .expect("spawn");
    reg.wait_for_finish(&ticket.task_id).await.expect("finish");

    let preview = reg.runtime_preview(&ticket.task_id).expect("preview");
    let durable = reg.read_output(&ticket.task_id, None).await.expect("log");
    assert_eq!(preview.output, "STDERR: error-byte");
    assert_eq!(preview.output, durable.content);
    assert_eq!(preview.start_offset, 0);
    assert_eq!(preview.next_offset, durable.next_offset);
}
