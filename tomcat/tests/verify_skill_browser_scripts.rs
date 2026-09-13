//! Opt-in real-browser check for the managed verify skill assets.
//!
//! Run explicitly with:
//! `cargo test --test verify_skill_browser_scripts -- --ignored`

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use serial_test::serial;
use tomcat::{core::skill::materialize_builtin_skills, AppConfig};

const HEALTHY_FIXTURE: &str = include_str!("fixtures/verify_skill_ui/healthy.html");
const ERROR_FIXTURE: &str = include_str!("fixtures/verify_skill_ui/console-error.html");

struct FixtureServer {
    base_url: String,
    stop: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,
}

impl FixtureServer {
    fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind fixture server");
        listener
            .set_nonblocking(true)
            .expect("make fixture server non-blocking");
        let address = listener.local_addr().expect("fixture address");
        let stop = Arc::new(AtomicBool::new(false));
        let stop_for_thread = stop.clone();
        let thread = thread::spawn(move || {
            while !stop_for_thread.load(Ordering::Acquire) {
                match listener.accept() {
                    Ok((stream, _)) => respond(stream),
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(error) => panic!("fixture server accept failed: {error}"),
                }
            }
        });
        Self {
            base_url: format!("http://{address}"),
            stop,
            thread: Some(thread),
        }
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn respond(mut stream: TcpStream) {
    stream
        .set_nonblocking(false)
        .expect("make accepted fixture socket blocking");
    let mut request = [0; 1024];
    let read = stream.read(&mut request).expect("read fixture request");
    let target = std::str::from_utf8(&request[..read])
        .expect("valid request")
        .split_whitespace()
        .nth(1)
        .unwrap_or("/");
    let body = match target {
        "/healthy.html" => HEALTHY_FIXTURE,
        "/console-error.html" => ERROR_FIXTURE,
        _ => "",
    };
    let status = if body.is_empty() {
        "404 Not Found"
    } else {
        "200 OK"
    };
    let response = format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    stream
        .write_all(response.as_bytes())
        .expect("write fixture response");
}

#[test]
fn verify_assets_default_to_agents_shots_and_document_custom_configuration() {
    let skill = include_str!("../assets/skills/verify/SKILL.md");
    let shot = include_str!("../assets/skills/verify/scripts/shot.mjs");

    assert!(skill.contains("workspace.project_resource_dir"));
    assert!(skill.contains("<workspace>/<project_resource_dir>/shots"));
    assert!(shot.contains("path.resolve(\".agents\", \"shots\")"));
    assert!(!shot.contains("path.resolve(\".tomcat\", \"shots\")"));
}

#[test]
#[ignore = "requires Node, network/browser bootstrap, and a local Chromium-compatible browser"]
#[serial]
fn bootstrap_then_shot_writes_three_artifacts_and_rejects_console_errors() {
    let temp = tempfile::tempdir().expect("temporary directory");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
    let skill_path = materialize_builtin_skills(&cfg).expect("materialize verify assets");
    let scripts_dir = skill_path
        .parent()
        .expect("skill directory")
        .join("scripts");
    let browser_path = temp.path().join("playwright");
    assert!(
        std::fs::read_to_string(&skill_path)
            .expect("read materialized verify skill")
            .contains("workspace.project_resource_dir"),
        "verify guidance must require the configured project resource directory"
    );

    let bootstrap = Command::new("node")
        .arg("bootstrap.mjs")
        .current_dir(&scripts_dir)
        .env("PLAYWRIGHT_BROWSERS_PATH", &browser_path)
        .output()
        .expect("run managed bootstrap");
    assert!(
        bootstrap.status.success(),
        "bootstrap failed:\n{}",
        String::from_utf8_lossy(&bootstrap.stderr)
    );

    let server = FixtureServer::start();
    let default_output_dir = temp.path().join(".agents").join("shots");
    let healthy_url = format!("{}/healthy.html", server.base_url);
    let healthy = Command::new("node")
        .args([
            scripts_dir
                .join("shot.mjs")
                .to_str()
                .expect("UTF-8 shot script"),
            healthy_url.as_str(),
            "--name",
            "healthy",
            "--viewport",
            "390x844",
        ])
        .current_dir(temp.path())
        .env("PLAYWRIGHT_BROWSERS_PATH", &browser_path)
        .output()
        .expect("capture healthy fixture");
    assert!(
        healthy.status.success(),
        "healthy shot failed:\n{}",
        String::from_utf8_lossy(&healthy.stderr)
    );
    for suffix in [".png", ".aria.txt", ".console.json"] {
        assert!(default_output_dir
            .join(format!("healthy{suffix}"))
            .is_file());
    }

    let output_dir = temp.path().join("shots");
    let console_error_url = format!("{}/console-error.html", server.base_url);
    let console_error = Command::new("node")
        .args([
            "shot.mjs",
            console_error_url.as_str(),
            "--out",
            output_dir.to_str().expect("UTF-8 output path"),
            "--name",
            "console-error",
        ])
        .current_dir(&scripts_dir)
        .env("PLAYWRIGHT_BROWSERS_PATH", &browser_path)
        .output()
        .expect("capture console-error fixture");
    assert!(
        !console_error.status.success(),
        "console errors must fail a shot"
    );
    assert!(
        std::fs::read_to_string(output_dir.join("console-error.console.json"))
            .expect("console report")
            .contains("intentional UI fixture error")
    );

    let node_modules = scripts_dir.join("node_modules");
    let unavailable_modules = scripts_dir.join("node_modules-unavailable");
    std::fs::rename(&node_modules, &unavailable_modules).expect("hide Playwright dependencies");
    let missing_dependency = Command::new("node")
        .args([
            "shot.mjs",
            healthy_url.as_str(),
            "--out",
            output_dir.to_str().expect("UTF-8 output path"),
            "--name",
            "missing-dependency",
        ])
        .current_dir(&scripts_dir)
        .env("PLAYWRIGHT_BROWSERS_PATH", &browser_path)
        .output()
        .expect("run shot without Playwright dependency");
    std::fs::rename(&unavailable_modules, &node_modules).expect("restore Playwright dependencies");
    assert!(!missing_dependency.status.success());
    assert!(
        String::from_utf8_lossy(&missing_dependency.stderr).contains("bootstrap.mjs"),
        "missing dependency must direct the user to bootstrap: {}",
        String::from_utf8_lossy(&missing_dependency.stderr)
    );
}
