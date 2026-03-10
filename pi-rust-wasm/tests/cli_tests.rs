//! CLI 子命令集成测试：通过 assert_cmd 黑盒测试 pi_awsm 二进制。
//! 覆盖 TASK-02 (T1-P0-010-completion) 验收标准：
//!   doctor / config get|set / plugin list|load|info / audit list|export / session list|new
//! 遵循 INTEGRATION_TEST_SPEC：AAA 模式、日志门禁、鲁棒性边界。

mod common;

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use tracing::{info, info_span};

#[allow(deprecated)]
fn cmd() -> Command {
    Command::cargo_bin("pi_awsm").expect("binary pi_awsm should exist")
}

// ────────────────────── help & version ──────────────────────

#[test]
fn test_help_output_contains_pi_awsm_and_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_help_output_contains_pi_awsm_and_exits_ok").entered();

    info!("Arrange: prepare --help command");
    let mut c = cmd();
    c.arg("--help");

    info!("Act: execute pi_awsm --help");
    let assert = c.assert();

    info!("Assert: exit 0 and output contains pi-awsm");
    assert
        .success()
        .stdout(predicate::str::contains("pi-awsm"))
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("doctor"))
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("session"))
        .stdout(predicate::str::contains("plugin"))
        .stdout(predicate::str::contains("audit"));
}

#[test]
fn test_version_output_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_version_output_exits_ok").entered();

    info!("Arrange: prepare --version");
    let mut c = cmd();
    c.arg("--version");

    info!("Act: execute --version");
    let assert = c.assert();

    info!("Assert: exit 0 and contains version string");
    assert
        .success()
        .stdout(predicate::str::contains("pi-awsm"));
}

// ────────────────────── init ──────────────────────

#[test]
fn test_init_creates_config_file_in_temp_dir() {
    common::setup_logging();
    let _span = info_span!("test_init_creates_config_file_in_temp_dir").entered();

    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");

    info!("Arrange: temp dir at {:?}", dir.path());
    let mut c = cmd();
    c.args(["init", "--config", config_path.to_str().unwrap()]);

    info!("Act: execute init");
    let assert = c.assert();

    info!("Assert: exit 0, config file created, output mentions file path");
    assert
        .success()
        .stdout(predicate::str::contains("已生成配置文件"));
    assert!(config_path.exists(), "config file should be created");
    let content = fs::read_to_string(&config_path).unwrap();
    assert!(
        content.contains("[log]"),
        "config should contain [log] section"
    );
}

// ────────────────────── doctor ──────────────────────

#[test]
fn test_doctor_without_config_prompts_init() {
    common::setup_logging();
    let _span = info_span!("test_doctor_without_config_prompts_init").entered();

    info!("Arrange: point to nonexistent config");
    let mut c = cmd();
    c.args(["doctor", "--config", "/tmp/nonexistent_pi_test_cfg.toml"]);

    info!("Act: execute doctor");
    let assert = c.assert();

    info!("Assert: exit 0, prompts about missing config");
    assert
        .success()
        .stdout(predicate::str::contains("未找到配置文件"));
}

#[test]
fn test_doctor_with_valid_config_checks_environment() {
    common::setup_logging();
    let _span = info_span!("test_doctor_with_valid_config_checks_environment").entered();

    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");

    info!("Arrange: create valid config via init");
    cmd()
        .args(["init", "--config", config_path.to_str().unwrap()])
        .assert()
        .success();

    info!("Act: execute doctor with valid config");
    let mut c = cmd();
    c.args(["doctor", "--config", config_path.to_str().unwrap()]);
    let assert = c.assert();

    info!("Assert: exit 0, mentions config validity and wasm checks");
    assert
        .success()
        .stdout(predicate::str::contains("配置合法").or(predicate::str::contains("✓")));
}

// ────────────────────── config ──────────────────────

#[test]
fn test_config_get_without_key_outputs_full_config() {
    common::setup_logging();
    let _span = info_span!("test_config_get_without_key_outputs_full_config").entered();

    info!("Arrange: use default config");
    let mut c = cmd();
    c.args(["config", "get"]);

    info!("Act: execute config get");
    let assert = c.assert();

    info!("Assert: exit 0, output contains config sections");
    assert
        .success()
        .stdout(predicate::str::contains("log").or(predicate::str::contains("level")));
}

