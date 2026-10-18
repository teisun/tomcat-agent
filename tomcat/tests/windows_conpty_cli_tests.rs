//! 真实 Windows 控制台（ConPTY）里的 CLI 提问面板：Ctrl+C 立即打断，中文自定义答案完整记录。
//!
//! 控制台分支只有在真实控制台里才能测；ConPTY 给测试一个隔离的控制台，不会往
//! 开发者自己的终端里塞按键。
#![cfg(windows)]

mod common;

use std::io::{Read, Write};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::serve::{
    captured_non_title_requests, cargo_bin_path, isolated_serve_command, response,
    setup_serve_fixture, spawn_scripted_openai_stream_server, sse_delta, sse_done, sse_finish,
    sse_tool_call, ServeFixture,
};
use portable_pty::{native_pty_system, Child, CommandBuilder, PtySize};

const MAIN_PROMPT: &str = "|gpt-5.4]> ";

struct ConsoleCli {
    child: Box<dyn Child + Send + Sync>,
    writer: Arc<Mutex<Box<dyn Write + Send>>>,
    screen: Arc<Mutex<Vec<u8>>>,
}

impl ConsoleCli {
    fn spawn(fx: &ServeFixture) -> Self {
        let pty = native_pty_system()
            .openpty(PtySize {
                rows: 40,
                cols: 160,
                pixel_width: 0,
                pixel_height: 0,
            })
            .expect("open ConPTY");
        let mut builder = CommandBuilder::new(cargo_bin_path());
        builder.cwd(&fx.workspace);
        for (key, value) in isolated_serve_command(&fx.home_path).get_envs() {
            match value {
                Some(value) => builder.env(key, value),
                None => builder.env_remove(key),
            }
        }
        builder.env("OPENAI_API_KEY", "dummy-key");
        builder.arg("chat");
        let child = pty.slave.spawn_command(builder).expect("spawn tomcat chat");
        drop(pty.slave);

        let writer = Arc::new(Mutex::new(pty.master.take_writer().expect("pty writer")));
        let screen = Arc::new(Mutex::new(Vec::new()));
        let mut reader = pty.master.try_clone_reader().expect("pty reader");
        let (sink, replies) = (screen.clone(), writer.clone());
        std::thread::spawn(move || {
            let _master = pty.master;
            let mut buffer = [0u8; 4096];
            while let Ok(count) = reader.read(&mut buffer) {
                if count == 0 {
                    break;
                }
                let chunk = &buffer[..count];
                // ConPTY 启动时询问光标位置，不回答会一直等。
                if chunk.windows(4).any(|window| window == b"\x1b[6n") {
                    let _ = replies.lock().unwrap().write_all(b"\x1b[1;1R");
                }
                sink.lock().unwrap().extend_from_slice(chunk);
            }
        });
        Self {
            child,
            writer,
            screen,
        }
    }

    fn send(&self, keys: &str) {
        let mut writer = self.writer.lock().unwrap();
        writer
            .write_all(keys.as_bytes())
            .expect("write console input");
        writer.flush().expect("flush console input");
    }

    /// 当前输出位置；配合 [`Self::wait_for`] 只看之后的新输出（rustyline 会重绘提示符）。
    fn mark(&self) -> usize {
        self.screen.lock().unwrap().len()
    }

    fn screen_since(&self, mark: usize) -> String {
        let raw = self.screen.lock().unwrap();
        strip_vt(&String::from_utf8_lossy(&raw[mark.min(raw.len())..]))
    }

    fn wait_for(&mut self, needle: &str, mark: usize, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while !self.screen_since(mark).contains(needle) {
            if Instant::now() >= deadline {
                let raw = String::from_utf8_lossy(&self.screen.lock().unwrap()).into_owned();
                panic!(
                    "waited {timeout:?} for {needle:?}; child={:?}; raw={raw:?}; screen:\n{}",
                    self.child.try_wait(),
                    strip_vt(&raw)
                );
            }
            std::thread::sleep(Duration::from_millis(20));
        }
    }
}

impl Drop for ConsoleCli {
    fn drop(&mut self) {
        let _ = self.child.kill();
    }
}

fn ask_question_turn(call_id: &str) -> common::serve::ScriptedResponse {
    let args = serde_json::json!({
        "questions": [{
            "id": "deploy",
            "prompt": "选择部署方式",
            "options": [
                {"id": "docker", "label": "Docker", "recommended": true},
                {"id": "metal", "label": "Bare-metal"}
            ]
        }]
    })
    .to_string();
    response(vec![
        sse_tool_call(call_id, "ask_question", &args),
        sse_finish("tool_calls"),
        sse_done(),
    ])
}

/// 提问时按 Ctrl+C：2 秒内回到主输入框；下一轮输入 `c 中文答案` 原样送达模型。
#[test]
fn ctrl_c_releases_a_pending_question_and_chinese_custom_answer_reaches_the_model() {
    let server = spawn_scripted_openai_stream_server(vec![
        ask_question_turn("call_q1"),
        ask_question_turn("call_q2"),
        response(vec![
            sse_delta("ANSWER_RECORDED"),
            sse_finish("stop"),
            sse_done(),
        ]),
    ]);
    let fx = setup_serve_fixture(&server.base_url);
    let mut cli = ConsoleCli::spawn(&fx);
    cli.wait_for("[y/N]", 0, Duration::from_secs(30));
    let mark = cli.mark();
    cli.send("y\r");
    cli.wait_for(MAIN_PROMPT, mark, Duration::from_secs(30));

    let mark = cli.mark();
    cli.send("ask-me\r");
    cli.wait_for("选择部署方式", mark, Duration::from_secs(30));
    cli.wait_for("Choice/c/skip", mark, Duration::from_secs(10));
    let mark = cli.mark();
    let pressed = Instant::now();
    cli.send("\x03");
    cli.wait_for(MAIN_PROMPT, mark, Duration::from_secs(2));
    assert!(pressed.elapsed() < Duration::from_secs(2));
    assert!(
        cli.screen_since(mark).contains("Interrupted"),
        "{}",
        cli.screen_since(mark)
    );

    let mark = cli.mark();
    cli.send("ask-again\r");
    cli.wait_for("Choice/c/skip", mark, Duration::from_secs(30));
    cli.send("c 中文答案\r");
    cli.wait_for("ANSWER_RECORDED", mark, Duration::from_secs(30));

    let requests = captured_non_title_requests(&server);
    let answered = requests.last().expect("tool-result request");
    assert!(answered.contains("中文答案"), "{answered}");
}

/// 去掉 VT 控制序列（CSI、OSC、单字符 ESC），只留可见文本；ConPTY 用"光标右移 n 格"
/// 代替连续空格，这里还原成空格。
fn strip_vt(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        if ch != '\x1b' {
            out.push(ch);
            continue;
        }
        match chars.next() {
            Some('[') => {
                let mut params = String::new();
                for next in chars.by_ref() {
                    if ('@'..='~').contains(&next) {
                        if next == 'C' {
                            let count = params.parse().unwrap_or(1);
                            out.extend(std::iter::repeat_n(' ', count));
                        }
                        break;
                    }
                    params.push(next);
                }
            }
            Some(']') => {
                while let Some(next) = chars.next() {
                    if next == '\x07' || (next == '\x1b' && chars.next_if_eq(&'\\').is_some()) {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    out
}
