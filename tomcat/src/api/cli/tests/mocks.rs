//! # CLI 测试共享 fixture
//!
//! - `with_tomcat_config_in_home`：把进程 `HOME` 指向临时目录并预先写入
//!   `~/.tomcat/tomcat.config.toml`，以便 `tomcat workspace` 等子命令读写真实配置文件
//!   时不影响开发机 `~/.tomcat`。多个工作区相关用例共用一把全局锁串行化执行。
//! - `test_config`：返回一个把 `storage.work_dir` 指向给定路径的 `AppConfig`，
//!   供 session/plugin/audit 用例隔离会话目录。

use std::sync::Mutex;

use super::super::*;

static WORKSPACE_CLI_HOME_LOCK: Mutex<()> = Mutex::new(());

pub(super) fn with_tomcat_config_in_home<R>(
    work_dir: &std::path::Path,
    f: impl FnOnce() -> R,
) -> R {
    let _lock = WORKSPACE_CLI_HOME_LOCK.lock().unwrap();
    let home = tempfile::tempdir().unwrap();
    let tomcat = home.path().join(".tomcat");
    std::fs::create_dir_all(&tomcat).unwrap();
    let mut c = AppConfig::default();
    c.log.level = "info".to_string();
    c.storage.work_dir = Some(work_dir.to_str().unwrap().to_string());
    std::fs::write(
        tomcat.join("tomcat.config.toml"),
        toml::to_string_pretty(&c).unwrap(),
    )
    .unwrap();
    let prev = std::env::var("HOME").ok();
    std::env::set_var("HOME", home.path());
    let out = f();
    match prev {
        Some(h) => std::env::set_var("HOME", h),
        None => std::env::remove_var("HOME"),
    }
    out
}

pub(super) fn test_config(dir: &std::path::Path) -> AppConfig {
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(dir.to_str().unwrap().to_string());
    cfg
}