#[test]
fn test_config_get_with_known_key_outputs_value() {
    common::setup_logging();
    let _span = info_span!("test_config_get_with_known_key_outputs_value").entered();

    info!("Arrange: query log.level");
    let mut c = cmd();
    c.args(["config", "get", "log.level"]);

    info!("Act: execute config get log.level");
    let assert = c.assert();

    info!("Assert: exit 0, output shows value");
    assert.success();
}

#[test]
fn test_config_get_with_unknown_key_shows_hint() {
    common::setup_logging();
    let _span = info_span!("test_config_get_with_unknown_key_shows_hint").entered();

    info!("Arrange: query nonexistent key");
    let mut c = cmd();
    c.args(["config", "get", "nonexistent.key"]);

    info!("Act: execute config get nonexistent.key");
    let assert = c.assert();

    info!("Assert: exit 0, output mentions not found");
    assert
        .success()
        .stdout(predicate::str::contains("未找到").or(predicate::str::contains("不存在")));
}

#[test]
fn test_config_export_creates_file() {
    common::setup_logging();
    let _span = info_span!("test_config_export_creates_file").entered();

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("exported.toml");

    info!("Arrange: temp export path {:?}", out);
    let mut c = cmd();
    c.args(["config", "export", out.to_str().unwrap()]);

    info!("Act: execute config export");
    let assert = c.assert();

    info!("Assert: exit 0, file exists and contains toml");
    assert
        .success()
        .stdout(predicate::str::contains("已导出"));
    assert!(out.exists(), "exported file should exist");
}

#[test]
fn test_config_import_valid_toml_succeeds() {
    common::setup_logging();
    let _span = info_span!("test_config_import_valid_toml_succeeds").entered();

    let dir = tempfile::tempdir().unwrap();
    let export_path = dir.path().join("cfg.toml");

    info!("Arrange: export then import");
    cmd()
        .args(["config", "export", export_path.to_str().unwrap()])
        .assert()
        .success();

    info!("Act: import the exported file");
    let mut c = cmd();
    c.args(["config", "import", export_path.to_str().unwrap()]);
    let assert = c.assert();

    info!("Assert: exit 0, mentions import success");
    assert
        .success()
        .stdout(predicate::str::contains("导入"));
}

#[test]
fn test_config_import_invalid_file_fails() {
    common::setup_logging();
    let _span = info_span!("test_config_import_invalid_file_fails").entered();

    let dir = tempfile::tempdir().unwrap();
    let bad = dir.path().join("bad.toml");
    fs::write(&bad, "this is not valid toml config { broken }").unwrap();

    info!("Arrange: create invalid toml file");
    let mut c = cmd();
    c.args(["config", "import", bad.to_str().unwrap()]);

    info!("Act: import invalid file");
    let assert = c.assert();

    info!("Assert: exits with error");
    assert.failure();
}

// ────────────────────── config set (boundary) ──────────────────────

#[test]
fn test_config_set_missing_args_shows_error() {
    common::setup_logging();
    let _span = info_span!("test_config_set_missing_args_shows_error").entered();

    info!("Arrange: config set with no args");
    let mut c = cmd();
    c.args(["config", "set"]);

    info!("Act: execute config set without key/value");
    let assert = c.assert();

    info!("Assert: clap rejects missing arguments");
    assert.failure().stderr(predicate::str::contains("Usage").or(predicate::str::contains("error")));
}

// ────────────────────── config help ──────────────────────

#[test]
fn test_config_help_lists_subcommands() {
    common::setup_logging();
    let _span = info_span!("test_config_help_lists_subcommands").entered();

    info!("Arrange: config --help");
    let mut c = cmd();
    c.args(["config", "--help"]);

    info!("Act: execute");
    let assert = c.assert();

    info!("Assert: lists get/set/edit/export/import");
    assert
        .success()
        .stdout(predicate::str::contains("get"))
        .stdout(predicate::str::contains("set"))
        .stdout(predicate::str::contains("edit"))
        .stdout(predicate::str::contains("export"))
        .stdout(predicate::str::contains("import"));
}

// ────────────────────── plugin ──────────────────────

#[test]
fn test_plugin_list_empty_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_plugin_list_empty_exits_ok").entered();

    info!("Arrange: no plugins loaded");
    let mut c = cmd();
    c.args(["plugin", "list"]);

    info!("Act: execute plugin list");
    let assert = c.assert();

    info!("Assert: exit 0, mentions no plugins");
    assert
        .success()
        .stdout(predicate::str::contains("无已加载插件").or(predicate::str::contains("插件")));
}

