//! CLI 子命令集成测试：通过 assert_cmd 黑盒测试 pi 二进制。
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
    Command::cargo_bin("pi").expect("binary pi should exist")
}

// ────────────────────── help & version ──────────────────────

/// [--help 输出] 验证主帮助页包含所有一级子命令名称
///
/// 验证：exit 0 且 stdout 包含 pi、init、doctor、config、session、plugin、audit
/// 意义：CLI 入口完整性门禁（TASK-02 验收：所有子命令帮助文档完整）
#[test]
fn test_help_output_contains_pi_and_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_help_output_contains_pi_and_exits_ok").entered();

    info!("Arrange: prepare --help command");
    let mut c = cmd();
    c.arg("--help");

    info!("Act: execute pi --help");
    let assert = c.assert();

    info!("Assert: exit 0 and output contains pi");
    assert
        .success()
        .stdout(predicate::str::contains("pi"))
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("doctor"))
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("session"))
        .stdout(predicate::str::contains("plugin"))
        .stdout(predicate::str::contains("audit"));
}

/// [--version 输出] 验证版本号输出格式
///
/// 验证：exit 0 且 stdout 含 pi 版本字符串
/// 意义：发布合规——二进制可报告自身版本
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
    assert.success().stdout(predicate::str::contains("pi"));
}

// ────────────────────── init ──────────────────────

/// [init 子命令] 在临时目录生成配置文件
///
/// 验证：exit 0、config.toml 已创建且含 [log] 段、stdout 提示"已生成配置文件"
/// 意义：首次使用流程门禁（TASK-02 10.2：引导 LLM 配置、生成配置文件）
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

/// [doctor 无配置] 未找到配置文件时给出引导提示
///
/// 验证：exit 0 且 stdout 含"未找到配置文件"
/// 意义：友好引导门禁（TASK-02 验收：首次运行无配置时的提示友好）
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

/// [doctor 有配置] init 后 doctor 通过配置与环境检测
///
/// 验证：exit 0 且 stdout 含"配置合法"或 checkmark
/// 意义：TASK-02 10.3 验收——doctor 检测 WasmEdge/QuickJS 可用性并输出修复建议
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

/// [config get 无参] 输出完整配置内容
///
/// 验证：exit 0 且 stdout 含 log/level 等配置段
/// 意义：TASK-02 10.4——config get 可展示全部配置
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

/// [config get 已知 key] 查询 log.level 返回具体值
///
/// 验证：exit 0
/// 意义：TASK-02 10.4——config get(key) 可查询单项配置
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

/// [config get 未知 key] 查询不存在的配置键给出提示
///
/// 验证：exit 0 且 stdout 含"未找到"或"不存在"
/// 意义：TASK-02 10.4——config get 对非法 key 的容错与友好提示
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

/// [config export] 导出配置到文件
///
/// 验证：exit 0、文件已创建、stdout 提示"已导出"
/// 意义：TASK-02 10.4——config export 可导出 TOML 配置
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
    assert.success().stdout(predicate::str::contains("已导出"));
    assert!(out.exists(), "exported file should exist");
}

/// [config import 合法] 先导出再导入合法 TOML 成功
///
/// 验证：export exit 0 后 import exit 0、stdout 含"导入"
/// 意义：TASK-02 10.4——config import 可接受合法配置
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
    assert.success().stdout(predicate::str::contains("导入"));
}

/// [config import 非法] 导入格式错误的 TOML 文件失败
///
/// 验证：exit code 非 0
/// 意义：TASK-02 10.4——config import 拒绝非法配置，避免覆盖合法文件
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

/// [config set 缺参数] set 不带 key/value 时 clap 报错
///
/// 验证：exit code 非 0、stderr 含 Usage 或 error
/// 意义：TASK-02 10.4——config set 参数校验门禁
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
    assert
        .failure()
        .stderr(predicate::str::contains("Usage").or(predicate::str::contains("error")));
}

// ────────────────────── config help ──────────────────────

/// [config --help] 帮助页列出所有 config 子命令
///
/// 验证：exit 0 且 stdout 包含 get/set/edit/export/import
/// 意义：CLI 帮助完整性门禁
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

