//! `BashTaskRegistry` 后台 spawn / read_output / stop / list 行为单测。
//!
//! 仅触达 `crate::core::tools::primitive::{BashTaskRegistry, BashTaskStatus}`
//! 公共导出；与 [`crate::core::tools::primitive::bash_task`] 模块文档保持一致。

use crate::core::tools::primitive::{BashTaskRegistry, BashTaskStatus};

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
async fn read_output_unknown_task_id_errors() {
    let dir = tempfile::tempdir().expect("tempdir");
    let reg = BashTaskRegistry::new(dir.path().join("tool-results"));
    let err = reg
        .read_output("not-exist", None)
        .await
        .expect_err("应当 not found");
    assert!(format!("{}", err).contains("not found"));
}