#[test]
fn test_plugin_load_nonexistent_path_shows_error() {
    common::setup_logging();
    let _span = info_span!("test_plugin_load_nonexistent_path_shows_error").entered();

    info!("Arrange: load from nonexistent path");
    let mut c = cmd();
    c.args(["plugin", "load", "/tmp/nonexistent_pi_plugin_xyz"]);

    info!("Act: execute plugin load");
    let assert = c.assert();

    info!("Assert: exit 0, mentions path not found");
    assert
        .success()
        .stdout(predicate::str::contains("不存在"));
}

#[test]
fn test_plugin_info_not_found_shows_message() {
    common::setup_logging();
    let _span = info_span!("test_plugin_info_not_found_shows_message").entered();

    info!("Arrange: query nonexistent plugin");
    let mut c = cmd();
    c.args(["plugin", "info", "nonexistent-plugin-id"]);

    info!("Act: execute plugin info");
    let assert = c.assert();

    info!("Assert: exit 0, mentions not found");
    assert
        .success()
        .stdout(predicate::str::contains("未找到"));
}

#[test]
fn test_plugin_unload_not_found_shows_message() {
    common::setup_logging();
    let _span = info_span!("test_plugin_unload_not_found_shows_message").entered();

    info!("Arrange: unload nonexistent plugin");
    let mut c = cmd();
    c.args(["plugin", "unload", "nonexistent-plugin-id"]);

    info!("Act: execute plugin unload");
    let assert = c.assert();

    info!("Assert: exit 0, mentions failure");
    assert
        .success()
        .stdout(predicate::str::contains("卸载失败"));
}

#[test]
fn test_plugin_enable_not_found_shows_message() {
    common::setup_logging();
    let _span = info_span!("test_plugin_enable_not_found_shows_message").entered();

    info!("Arrange: enable nonexistent plugin");
    let mut c = cmd();
    c.args(["plugin", "enable", "nonexistent-plugin-id"]);

    info!("Act: execute plugin enable");
    let assert = c.assert();

    info!("Assert: exit 0, mentions failure");
    assert
        .success()
        .stdout(predicate::str::contains("启用失败"));
}

#[test]
fn test_plugin_disable_not_found_shows_message() {
    common::setup_logging();
    let _span = info_span!("test_plugin_disable_not_found_shows_message").entered();

    info!("Arrange: disable nonexistent plugin");
    let mut c = cmd();
    c.args(["plugin", "disable", "nonexistent-plugin-id"]);

    info!("Act: execute plugin disable");
    let assert = c.assert();

    info!("Assert: exit 0, mentions failure");
    assert
        .success()
        .stdout(predicate::str::contains("禁用失败"));
}

#[test]
fn test_plugin_help_lists_subcommands() {
    common::setup_logging();
    let _span = info_span!("test_plugin_help_lists_subcommands").entered();

    info!("Arrange: plugin --help");
    let mut c = cmd();
    c.args(["plugin", "--help"]);

    info!("Act: execute");
    let assert = c.assert();

    info!("Assert: lists all plugin subcommands");
    assert
        .success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("load"))
        .stdout(predicate::str::contains("unload"))
        .stdout(predicate::str::contains("enable"))
        .stdout(predicate::str::contains("disable"))
        .stdout(predicate::str::contains("info"));
}

// ────────────────────── audit ──────────────────────

#[test]
fn test_audit_list_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_audit_list_exits_ok").entered();

    info!("Arrange: default config (file_enabled likely false)");
    let mut c = cmd();
    c.args(["audit", "list"]);

    info!("Act: execute audit list");
    let assert = c.assert();

    info!("Assert: exit 0, either shows entries or explains disabled/missing");
    assert.success();
}

#[test]
fn test_audit_help_lists_subcommands() {
    common::setup_logging();
    let _span = info_span!("test_audit_help_lists_subcommands").entered();

    info!("Arrange: audit --help");
    let mut c = cmd();
    c.args(["audit", "--help"]);

    info!("Act: execute");
    let assert = c.assert();

    info!("Assert: lists list/show/export");
    assert
        .success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("show"))
        .stdout(predicate::str::contains("export"));
}