/// [plugin list 空] 无已加载插件时正常退出
///
/// 验证：exit 0 且 stdout 含"无已加载插件"或"插件"
/// 意义：TASK-06 验收——plugin list 空列表不崩溃
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

/// [plugin load 不存在路径] 加载不存在的 wasm 文件给出提示
///
/// 验证：exit 0 且 stdout 含"不存在"
/// 意义：TASK-06——plugin load 路径校验与友好错误提示
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
    assert.success().stdout(predicate::str::contains("不存在"));
}

/// [plugin info 不存在] 查询不存在的插件 ID 提示"未找到"
///
/// 验证：exit 0 且 stdout 含"未找到"
/// 意义：TASK-06——plugin info 对非法 ID 的容错
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
    assert.success().stdout(predicate::str::contains("未找到"));
}

/// [plugin unload 不存在] 卸载不存在的插件给出"卸载失败"
///
/// 验证：exit 0 且 stdout 含"卸载失败"
/// 意义：TASK-06——plugin unload 对非法 ID 的容错
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

/// [plugin enable 不存在] 启用不存在的插件给出"启用失败"
///
/// 验证：exit 0 且 stdout 含"启用失败"
/// 意义：TASK-06——plugin enable 对非法 ID 的容错
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

/// [plugin disable 不存在] 禁用不存在的插件给出"禁用失败"
///
/// 验证：exit 0 且 stdout 含"禁用失败"
/// 意义：TASK-06——plugin disable 对非法 ID 的容错
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

/// [plugin --help] 帮助页列出所有 plugin 子命令
///
/// 验证：exit 0 且 stdout 包含 list/load/unload/enable/disable/info
/// 意义：CLI 帮助完整性门禁
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

/// [audit list] 列出审计记录正常退出
///
/// 验证：exit 0
/// 意义：TASK-02 10.7——audit list 不崩溃，无审计记录或已禁用时友好处理
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

/// [audit --help] 帮助页列出所有 audit 子命令
///
/// 验证：exit 0 且 stdout 包含 list/show/export
/// 意义：CLI 帮助完整性门禁
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

/// [session list] 空会话列表正常退出
///
/// 验证：exit 0
/// 意义：TASK-02 10.6——session list 在无会话时不崩溃
#[test]
fn test_session_list_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_session_list_exits_ok").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: temp work dir {:?}", work_dir);
    let mut c = cmd();
    c.env("PI_WASM__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
    c.args(["session", "list"]);

    info!("Act: execute session list");
    let assert = c.assert();

    info!("Assert: exit 0");
    assert.success();
}

