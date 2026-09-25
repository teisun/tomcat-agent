//! Real CLI startup: one project decision before chat input, isolated HOME and no network.
mod common;

use std::fs;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use common::serve::{cargo_bin_path, setup_serve_fixture};

fn isolated_command(home: &Path, cwd: &Path) -> Command {
    let mut command = Command::new(cargo_bin_path());
    for (key, _) in std::env::vars_os() {
        let name = key.to_string_lossy();
        if name == "TOMCAT_AGENT_ACTIVE"
            || name.starts_with("TOMCAT__")
            || name.ends_with("_API_KEY")
            || matches!(
                name.to_ascii_lowercase().as_str(),
                "http_proxy" | "https_proxy" | "all_proxy"
            )
        {
            command.env_remove(key);
        }
    }
    command
        .env("HOME", home)
        .env("SHELL", "/bin/zsh")
        .env("OPENAI_API_KEY", "offline-test-only-placeholder")
        .current_dir(cwd)
        .arg("code");
    command
}

fn wait_for_exit(child: &mut Child) {
    let deadline = Instant::now() + Duration::from_secs(25);
    loop {
        if child.try_wait().unwrap().is_some() {
            return;
        }
        if Instant::now() >= deadline {
            child.kill().ok();
            child.wait().ok();
            panic!("CLI failed to exit after EOF");
        }
        std::thread::sleep(Duration::from_millis(25));
    }
}

#[cfg(unix)]
struct Interactive {
    child: Child,
    input: std::fs::File,
    output: std::sync::mpsc::Receiver<String>,
    seen: String,
}

#[cfg(unix)]
impl Interactive {
    fn start(home: &Path, cwd: &Path) -> Self {
        use std::os::fd::FromRawFd;
        let (mut master, mut slave) = (-1, -1);
        // SAFETY: the descriptors are output parameters; null pointers ask for PTY defaults.
        assert_eq!(
            unsafe {
                libc::openpty(
                    &mut master,
                    &mut slave,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                )
            },
            0
        );
        // SAFETY: openpty returned owned descriptors; File takes each exactly once.
        let input = unsafe { fs::File::from_raw_fd(master) };
        let terminal = unsafe { fs::File::from_raw_fd(slave) };
        let mut child = isolated_command(home, cwd)
            .stdin(Stdio::from(terminal.try_clone().unwrap()))
            .stdout(Stdio::from(terminal.try_clone().unwrap()))
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn interactive CLI");
        drop(terminal);
        let mut reader = input.try_clone().unwrap();
        let (tx, output) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let mut chunk = [0u8; 4096];
            loop {
                match reader.read(&mut chunk) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if tx
                            .send(String::from_utf8_lossy(&chunk[..n]).into_owned())
                            .is_err()
                        {
                            break;
                        }
                    }
                }
            }
        });
        if child.try_wait().unwrap().is_some() {
            panic!("CLI exited before project trust prompt");
        }
        Self {
            child,
            input,
            output,
            seen: String::new(),
        }
    }

    fn until(&mut self, marker: &str) {
        let deadline = Instant::now() + Duration::from_secs(25);
        while !self.seen.contains(marker) {
            let remaining = deadline.saturating_duration_since(Instant::now());
            assert!(
                !remaining.is_zero(),
                "CLI did not print {marker:?}: {:?}",
                self.seen
            );
            self.seen.push_str(
                &self
                    .output
                    .recv_timeout(remaining)
                    .expect("CLI PTY closed or timed out"),
            );
        }
    }

    fn answer(&mut self, answer: &str) {
        self.input.write_all(answer.as_bytes()).unwrap();
        self.input.flush().unwrap();
    }
}

#[cfg(unix)]
impl Drop for Interactive {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            self.child.kill().ok();
        }
        self.child.wait().ok();
    }
}

#[cfg(unix)]
#[test]
fn tty_trust_once_then_does_not_prompt_again() {
    let fixture = setup_serve_fixture("http://127.0.0.1:1");
    let mut first = Interactive::start(&fixture.home_path, &fixture.workspace);
    first.until("Trust this project?");
    assert!(first
        .seen
        .contains(&fixture.workspace.to_string_lossy().to_string()));
    let record = fixture.home_path.join(".tomcat/project-trust.json");
    assert!(
        !record.exists(),
        "prompt must not grant trust until the user says yes"
    );
    first.answer("y\n");
    first.until("[y/N] y");
    let deadline = Instant::now() + Duration::from_secs(10);
    while !record.exists() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(record.exists(), "yes must persist the project decision");
    first.answer("\u{4}");
    wait_for_exit(&mut first.child);
    drop(first);

    let mut second = Interactive::start(&fixture.home_path, &fixture.workspace);
    second.until("输入 /help 查看命令列表。");
    second.answer("\u{4}");
    wait_for_exit(&mut second.child);
    while let Ok(chunk) = second.output.try_recv() {
        second.seen.push_str(&chunk);
    }
    assert!(
        !second.seen.contains("Trust this project?"),
        "trusted project should not prompt on restart"
    );
}

#[cfg(unix)]
#[test]
fn tty_not_now_does_not_persist_and_non_tty_keeps_first_command() {
    let fixture = setup_serve_fixture("http://127.0.0.1:1");
    let record = fixture.home_path.join(".tomcat/project-trust.json");
    let mut child = Interactive::start(&fixture.home_path, &fixture.workspace);
    child.until("Trust this project?");
    child.answer("n\n");
    child.answer("\u{4}");
    wait_for_exit(&mut child.child);
    assert!(
        !record.exists(),
        "not now must not write a denial or a grant"
    );
    drop(child);

    let mut piped = isolated_command(&fixture.home_path, &fixture.workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    piped.stdin.take().unwrap().write_all(b"/help\n").unwrap();
    let output = piped.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        !text.contains("Trust this project?"),
        "piped first line must not be consumed by trust"
    );
    assert!(
        text.contains("可用命令："),
        "piped /help must reach the chat command handler: {text}"
    );
    assert!(!record.exists());
}

#[test]
fn legacy_mcp_config_does_not_abort_or_disable_piped_chat() {
    let fixture = setup_serve_fixture("http://127.0.0.1:1");
    fs::write(
        fixture.home_path.join(".tomcat/mcp.json"),
        r#"{"mcpServers":{"legacy":{"command":"node","startupTimeoutMs":5000,"trusted":true,"integrity":"obsolete"}}}"#,
    )
    .unwrap();
    let mut piped = isolated_command(&fixture.home_path, &fixture.workspace)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    piped.stdin.take().unwrap().write_all(b"/help\n").unwrap();
    let output = piped.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(
        !text.contains("[MCP 未加载]"),
        "legacy keys must not disable MCP: {text}"
    );
    assert!(
        text.contains("可用命令："),
        "chat command must still work: {text}"
    );
}
