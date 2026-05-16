mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serial_test::serial;
use tomcat::api::chat::preflight::start_git_preflight;
use tomcat::{
    wire, AppConfig, CheckpointKind, CheckpointRecordRequest, CheckpointStore, DefaultEventBus,
    EventBus, EventContext, SwitchingCheckpointStore,
};

fn worktree_fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let root = tempfile::tempdir().unwrap();
    let worktree = root.path().join("workspace");
    let trail = root.path().join("trail");
    fs::create_dir_all(&worktree).unwrap();
    fs::create_dir_all(&trail).unwrap();
    fs::write(worktree.join("note.txt"), "hello").unwrap();
    (root, worktree, trail)
}

fn git_preflight_config(home: &Path, auto_install_git: bool) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(home.join(".tomcat").to_string_lossy().to_string());
    cfg.preflight.auto_install_git = auto_install_git;
    cfg
}

fn record_once(store: &dyn CheckpointStore) -> tomcat::CheckpointId {
    store
        .record(CheckpointRecordRequest {
            session_id: "sess-git-preflight".to_string(),
            turn_id: "turn-1".to_string(),
            kind: CheckpointKind::TurnEnd,
            message_anchor: Some("msg-1".to_string()),
            notes: None,
        })
        .unwrap()
}

fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    assert!(predicate(), "condition not met within {:?}", timeout);
}

fn capture_git_preflight_statuses(
    bus: Arc<dyn EventBus>,
) -> (Arc<Mutex<Vec<String>>>, tomcat::EventListenerId) {
    let statuses = Arc::new(Mutex::new(Vec::new()));
    let statuses_for_cb = Arc::clone(&statuses);
    let id = bus.on(
        wire::WIRE_GIT_PREFLIGHT,
        Box::new(move |evt: EventContext| {
            if let Some(status) = evt.payload.get("status").and_then(|v| v.as_str()) {
                statuses_for_cb.lock().unwrap().push(status.to_string());
            }
            Ok(())
        }),
    );
    (statuses, id)
}

fn find_binary_on_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

#[test]
#[serial]
fn git_preflight_auto_install_disabled_keeps_noop() {
    common::setup_logging();

    let home = tempfile::tempdir().unwrap();
    let (_root, worktree, trail) = worktree_fixture();
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let (statuses, listener_id) = capture_git_preflight_statuses(Arc::clone(&bus));
    let switcher = Arc::new(SwitchingCheckpointStore::new(trail, worktree, false));
    let cfg = git_preflight_config(home.path(), false);

    start_git_preflight(&cfg, Arc::clone(&bus), Arc::clone(&switcher));
    std::thread::sleep(Duration::from_millis(150));

    assert!(
        statuses.lock().unwrap().is_empty(),
        "auto_install_git=false 时不应发 git preflight 事件"
    );
    assert!(
        !switcher.is_shadow(),
        "禁用 auto install 时 checkpoint store 应保持 Noop"
    );
    bus.off(listener_id);
}

#[cfg(unix)]
fn write_fake_shell_script(bin_dir: &Path, name: &str, body: &str) -> PathBuf {
    let script = bin_dir.join(name);
    fs::write(&script, format!("#!/bin/sh\n{body}\n")).unwrap();
    let mut perms = fs::metadata(&script).unwrap().permissions();
    use std::os::unix::fs::PermissionsExt;
    perms.set_mode(0o755);
    fs::set_permissions(&script, perms).unwrap();
    script
}

#[cfg(unix)]
fn write_fake_brew_script(bin_dir: &Path, body: &str) -> PathBuf {
    write_fake_shell_script(bin_dir, "brew", body)
}

#[cfg(unix)]
fn write_fake_nohup_script(bin_dir: &Path) -> PathBuf {
    write_fake_shell_script(bin_dir, "nohup", "exec \"$@\"")
}