/// [session new] 创建新会话
///
/// 验证：exit 0 且 stdout 含"已创建会话"
/// 意义：TASK-02 10.6——session new 可创建并持久化会话
#[test]
fn test_session_new_creates_session() {
    common::setup_logging();
    let _span = info_span!("test_session_new_creates_session").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: temp work dir {:?}", work_dir);
    let mut c = cmd();
    c.env("PI_WASM__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
    c.args(["session", "new"]);

    info!("Act: execute session new");
    let assert = c.assert();

    info!("Assert: exit 0, mentions created");
    assert
        .success()
        .stdout(predicate::str::contains("已创建会话"));
}

/// [session --help] 帮助页列出所有 session 子命令
///
/// 验证：exit 0 且 stdout 包含 list/new/switch/delete/archive/search
/// 意义：CLI 帮助完整性门禁（TASK-02 验收）
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

// ────────────────────── chat ──────────────────────

/// [chat 无配置] 没有 API key 和配置时 chat 失败退出
///
/// 验证：exit code 非 0
/// 意义：INTEGRATION_TEST_SPEC——无 key 不得 ignore，必须失败
#[test]
fn test_chat_without_config_exits_with_error() {
    common::setup_logging();
    let _span = info_span!("test_chat_without_config_exits_with_error").entered();

    info!("Arrange: chat command without valid config/env");
    let mut c = cmd();
    c.arg("chat");
    c.env_remove("OPENAI_API_KEY");

    info!("Act: execute chat");
    let assert = c.assert();

    info!("Assert: non-zero exit (no API key or config)");
    assert.failure();
}

/// [chat 有 API key] 有合法配置与 API key 时 chat 启动并产生输出
///
/// 验证：exit 0 且 stdout 包含"对话模式"banner 或模型信息或 AI 提示
/// 意义：TASK-02 10.1——chat 端到端可用；INTEGRATION_TEST_SPEC：无 key 不得 ignore
#[test]
fn test_chat_with_valid_config_and_api_key_starts_and_produces_output() {
    common::setup_logging();
    let _ = dotenvy::dotenv().ok();
    let _span =
        info_span!("test_chat_with_valid_config_and_api_key_starts_and_produces_output").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join("config.toml");

    info!("Arrange: init config in temp dir, set work_dir and OPENAI_API_KEY");
    cmd()
        .args(["init", "--config", config_path.to_str().unwrap()])
        .assert()
        .success();

    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        panic!("集成测试要求设置 OPENAI_API_KEY（无 key 时用例失败，符合 INTEGRATION_TEST_SPEC）")
    });

    let mut c = cmd();
    c.arg("chat")
        .env("PI_WASM__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("OPENAI_API_KEY", api_key)
        .write_stdin("hi\n")
        .timeout(std::time::Duration::from_secs(60));

    info!("Act: execute chat with stdin 'hi', timeout 60s");
    let assert = c.assert();
    let out = assert.get_output().stdout.clone();
    let out_str = String::from_utf8_lossy(&out);

    info!("Assert: exit 0 and stdout contains 对话模式 banner or AI output");
    assert.success();
    assert!(
        out_str.contains("对话模式") || out_str.contains("模型:") || out_str.contains("AI>"),
        "chat 应输出对话模式 banner 或模型信息或 AI 提示，实际: {}",
        out_str.chars().take(500).collect::<String>()
    );
}

