use std::process::Stdio;
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use super::{exit_code, ProcessTree};
use crate::infra::platform::posix_shell;

fn shell_command(script: &str) -> tokio::process::Command {
    let mut command = tokio::process::Command::new(posix_shell().unwrap());
    command
        .arg("-c")
        .arg(script)
        .kill_on_drop(true)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    command
}

fn shell_status(script: &str) -> std::process::ExitStatus {
    std::process::Command::new(posix_shell().unwrap())
        .arg("-c")
        .arg(script)
        .stdin(Stdio::null())
        .status()
        .unwrap()
}

#[tokio::test]
async fn kill_ends_background_grandchildren() {
    let (mut child, tree) =
        ProcessTree::spawn(&mut shell_command("sleep 30 & echo ready; wait")).unwrap();
    let mut stdout = BufReader::new(child.stdout.take().unwrap());
    let mut first_line = String::new();
    tokio::time::timeout(Duration::from_secs(10), stdout.read_line(&mut first_line))
        .await
        .expect("shell did not start the background sleep in time")
        .unwrap();
    assert_eq!(first_line.trim(), "ready");

    tree.kill().unwrap();

    // 后台 sleep 也持有这根管道：读到 EOF 说明整棵树都结束了。
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), stdout.read_to_end(&mut rest))
        .await
        .expect("a process in the tree still holds stdout open")
        .unwrap();
    let _ = child.wait().await;
}

#[tokio::test]
async fn kill_after_exit_is_ok() {
    let (mut child, tree) = ProcessTree::spawn(&mut shell_command("exit 0")).unwrap();
    child.wait().await.unwrap();
    tree.kill().unwrap();
}

#[tokio::test]
async fn kill_after_exit_before_reaping_is_ok() {
    let (mut child, tree) = ProcessTree::spawn(&mut shell_command("exit 0")).unwrap();
    // 读到 EOF 说明 shell 已退出；不调用 wait，进程就停在"已退出、未回收"的状态。
    let mut rest = Vec::new();
    tokio::time::timeout(
        Duration::from_secs(10),
        child.stdout.take().unwrap().read_to_end(&mut rest),
    )
    .await
    .expect("shell did not exit in time")
    .unwrap();
    // 关闭 stdout 到进程彻底退出之间还有一小段时间。
    tokio::time::sleep(Duration::from_millis(200)).await;
    tree.kill().unwrap();
    child.wait().await.unwrap();
}

#[test]
fn exit_code_reports_shell_exit_status() {
    assert_eq!(exit_code(&shell_status("exit 3")), 3);
}

#[cfg(unix)]
#[test]
fn exit_code_maps_signal_to_128_plus_signal() {
    assert_eq!(exit_code(&shell_status("kill -9 $$")), 137);
}

#[test]
fn spawn_detached_returns_at_once_and_appends_output_to_the_log() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("detached install.log");
    let shell = posix_shell().unwrap();

    let started = std::time::Instant::now();
    super::spawn_detached(
        &shell.to_string_lossy(),
        &["-c", "sleep 2; echo done"],
        &[],
        &log,
    )
    .unwrap();
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "spawn_detached must not wait for the command"
    );

    let deadline = std::time::Instant::now() + Duration::from_secs(20);
    while !std::fs::read_to_string(&log).is_ok_and(|text| text.contains("done")) {
        assert!(
            std::time::Instant::now() < deadline,
            "detached command never wrote its output to the log: {:?}",
            std::fs::read_to_string(&log)
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

#[cfg(unix)]
#[test]
fn nohup_shell_command_quotes_log_path_with_spaces() {
    let cmd = super::nohup_shell_command(
        "brew",
        &["install", "--force-bottle", "ripgrep", "fd"],
        &[("HOMEBREW_NO_BUILD_FROM_SOURCE", "1")],
        std::path::Path::new("/tmp/fake preflight.log"),
    );
    assert_eq!(
        cmd,
        "HOMEBREW_NO_BUILD_FROM_SOURCE=1 nohup brew install --force-bottle ripgrep fd \
         >> '/tmp/fake preflight.log' 2>&1 &"
    );
}

#[cfg(unix)]
#[test]
fn nohup_shell_command_without_envs_starts_with_nohup() {
    let cmd = super::nohup_shell_command(
        "apt-get",
        &["install", "-y", "ripgrep", "fd-find"],
        &[],
        std::path::Path::new("/tmp/p.log"),
    );
    assert_eq!(
        cmd,
        "nohup apt-get install -y ripgrep fd-find >> /tmp/p.log 2>&1 &"
    );
}