#[cfg(unix)]
#[test]
#[serial]
fn git_preflight_detached_spawn_does_not_block() {
    common::setup_logging();

    let old_path = std::env::var_os("PATH");
    let old_home = std::env::var_os("HOME");

    let home = tempfile::tempdir().unwrap();
    let bin_dir = home.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let _brew = write_fake_brew_script(&bin_dir, "/bin/sleep 2\nexit 0");
    let _nohup = write_fake_nohup_script(&bin_dir);
    let (_root, worktree, trail) = worktree_fixture();
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let (statuses, listener_id) = capture_git_preflight_statuses(Arc::clone(&bus));
    let switcher = Arc::new(SwitchingCheckpointStore::new(trail, worktree, false));
    let cfg = git_preflight_config(home.path(), true);

    // SAFETY: serial_test 保证同进程串行改动 PATH/HOME；作用域末尾恢复。
    unsafe {
        std::env::set_var("PATH", &bin_dir);
        std::env::set_var("HOME", home.path());
    }

    let started = Instant::now();
    start_git_preflight(&cfg, Arc::clone(&bus), Arc::clone(&switcher));
    assert!(
        started.elapsed() < Duration::from_millis(80),
        "Unix detached git preflight 不应阻塞主线程"
    );

    wait_until(Duration::from_secs(2), || {
        let events = statuses.lock().unwrap();
        events.iter().any(|s| s == "start") && events.iter().any(|s| s == "detached")
    });
    assert!(
        !switcher.is_shadow(),
        "detached 安装刚启动时 store 仍应保持 Noop，等待后续真实 checkpoint 触发升级"
    );

    bus.off(listener_id);
    // SAFETY: 恢复环境变量。
    unsafe {
        match old_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}

#[cfg(unix)]
#[test]
#[serial]
fn git_preflight_install_enables_shadow_on_next_checkpoint() {
    common::setup_logging();

    let real_git = match find_binary_on_path("git") {
        Some(path) => path,
        None => return,
    };

    let old_path = std::env::var_os("PATH");
    let old_home = std::env::var_os("HOME");

    let home = tempfile::tempdir().unwrap();
    let bin_dir = home.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let git_link = bin_dir.join("git");
    let script_body = "/bin/sleep 2\nexit 0";
    let _brew = write_fake_brew_script(&bin_dir, script_body);
    let _nohup = write_fake_nohup_script(&bin_dir);
    let (_root, worktree, trail) = worktree_fixture();
    let bus: Arc<dyn EventBus> = Arc::new(DefaultEventBus::new());
    let (statuses, listener_id) = capture_git_preflight_statuses(Arc::clone(&bus));
    let switcher = Arc::new(SwitchingCheckpointStore::new(
        trail.clone(),
        worktree.clone(),
        false,
    ));
    let cfg = git_preflight_config(home.path(), true);

    // SAFETY: serial_test 保证同进程串行改动 PATH/HOME；作用域末尾恢复。
    unsafe {
        std::env::set_var("PATH", &bin_dir);
        std::env::set_var("HOME", home.path());
    }

    start_git_preflight(&cfg, Arc::clone(&bus), Arc::clone(&switcher));

    wait_until(Duration::from_secs(2), || {
        statuses.lock().unwrap().iter().any(|s| s == "detached")
    });
    std::os::unix::fs::symlink(&real_git, &git_link).unwrap();
    let mut activated_id = None;
    wait_until(Duration::from_secs(3), || {
        let id = record_once(switcher.as_ref());
        if id.is_null() {
            return false;
        }
        activated_id = Some(id);
        switcher.is_shadow()
    });
    let id = activated_id.expect("shadow activation id");
    assert!(
        !id.is_null(),
        "后台安装把真实 git 放回 PATH 后，下一次 checkpoint 操作应自动升级为 ShadowStore"
    );
    assert!(switcher.is_shadow(), "record() 后 store 应升级为 Shadow");

    bus.off(listener_id);
    // SAFETY: 恢复环境变量。
    unsafe {
        match old_path {
            Some(v) => std::env::set_var("PATH", v),
            None => std::env::remove_var("PATH"),
        }
        match old_home {
            Some(v) => std::env::set_var("HOME", v),
            None => std::env::remove_var("HOME"),
        }
    }
}