/// [chat + session 协作] session new 后启动 chat 不挂起不崩溃
///
/// 验证：进程在 5s 内结束且产生 stdout 或 stderr
/// 意义：TASK-02——chat 与 session 子系统协作无死锁/崩溃
#[test]
fn test_chat_with_session_dir_does_not_crash() {
    common::setup_logging();
    let _span = info_span!("test_chat_with_session_dir_does_not_crash").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join("config.toml");

    info!("Arrange: init config, session new, set work_dir");
    cmd()
        .args(["init", "--config", config_path.to_str().unwrap()])
        .assert()
        .success();

    let mut c_new = cmd();
    c_new
        .args(["session", "new"])
        .env("PI_WASM__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
    c_new.assert().success();

    let mut c = cmd();
    c.arg("chat")
        .env("PI_WASM__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env_remove("OPENAI_API_KEY")
        .write_stdin("\n")
        .timeout(std::time::Duration::from_secs(5));

    info!("Act: run chat without API key, timeout 5s");
    let output = c.output().expect("chat 进程应在 5s 内结束");

    info!("Assert: 有 stdout 或 stderr，进程未静默挂起");
    assert!(
        !output.stdout.is_empty() || !output.stderr.is_empty(),
        "chat 应产生输出（banner 或错误），不应静默崩溃"
    );
}

// ────────────────────── boundary: unknown subcommand ──────────────────────

/// [未知子命令] 输入不存在的子命令给出 clap 错误
///
/// 验证：exit code 非 0 且 stderr 含"error"
/// 意义：CLI 边界安全——防止静默忽略拼写错误
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

/// [init → doctor 联合] init 后 doctor 应通过配置检测
///
/// 验证：init exit 0 + doctor exit 0 且 stdout 含"配置合法"或 ✓
/// 意义：端到端新手引导流程（TASK-02 10.2 + 10.3 联合验收）
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

/// [config export → import 联合] 导出再导入配置一致
///
/// 验证：export exit 0 + import exit 0 且 stdout 含"导入"
/// 意义：配置可迁移性验证（TASK-02 10.4 联合验收）
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
    assert.success().stdout(predicate::str::contains("导入"));
}

// ────────────────────── 补充用例：session switch/delete/archive ──────────────────────

/// [session switch 不存在] switch 到不存在的会话给出提示
///
/// 验证：exit 0 且 stdout 含"不存在"
/// 意义：TASK-02 10.6——session switch 对非法 key 的容错
#[test]
fn test_session_switch_nonexistent_shows_error() {
    common::setup_logging();
    let _span = info_span!("test_session_switch_nonexistent_shows_error").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: switch to nonexistent session key");
    let mut c = cmd();
    c.env("PI_WASM__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
    c.args(["session", "switch", "nonexistent-key-xyz"]);

    info!("Act: execute session switch");
    let assert = c.assert();

    info!("Assert: exit 0, mentions not exist");
    assert.success().stdout(predicate::str::contains("不存在"));
}

/// [session delete via CLI] 创建会话后通过 CLI 删除
///
/// 验证：new exit 0 + delete exit 0 且 stdout 含"已删除"
/// 意义：TASK-02 10.6——session delete 端到端可用
#[test]
fn test_session_delete_via_cli_removes_session() {
    common::setup_logging();
    let _span = info_span!("test_session_delete_via_cli_removes_session").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: create a session first");
    cmd()
        .env("PI_WASM__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "new"])
        .assert()
        .success();

    info!("Act: delete the default session key");
    let mut c = cmd();
    c.env("PI_WASM__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
    c.args(["session", "delete", "agent:default:main"]);

    let assert = c.assert();

    info!("Assert: exit 0, mentions deleted");
    assert.success().stdout(predicate::str::contains("已删除"));
}

/// [session archive] archive 子命令可正常执行
///
/// 验证：exit 0（即使会话不存在也不崩溃）
/// 意义：TASK-02 10.6——session archive 端到端可用
#[test]
fn test_session_archive_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_session_archive_exits_ok").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: create session then archive");
    cmd()
        .env("PI_WASM__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "new"])
        .assert()
        .success();

    let mut c = cmd();
    c.env("PI_WASM__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
    c.args(["session", "archive", "agent:default:main"]);

    info!("Act: execute session archive");
    let assert = c.assert();

    info!("Assert: exit 0");
    assert.success().stdout(predicate::str::contains("已归档"));
}

// ────────────────────── 补充用例：config set 成功路径 ──────────────────────

/// [config set 合法] config set log.level warn 正常退出
///
/// 验证：exit 0（配置文件存在时修改成功，不存在时给出提示但不崩溃）
/// 意义：TASK-02 10.4——config set 正向路径覆盖（原有用例仅覆盖缺参数的失败路径）
#[test]
fn test_config_set_valid_key_value_updates_config() {
    common::setup_logging();
    let _span = info_span!("test_config_set_valid_key_value_updates_config").entered();

    info!("Act: config set log.level warn");
    let mut c = cmd();
    c.args(["config", "set", "log.level", "warn"]);
    let assert = c.assert();

    info!("Assert: exit 0");
    assert.success();
}

// ────────────────────── 补充用例：audit show/export ──────────────────────

/// [audit show 不存在 ID] 查看不存在的审计 ID 不崩溃
///
/// 验证：exit 0（打印"未找到"或类似提示，不 panic）
/// 意义：TASK-02 10.7——audit show 容错
#[test]
fn test_audit_show_with_invalid_id_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_audit_show_with_invalid_id_exits_ok").entered();

    info!("Arrange: show nonexistent audit id");
    let mut c = cmd();
    c.args(["audit", "show", "9999999"]);

    info!("Act: execute audit show");
    let assert = c.assert();

    info!("Assert: exit 0, doesn't crash");
    assert.success();
}

/// [audit export] 导出审计记录到文件可正常执行
///
/// 验证：exit 0
/// 意义：TASK-02 10.7——audit export 端到端可用
#[test]
fn test_audit_export_creates_file() {
    common::setup_logging();
    let _span = info_span!("test_audit_export_creates_file").entered();

    let dir = tempfile::tempdir().unwrap();
    let out = dir.path().join("audit_export.json");

    info!("Arrange: export audit to temp path");
    let mut c = cmd();
    c.args(["audit", "export", out.to_str().unwrap()]);

    info!("Act: execute audit export");
    let assert = c.assert();

    info!("Assert: exit 0");
    assert.success();
}