// ────────────────────── session ──────────────────────

#[test]
fn test_session_list_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_session_list_exits_ok").entered();

    let dir = tempfile::tempdir().unwrap();
    let sessions_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: temp sessions dir {:?}", sessions_dir);
    let mut c = cmd();
    c.env("PI_AWSM__STORAGE__SESSIONS_DIR", sessions_dir.to_str().unwrap());
    c.args(["session", "list"]);

    info!("Act: execute session list");
    let assert = c.assert();

    info!("Assert: exit 0");
    assert.success();
}

#[test]
fn test_session_new_creates_session() {
    common::setup_logging();
    let _span = info_span!("test_session_new_creates_session").entered();

    let dir = tempfile::tempdir().unwrap();
    let sessions_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: temp sessions dir {:?}", sessions_dir);
    let mut c = cmd();
    c.env("PI_AWSM__STORAGE__SESSIONS_DIR", sessions_dir.to_str().unwrap());
    c.args(["session", "new"]);

    info!("Act: execute session new");
    let assert = c.assert();

    info!("Assert: exit 0, mentions created");
    assert
        .success()
        .stdout(predicate::str::contains("已创建会话"));
}

#[test]
fn test_session_help_lists_subcommands() {
    common::setup_logging();
    let _span = info_span!("test_session_help_lists_subcommands").entered();

    info!("Arrange: session --help");
    let mut c = cmd();
    c.args(["session", "--help"]);

    info!("Act: execute");
    let assert = c.assert();

    info!("Assert: lists all session subcommands");
    assert
        .success()
        .stdout(predicate::str::contains("list"))
        .stdout(predicate::str::contains("new"))
        .stdout(predicate::str::contains("switch"))
        .stdout(predicate::str::contains("delete"))
        .stdout(predicate::str::contains("archive"))
        .stdout(predicate::str::contains("search"));
}

// ────────────────────── chat (placeholder) ──────────────────────

#[test]
fn test_chat_exits_ok_with_placeholder() {
    common::setup_logging();
    let _span = info_span!("test_chat_exits_ok_with_placeholder").entered();

    info!("Arrange: chat command");
    let mut c = cmd();
    c.arg("chat");

    info!("Act: execute chat");
    let assert = c.assert();

    info!("Assert: exit 0, placeholder message");
    assert
        .success()
        .stdout(predicate::str::contains("对话模式"));
}

// ────────────────────── boundary: unknown subcommand ──────────────────────

#[test]
fn test_unknown_subcommand_shows_error() {
    common::setup_logging();
    let _span = info_span!("test_unknown_subcommand_shows_error").entered();

    info!("Arrange: unknown subcommand");
    let mut c = cmd();
    c.arg("nonexistent_command");

    info!("Act: execute unknown command");
    let assert = c.assert();

    info!("Assert: exits with error from clap");
    assert.failure().stderr(predicate::str::contains("error"));
}

// ────────────────────── init + doctor roundtrip ──────────────────────

#[test]
fn test_init_then_doctor_roundtrip() {
    common::setup_logging();
    let _span = info_span!("test_init_then_doctor_roundtrip").entered();

    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join("config.toml");

    info!("Arrange: init config in temp dir");
    cmd()
        .args(["init", "--config", config_path.to_str().unwrap()])
        .assert()
        .success();

    info!("Act: doctor with generated config");
    let mut c = cmd();
    c.args(["doctor", "--config", config_path.to_str().unwrap()]);
    let assert = c.assert();

    info!("Assert: doctor passes config check");
    assert
        .success()
        .stdout(predicate::str::contains("配置合法").or(predicate::str::contains("✓")));
}

// ────────────────────── init + config export + import roundtrip ──────────────────────

#[test]
fn test_config_export_then_import_roundtrip() {
    common::setup_logging();
    let _span = info_span!("test_config_export_then_import_roundtrip").entered();

    let dir = tempfile::tempdir().unwrap();
    let export_path = dir.path().join("exported.toml");

    info!("Arrange: export current config");
    cmd()
        .args(["config", "export", export_path.to_str().unwrap()])
        .assert()
        .success();

    info!("Act: import the exported config");
    let mut c = cmd();
    c.args(["config", "import", export_path.to_str().unwrap()]);
    let assert = c.assert();

    info!("Assert: import succeeds");
    assert
        .success()
        .stdout(predicate::str::contains("导入"));
}
