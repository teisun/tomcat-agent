//! Real CLI startup: one project decision before chat input, isolated HOME and no network.
mod common;

use std::fs;
#[cfg(unix)]
use std::io::Read;
use std::io::Write;
use std::path::Path;
#[cfg(unix)]
use std::process::Child;
use std::process::{Command, Stdio};
#[cfg(unix)]
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
        .env("TOMCAT__UI__LANGUAGE", "en")
        .env("OPENAI_API_KEY", "offline-test-only-placeholder")
        .current_dir(cwd)
        .arg("code");
    command
}

#[cfg(unix)]
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
    output: std::sync::mpsc::Receiver<Vec<u8>>,
    seen: Vec<u8>,
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
            .env("TERM", "xterm")
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
                    Ok(0) => break,
                    Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                    Ok(n) => {
                        if tx.send(chunk[..n].to_vec()).is_err() {
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
            seen: Vec::new(),
        }
    }

    fn marker_offset(&self, marker: &str) -> Option<usize> {
        // PTY reads may split a UTF-8 code point. Match on the assembled bytes.
        self.seen
            .windows(marker.len())
            .position(|bytes| bytes == marker.as_bytes())
    }

    fn until(&mut self, marker: &str) -> usize {
        let deadline = Instant::now() + Duration::from_secs(25);
        loop {
            if let Some(offset) = self.marker_offset(marker) {
                return offset + marker.len();
            }
            match self
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(chunk) => self.seen.extend(chunk),
                Err(error) => panic!(
                    "CLI PTY waiting for {marker:?}: {error:?}; saw {:?}",
                    String::from_utf8_lossy(&self.seen),
                ),
            }
        }
    }

    fn answer(&mut self, answer: &str) {
        self.input.write_all(answer.as_bytes()).unwrap();
        self.input.flush().unwrap();
    }

    /// 等主输入框的提示符出现再发 Ctrl+D：在那之前终端还处于行模式，Linux 上此时的
    /// Ctrl+D 被行模式处理掉，行编辑器收不到 EOF。
    fn quit_at_prompt(&mut self) {
        self.until("]> ");
        self.answer("\u{4}");
        wait_for_exit(&mut self.child);
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
        .marker_offset(&fixture.workspace.to_string_lossy())
        .is_some());
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
    first.quit_at_prompt();
    drop(first);

    let mut second = Interactive::start(&fixture.home_path, &fixture.workspace);
    second.until("Enter /help for a command list.");
    second.quit_at_prompt();
    while let Ok(chunk) = second.output.try_recv() {
        second.seen.extend(chunk);
    }
    assert!(
        second.marker_offset("Trust this project?").is_none(),
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
    child.quit_at_prompt();
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
        text.contains("Available commands:"),
        "piped /help must reach the chat command handler: {text}"
    );
    assert!(!record.exists());
}

#[cfg(unix)]
#[test]
fn tty_background_explorer_wakes_only_before_the_first_key() {
    use common::serve::{
        response, spawn_routed_openai_stream_server, sse_delta, sse_done, sse_finish, sse_tool_call,
    };
    use serde_json::json;
    use std::sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    };
    for typed in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let main = calls.clone();
        let release = Arc::new(AtomicBool::new(false));
        let gate = release.clone();
        let server = spawn_routed_openai_stream_server(move |req| {
            let msgs = &req["messages"];
            let answer = |s: &str| response(vec![sse_delta(s), sse_finish("stop"), sse_done()]);
            if msgs[0]["content"]
                .as_str()
                .is_some_and(|s| s.starts_with("You are an Explorer subagent"))
            {
                let start = Instant::now();
                while !gate.load(Ordering::Acquire) && start.elapsed() < Duration::from_secs(20) {
                    std::thread::sleep(Duration::from_millis(10));
                }
                return answer("child-tty-result");
            }
            if !req["tools"].is_array() {
                return answer("Title");
            }
            match main.fetch_add(1, Ordering::SeqCst) {
                0 => {
                    let args = json!({"tasks":[{"type":"explorer","description":"Inspect","prompt":"Inspect the fixture"}],"run_in_background":true});
                    response(vec![
                        sse_tool_call("dispatch", "dispatch_agent", &args.to_string()),
                        sse_finish("tool_calls"),
                        sse_done(),
                    ])
                }
                1 => answer("Parent ready"),
                _ => {
                    let payload = msgs.to_string();
                    assert!(payload.contains("child-tty-result"));
                    if typed {
                        assert_eq!(payload.matches("保留草稿").count(), 1);
                    }
                    answer("Combined findings")
                }
            }
        });
        let fx = setup_serve_fixture(&server.base_url);
        let mut child = Interactive::start(&fx.home_path, &fx.workspace);
        child.until("Trust this project?");
        child.answer("n\n");
        child.until("]> ");
        child.seen.clear();
        child.answer("inspect fixture\n");
        let after_parent = child.until("Parent ready");
        child.seen.drain(..after_parent);
        child.until("]> ");
        child.seen.clear();
        if typed {
            child.answer("\x1b[200~保留草稿\x1b[201~");
            child.until("保留草稿");
        }
        release.store(true, Ordering::Release);
        if typed {
            std::thread::sleep(Duration::from_millis(250));
            assert_eq!(calls.load(Ordering::SeqCst), 2);
            child.answer("\n");
        }
        let after_combined = child.until("Combined findings");
        child.seen.drain(..after_combined);
        child.quit_at_prompt();
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }
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
        !text.contains(&tomcat::infra::tr(
            "terminal.mcpUnavailable",
            &[("detail", "")]
        )),
        "legacy keys must not disable MCP: {text}"
    );
    assert!(
        text.contains("Available commands:"),
        "chat command must still work: {text}"
    );
}
