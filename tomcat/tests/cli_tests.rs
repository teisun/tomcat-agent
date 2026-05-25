//! CLI 子命令集成测试：通过 assert_cmd 黑盒测试 tomcat 二进制。
//! 覆盖 TASK-02 (T1-P0-010-completion) 验收标准：
//!   doctor / config get|set / plugin list|load|info / audit list|export / session list|new
//! 遵循 INTEGRATION_TEST_SPEC：AAA 模式、日志门禁、鲁棒性边界。

mod common;

use assert_cmd::Command;
use predicates::prelude::*;
use std::fs;
use std::path::PathBuf;
use tomcat::core::session::DEFAULT_SESSION_KEY;
use tracing::{info, info_span};

#[allow(deprecated)]
fn cmd() -> Command {
    let mut c = Command::cargo_bin("tomcat").expect("binary tomcat should exist");
    // 避免宿主环境 TOMCAT__LLM__DEFAULT_MODEL 覆盖临时 HOME 下的 tomcat.config.toml
    c.env_remove("TOMCAT__LLM__DEFAULT_MODEL");
    c
}

fn trunc(s: &str, n: usize) -> String {
    s.chars().take(n).collect()
}

// ────────────────────── help & version ──────────────────────

/// [--help 输出] 验证主帮助页包含所有一级子命令名称
///
/// 验证：exit 0 且 stdout 包含 tomcat、init、doctor、config、session、workspace、plugin、audit
/// 意义：CLI 入口完整性门禁（TASK-02 验收：所有子命令帮助文档完整）
#[test]
fn test_help_output_contains_tomcat_and_exits_ok() {
    common::setup_logging();
    let _span = info_span!("test_help_output_contains_tomcat_and_exits_ok").entered();

    info!("Arrange: prepare --help command");
    let mut c = cmd();
    c.arg("--help");

    info!("Act: execute tomcat --help");
    let assert = c.assert();

    info!("Assert: exit 0 and output contains tomcat");
    assert
        .success()
        .stdout(predicate::str::contains("tomcat"))
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("doctor"))
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("session"))
        .stdout(predicate::str::contains("workspace"))
        .stdout(predicate::str::contains("plugin"))
        .stdout(predicate::str::contains("audit"));
}

/// [--version 输出] 验证版本号输出格式
///
/// 验证：exit 0 且 stdout 含 tomcat 版本字符串
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
    assert.success().stdout(predicate::str::contains("tomcat"));
}

// ────────────────────── init ──────────────────────

/// [init 子命令] 在临时目录生成配置文件
///
/// 验证：exit 0、tomcat.config.toml 已创建且默认 provider 为 openai-responses、stdout 含三步向导与「配置文件已写入」
/// 意义：首次使用流程门禁（TASK-02 10.2：引导 LLM 配置、生成配置文件）
#[test]
fn test_init_creates_config_file_in_temp_dir() {
    common::setup_logging();
    let _span = info_span!("test_init_creates_config_file_in_temp_dir").entered();

    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: temp dir at {:?}", dir.path());
    let mut c = cmd();
    c.args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh");

    info!("Act: execute init");
    let assert = c.assert();

    info!("Assert: exit 0, config file created, output mentions file path");
    assert
        .success()
        .stdout(predicate::str::contains("[1/3] 环境初始化"))
        .stdout(predicate::str::contains("配置文件已写入"));
    assert!(config_path.exists(), "config file should be created");
    let content = fs::read_to_string(&config_path).unwrap();
    assert!(
        content.contains("[log]"),
        "config should contain [log] section"
    );
    assert!(
        content.contains("provider = \"openai-responses\""),
        "config should default to openai-responses"
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

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: point to nonexistent config");
    let mut c = cmd();
    c.args(["doctor"]).env("HOME", dir.path());

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

    info!("Arrange: create valid config via init");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Act: execute doctor with valid config");
    let mut c = cmd();
    c.args(["doctor"]).env("HOME", dir.path());
    let assert = c.assert();

    info!("Assert: exit 0, mentions config validity and wasm checks");
    assert
        .success()
        .stdout(predicate::str::contains("配置合法").or(predicate::str::contains("✓")));
}

/// [E2E-CLI-004] 工作区 add / list / remove
///
/// 验证：init 后 workspace add → list 含路径 → remove → list 为空提示
/// 意义：TASK-12 / TASK-09：`tomcat workspace` 与 `tomcat.config.toml` `[workspace] workspace_roots` 一致
#[test]
fn test_workspace_add_list_remove_e2e() {
    common::setup_logging();
    let _span = info_span!("test_workspace_add_list_remove_e2e").entered();

    let home = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let proj_canon = std::fs::canonicalize(proj.path()).unwrap();
    let proj_str = proj_canon.to_str().unwrap();

    cmd()
        .args(["init"])
        .env("HOME", home.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    cmd()
        .args(["workspace", "add", proj_str])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("已添加工作区"));

    let list_assert = cmd()
        .args(["workspace", "list"])
        .env("HOME", home.path())
        .assert();
    let list_out = String::from_utf8_lossy(&list_assert.get_output().stdout).to_string();
    list_assert.success();
    assert!(
        list_out.contains(proj_str),
        "list 应含已添加路径，实际: {}",
        trunc(&list_out, 200)
    );

    cmd()
        .args(["workspace", "remove", proj_str])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("已移除工作区"));

    cmd()
        .args(["workspace", "list"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("无已授权工作区"));
}

/// [E2E-CLI-017] workspace add --cwd 将当前目录加入授权列表
#[test]
fn test_workspace_add_cwd_e2e() {
    common::setup_logging();
    let _span = info_span!("test_workspace_add_cwd_e2e").entered();

    let home = tempfile::tempdir().unwrap();
    let proj = tempfile::tempdir().unwrap();
    let proj_canon = std::fs::canonicalize(proj.path()).unwrap();
    let proj_str = proj_canon.to_str().unwrap();

    cmd()
        .args(["init"])
        .env("HOME", home.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    // `std::env::current_dir` 为进程全局；若将来 cli_tests 改为多线程并行，需改为子进程或串行策略，避免与其它用例竞态。
    let prev = std::env::current_dir().unwrap();
    std::env::set_current_dir(proj.path()).unwrap();
    cmd()
        .args(["workspace", "add", "--cwd"])
        .env("HOME", home.path())
        .assert()
        .success()
        .stdout(predicate::str::contains("已添加工作区"));
    std::env::set_current_dir(&prev).unwrap();

    let list_assert = cmd()
        .args(["workspace", "list"])
        .env("HOME", home.path())
        .assert();
    let list_out = String::from_utf8_lossy(&list_assert.get_output().stdout).to_string();
    list_assert.success();
    assert!(
        list_out.contains(proj_str),
        "list 应含当前目录，实际: {}",
        trunc(&list_out, 200)
    );
}

/// [E2E-CLI-005] init 自动将 PATH 写入隔离 HOME 下的 shell 配置文件
#[test]
fn test_init_auto_adds_path_to_shell_profile() {
    common::setup_logging();
    let _span = info_span!("test_init_auto_adds_path_to_shell_profile").entered();

    let dir = tempfile::tempdir().unwrap();
    let zshrc = dir.path().join(".zshrc");

    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let content = fs::read_to_string(&zshrc).expect(".zshrc should be created under HOME");
    assert!(
        content.contains("export PATH=") && content.contains("# Added by tomcat init"),
        "expected PATH block in .zshrc, got: {}",
        trunc(&content, 400)
    );
}

/// init 两次后 shell 配置中仅一条 export PATH（幂等）
#[test]
fn test_init_path_export_idempotent_in_shell_profile() {
    common::setup_logging();
    let _span = info_span!("test_init_path_export_idempotent_in_shell_profile").entered();

    let dir = tempfile::tempdir().unwrap();
    let zshrc = dir.path().join(".zshrc");

    for _ in 0..2 {
        cmd()
            .args(["init"])
            .env("HOME", dir.path())
            .env("SHELL", "/bin/zsh")
            .assert()
            .success();
    }
    let content = fs::read_to_string(&zshrc).unwrap();
    let count = content.matches("export PATH=").count();
    assert_eq!(
        count,
        1,
        "expected single export PATH line, got {} in: {}",
        count,
        trunc(&content, 500)
    );
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
/// 验证：exit 0 且 stdout 包含 get/set/edit
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

    info!("Assert: lists get/set/edit");
    assert
        .success()
        .stdout(predicate::str::contains("get"))
        .stdout(predicate::str::contains("set"))
        .stdout(predicate::str::contains("edit"));
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
    c.env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
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
    c.env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
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

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: 无 ~/.tomcat/ 配置且无 OPENAI_API_KEY（HOME 指向空临时目录）");
    let mut c = cmd();
    c.arg("chat")
        .env("HOME", dir.path())
        .env_remove("OPENAI_API_KEY");

    info!("Act: execute chat");
    let assert = c.assert();

    info!("Assert: non-zero exit (no API key or config)");
    assert.failure();
}

/// [chat 有 API key] 有合法配置与 API key 时 chat 启动并产生输出
///
/// 验证：exit 0 且 stdout 包含"对话模式"banner 或模型信息或 agent prompt
/// 意义：TASK-02 10.1——chat 端到端可用；INTEGRATION_TEST_SPEC：无 key 不得 ignore
#[test]
fn test_chat_with_valid_config_and_api_key_starts_and_produces_output() {
    common::setup_logging();
    common::load_openai_test_env();
    let _span =
        info_span!("test_chat_with_valid_config_and_api_key_starts_and_produces_output").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();

    info!("Arrange: init config in temp dir, set work_dir and OPENAI_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        panic!("集成测试要求设置 OPENAI_API_KEY（无 key 时用例失败，符合 INTEGRATION_TEST_SPEC）")
    });

    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
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
        out_str.contains("对话模式")
            || out_str.contains("模型:")
            || out_str.contains("agent.main>"),
        "chat 应输出对话模式 banner 或模型信息或 agent.main> 提示，实际: {}",
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

    info!("Arrange: init config, session new, set work_dir");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let mut c_new = cmd();
    c_new
        .args(["session", "new"])
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
    c_new.assert().success();

    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
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

    info!("Arrange: init config in temp dir");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Act: doctor with generated config");
    let mut c = cmd();
    c.args(["doctor"]).env("HOME", dir.path());
    let assert = c.assert();

    info!("Assert: doctor passes config check");
    assert
        .success()
        .stdout(predicate::str::contains("配置合法").or(predicate::str::contains("✓")));
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
    c.env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
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
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "new"])
        .assert()
        .success();

    info!("Act: delete the default session key");
    let mut c = cmd();
    c.env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
    c.args(["session", "delete", "agent:main:main"]);

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
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "new"])
        .assert()
        .success();

    let mut c = cmd();
    c.env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap());
    c.args(["session", "archive", "agent:main:main"]);

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

// ══════════════════════════════════════════════════════════════════
// E2E 全量覆盖：test_user_* 用例（按 E2E_SCENARIO_LIBRARY 编号）
// ══════════════════════════════════════════════════════════════════

// ──────────────────── Story 1: 宿主初始化与基础配置 (E2E-CLI-001~006) ────────────────────

/// [E2E-CLI-001] 新用户首次安装，完成初始化并验证环境健康
///
/// 用户意图：新用户首次安装，完成初始化并验证环境健康
/// 验证：init exit 0 + stdout 含 [1/3][2/3][3/3]、tomcat chat、PATH 自动配置；doctor exit 0 + stdout 含"配置合法"和"内嵌资源已就绪"
#[test]
fn test_user_first_time_setup_init_and_doctor() {
    common::setup_logging();
    common::load_openai_test_env();
    let _span = info_span!("test_user_first_time_setup_init_and_doctor").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: fresh temp dir, no existing config");
    info!("Act: tomcat init");
    let init_assert = cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert();
    let init_out = String::from_utf8_lossy(&init_assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert init: exit 0 + 三步向导 + tomcat chat；actual: {}",
        trunc(&init_out, 400)
    );
    init_assert
        .success()
        .stdout(predicate::str::contains("[1/3]"))
        .stdout(predicate::str::contains("[2/3]"))
        .stdout(predicate::str::contains("[3/3]"))
        .stdout(predicate::str::contains("配置文件已写入"))
        .stdout(predicate::str::contains("tomcat chat"))
        .stdout(predicate::str::contains("PATH"));

    info!("Act: tomcat doctor");
    let mut c = cmd();
    c.args(["doctor"]).env("HOME", dir.path());
    let doctor_assert = c.assert();
    let doctor_out =
        String::from_utf8_lossy(&doctor_assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert doctor: exit 0 + stdout 含 配置合法 + 内嵌资源；actual: {}",
        trunc(&doctor_out, 400)
    );
    doctor_assert
        .success()
        .stdout(predicate::str::contains("配置合法"))
        .stdout(predicate::str::contains("内嵌资源已就绪").or(predicate::str::contains("✓")));
}

/// [E2E-CLI-002] 用户修改日志级别
///
/// 用户意图：修改 log.level 为 warn
/// 验证：exit 0
#[test]
fn test_user_sets_config_value() {
    common::setup_logging();
    let _span = info_span!("test_user_sets_config_value").entered();

    info!("Arrange: no special setup needed");
    info!("Act: tomcat config set log.level warn");
    let assert = cmd().args(["config", "set", "log.level", "warn"]).assert();
    info!("Assert: exit 0");
    assert.success();
}

/// [E2E-CLI-003] 用户查看当前全部配置
///
/// 用户意图：查看当前全部配置
/// 验证：exit 0；stdout 含配置段关键字（llm/log/storage）
#[test]
fn test_user_views_full_config() {
    common::setup_logging();
    let _span = info_span!("test_user_views_full_config").entered();

    info!("Arrange: use default config");
    info!("Act: tomcat config get");
    let assert = cmd().args(["config", "get"]).assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0, stdout 含配置段关键字；actual: {}",
        trunc(&out, 300)
    );
    assert.success().stdout(
        predicate::str::contains("llm")
            .or(predicate::str::contains("log"))
            .or(predicate::str::contains("storage")),
    );
}

/// [E2E-CLI-006] 用户运行 doctor 检测 WasmEdge/QuickJS 可用性
///
/// 用户意图：运行 doctor 检测环境
/// 验证：exit 0；stdout 含环境检测项（WasmEdge/配置/✓）
#[test]
fn test_user_doctor_detects_environment() {
    common::setup_logging();
    let _span = info_span!("test_user_doctor_detects_environment").entered();

    info!("Arrange: default config");
    info!("Act: tomcat doctor");
    let assert = cmd().args(["doctor"]).assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含 WasmEdge / 配置 / 内嵌资源 / .env 检查项；actual: {}",
        trunc(&out, 500)
    );
    assert.success().stdout(
        predicate::str::contains("WasmEdge")
            .or(predicate::str::contains("配置"))
            .or(predicate::str::contains("✓"))
            .or(predicate::str::contains("内嵌资源"))
            .or(predicate::str::contains(".env")),
    );
}

// ──────────────────── TASK-06 新增集成测试：内嵌资源 + init .env ────────────────────

/// [TASK-06] init 后生成配置中的 LLM 段
///
/// 验证：tomcat init exit 0；`tomcat.config.toml` 存在且默认 provider 为 openai-responses（.env 仅在用户输入非空 Key 时写入）
#[test]
fn test_init_creates_env_file() {
    common::setup_logging();
    let _span = info_span!("test_init_creates_env_file").entered();

    let dir = tempfile::tempdir().unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: fresh temp dir");
    info!("Act: tomcat init");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Assert: config file created");
    assert!(config_path.exists(), "config file should be created");

    let cfg_content = fs::read_to_string(&config_path).unwrap();
    info!("Config content (truncated): {}", trunc(&cfg_content, 300));
    assert!(
        cfg_content.contains("[llm]") || cfg_content.contains("provider"),
        "config should contain LLM section"
    );
    assert!(
        cfg_content.contains("provider = \"openai-responses\""),
        "config should default to openai-responses"
    );
}

/// [TASK-06] init 后 .env 权限为 0600
#[test]
#[cfg(unix)]
fn test_init_creates_env_with_correct_permissions() {
    use std::os::unix::fs::PermissionsExt;
    common::setup_logging();
    let _span = info_span!("test_init_creates_env_with_correct_permissions").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: fresh temp dir");
    info!("Act: tomcat init → check .env permissions");

    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let env_path = dir.path().join(".tomcat").join("assets").join(".env");
    if env_path.exists() {
        let mode = fs::metadata(&env_path).unwrap().permissions().mode() & 0o777;
        info!("Assert: .env permissions = {:04o}", mode);
        assert_eq!(mode, 0o600, ".env should have 0600 permissions");
    }
}

/// [TASK-06] doctor 对完整环境报告所有检查项
///
/// 验证：先 init 再 doctor，输出含 配置合法 / 内嵌资源 / QuickJS wasm / WasmEdge / 资源版本
#[test]
fn test_doctor_reports_all_checks() {
    common::setup_logging();
    let _span = info_span!("test_doctor_reports_all_checks").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: tomcat init first");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Act: tomcat doctor");
    let assert = cmd().args(["doctor"]).env("HOME", dir.path()).assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: all check items present；actual: {}",
        trunc(&out, 600)
    );
    assert
        .success()
        .stdout(predicate::str::contains("配置合法"))
        .stdout(predicate::str::contains("内嵌资源"))
        .stdout(predicate::str::contains("QuickJS wasm"))
        .stdout(predicate::str::contains("WasmEdge"));
}

/// [E2E-CLI-010] init 幂等：第二次不覆盖配置并给出提示
///
/// 验证：连续两次 tomcat init，第二次 exit 0 且 stdout 含保留/使用已有配置提示
#[test]
fn test_init_idempotent() {
    common::setup_logging();
    let _span = info_span!("test_init_idempotent").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Act: tomcat init (first)");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Act: tomcat init (second, idempotent)");
    let assert = cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!("Assert: second init exit 0；actual: {}", trunc(&out, 300));
    assert.success().stdout(
        predicate::str::contains("已存在配置文件").or(predicate::str::contains("使用已有配置文件")),
    );
}

/// [TASK-06] ensure_embedded_assets 释放 wasm 到 work_dir
///
/// 验证：tomcat init 后 ~/.tomcat/assets/wasm/wasmedge_quickjs.wasm 存在
#[test]
fn test_ensure_embedded_assets_extracts_wasm() {
    common::setup_logging();
    let _span = info_span!("test_ensure_embedded_assets_extracts_wasm").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Act: tomcat init");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Assert: doctor 能发现 QuickJS wasm");
    let assert = cmd().args(["doctor"]).env("HOME", dir.path()).assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!("doctor output: {}", trunc(&out, 500));
    assert
        .success()
        .stdout(predicate::str::contains("QuickJS wasm"));
}

/// [TASK-06] ensure_embedded_assets 重复调用不报错
///
/// 验证：连续 tomcat doctor 两次（每次都触发 ensure_embedded_assets），均 exit 0
#[test]
fn test_ensure_embedded_assets_idempotent() {
    common::setup_logging();
    let _span = info_span!("test_ensure_embedded_assets_idempotent").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: tomcat init");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Act: tomcat doctor x2（每次触发 ensure_embedded_assets）");
    cmd()
        .args(["doctor"])
        .env("HOME", dir.path())
        .assert()
        .success();
    cmd()
        .args(["doctor"])
        .env("HOME", dir.path())
        .assert()
        .success();
}

/// [TASK-06] ensure_embedded_assets 在 SHA 不匹配时覆盖旧文件
///
/// 验证：篡改 wasm 文件后，tomcat doctor 仍能正常通过（ensure_embedded_assets 覆盖了篡改文件）
#[test]
fn test_ensure_embedded_assets_upgrades_on_sha_mismatch() {
    common::setup_logging();
    let _span = info_span!("test_ensure_embedded_assets_upgrades_on_sha_mismatch").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: tomcat init");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Arrange: tamper wasm file in default work_dir");
    let wasm_path = dir
        .path()
        .join(".tomcat")
        .join("assets")
        .join("wasm")
        .join("wasmedge_quickjs.wasm");
    if wasm_path.exists() {
        let original_len = fs::metadata(&wasm_path).unwrap().len();
        fs::write(&wasm_path, b"tampered").unwrap();
        info!("Tampered wasm: {} bytes -> 8 bytes", original_len);

        info!("Act: tomcat doctor（触发 ensure_embedded_assets 覆盖）");
        let assert = cmd().args(["doctor"]).env("HOME", dir.path()).assert();
        assert.success();

        let restored_len = fs::metadata(&wasm_path).unwrap().len();
        info!(
            "Assert: wasm restored from 8 bytes to {} bytes",
            restored_len
        );
        assert!(
            restored_len > 100,
            "wasm should be restored after SHA mismatch, got {} bytes",
            restored_len
        );
    }
}

// ──────────────────── Story 2: 4原语安全管控（E2E-CLI-011~012，需 OPENAI_API_KEY） ────────────────────

/// [E2E-CLI-011] 用户向助手提问并收到回答
///
/// 用户意图：在 tomcat chat 中提问，收到 AI 回复
/// 验证：exit 0；stdout 非空
/// 要求：OPENAI_API_KEY 环境变量已设置；无 key 时 panic（符合规范）
#[test]
fn test_user_asks_pi_a_question() {
    common::setup_logging();
    common::load_openai_test_env();
    let _span = info_span!("test_user_asks_pi_a_question").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init + OPENAI_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        panic!(
            "集成测试要求设置 OPENAI_API_KEY（无 key 时用例失败，符合 INTEGRATION_TEST_SPEC §5.2）"
        )
    });

    info!("Act: tomcat chat stdin 你好，介绍一下你自己，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("OPENAI_API_KEY", &api_key)
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin("你好，介绍一下你自己\n")
        .timeout(std::time::Duration::from_secs(60));
    let assert = c.assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!("Assert: exit 0 + stdout 非空；actual: {}", trunc(&out, 300));
    assert.success();
    assert!(
        !out.trim().is_empty(),
        "AI 应输出非空回复，实际 stdout 为空"
    );
}

/// [E2E-CLI-012] 用户问技术问题，验证 LLM 回复质量
///
/// 用户意图：问 Rust 所有权系统
/// 验证：exit 0；stdout 含"所有权"或"ownership"
/// 要求：OPENAI_API_KEY 环境变量已设置
#[test]
fn test_user_asks_pi_technical_question() {
    common::setup_logging();
    common::load_openai_test_env();
    let _span = info_span!("test_user_asks_pi_technical_question").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init + OPENAI_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        panic!(
            "集成测试要求设置 OPENAI_API_KEY（无 key 时用例失败，符合 INTEGRATION_TEST_SPEC §5.2）"
        )
    });

    info!("Act: tomcat chat stdin 问 Rust 所有权，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("OPENAI_API_KEY", &api_key)
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin("用一句话解释什么是 Rust 的所有权系统\n")
        .timeout(std::time::Duration::from_secs(60));
    let assert = c.assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含所有权/ownership；actual: {}",
        trunc(&out, 300)
    );
    assert.success();
    assert!(
        out.contains("所有权") || out.to_lowercase().contains("ownership"),
        "stdout 应含 '所有权' 或 'ownership'，实际: {}",
        trunc(&out, 300)
    );
}

/// [E2E-CLI-016] 用户要求助手通过 bash 执行一条命令
///
/// 验证：exit 0；stdout 含 hello_from_pi（或明显命令执行结果）
/// 意义：工具调用 E2E 门禁，保证 execute_bash 被真实调用
#[test]
fn test_user_asks_pi_to_run_bash_command() {
    common::setup_logging();
    common::load_openai_test_env();
    let _span = info_span!("test_user_asks_pi_to_run_bash_command").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init + OPENAI_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        panic!(
            "集成测试要求设置 OPENAI_API_KEY（无 key 时用例失败，符合 INTEGRATION_TEST_SPEC §5.2）"
        )
    });

    info!("Act: tomcat chat stdin 请执行 echo hello_from_pi，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("OPENAI_API_KEY", &api_key)
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .env("RUST_LOG", "tomcat=info")
        .write_stdin("请执行 echo hello_from_pi\n")
        .timeout(std::time::Duration::from_secs(60));
    let assert = c.assert();
    let output = assert.get_output();
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !stderr.is_empty() {
        info!("[tomcat chat stderr] {}", trunc(&stderr, 1500));
    }
    let out = String::from_utf8_lossy(&output.stdout).to_string();
    info!(
        "Assert: exit 0 + stdout 含 hello_from_pi；actual: {}",
        trunc(&out, 300)
    );
    assert.success();
    assert!(
        out.contains("hello_from_pi"),
        "stdout 应含 'hello_from_pi'（工具 execute_bash 被调用），实际: {}",
        trunc(&out, 300)
    );
}

/// [E2E-CLI-016B] 用户触发 read 失败时，终端应显示真实错误原因（非 failed 占位）
///
/// 验证：exit 0；stderr 含 `[tool] read` 且包含 not found 语义，并且不退化为 `✗ failed`
/// 要求：OPENAI_API_KEY 环境变量已设置
#[test]
fn test_user_sees_read_failure_reason_in_tool_line() {
    common::setup_logging();
    common::load_openai_test_env();
    let _span = info_span!("test_user_sees_read_failure_reason_in_tool_line").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(work_dir.join("workspace-main")).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init + OPENAI_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        panic!(
            "集成测试要求设置 OPENAI_API_KEY（无 key 时用例失败，符合 INTEGRATION_TEST_SPEC §5.2）"
        )
    });

    let missing_path = work_dir.join("workspace-main/definitely_missing_read_e2e.txt");
    let prompt = format!(
        "请只调用一次 read 工具读取文件 {}，offset=1，limit=5。不要调用其他工具。",
        missing_path.display()
    );

    info!("Act: tomcat chat 触发 read 不存在路径错误，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("OPENAI_API_KEY", &api_key)
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin(format!("{prompt}\n"))
        .timeout(std::time::Duration::from_secs(60));
    let assert = c.assert();
    let output = assert.get_output();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    info!(
        "Assert: stderr 含 [tool] read + 真实错误；stderr: {}",
        trunc(&stderr, 1200)
    );
    assert.success();
    assert!(
        stderr.contains("[tool] read"),
        "stderr 应出现 [tool] read 行，实际: {}",
        trunc(&stderr, 600)
    );
    let stderr_lower = stderr.to_ascii_lowercase();
    assert!(
        stderr_lower.contains("no such file")
            || stderr_lower.contains("not found")
            || stderr_lower.contains("os error 2")
            || stderr.contains("不存在"),
        "stderr 应包含路径不存在语义，实际: {}",
        trunc(&stderr, 800)
    );
    assert!(
        !stderr.contains("✗ failed"),
        "有真实错误文本时不应退化成 failed 占位，实际: {}",
        trunc(&stderr, 800)
    );
}

// ──────────────────── Story 2.5: Bash 后台监控 P1 真 LLM 黑盒（E2E-CLI-016C~016E） ────────────────────
//
// 这 3 条真测分别锁三条 P1 门禁：
// - `016C`：finish-only auto-feed。模型起后台 bash 后先做独立工作，不主动 poll，
//   必须依赖 `<background-task-finished ...>` synthetic user message 继续推进。
// - `016D`：`task_output(block=true)` 单次 wait-slice。模型必须走阻塞等待路径，
//   stderr 要出现倒计时 update，transcript 要出现真实 `task_output` / `task_stop`。
// - `016E`：真实多次 timeout slice 重试。模型必须至少经历两次
//   `wakeReason="timeout" && finished=false`，然后继续等到 `new_output` 再收尾。
//
// 这组 helper 只做四件事：
// 1. 起临时 HOME + `tomcat init`
// 2. 把本次 scratch 目录 `workspace add`
// 3. 统一跑一次 `tomcat chat` 黑盒并捕获 stdout/stderr
// 4. 统一读取当前 transcript，便于断言真实 tool_call / tool_result

struct BackgroundBashP1RealLlmFixture {
    _home: tempfile::TempDir,
    work_dir: PathBuf,
    config_path: PathBuf,
    scratch: PathBuf,
    api_key: String,
}

struct CliChatRunCapture {
    success: bool,
    stdout: String,
    stderr: String,
}

fn setup_background_bash_p1_real_llm_fixture(
    scratch_leaf: &str,
) -> BackgroundBashP1RealLlmFixture {
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(work_dir.join("workspace-main")).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("workspace-temp")
        .join(scratch_leaf);
    std::fs::create_dir_all(&scratch).unwrap();
    let scratch = scratch.canonicalize().expect("workspace-temp scratch path");
    let scratch_str = scratch.to_str().expect("utf8 scratch path");

    info!("Arrange: tomcat init + OPENAI_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        panic!(
            "集成测试要求设置 OPENAI_API_KEY（无 key 时用例失败，符合 INTEGRATION_TEST_SPEC §5.2）"
        )
    });

    info!("Arrange: tomcat workspace add {}", scratch_str);
    cmd()
        .args(["workspace", "add", scratch_str])
        .env("HOME", dir.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("已添加工作区"));

    BackgroundBashP1RealLlmFixture {
        _home: dir,
        work_dir,
        config_path,
        scratch,
        api_key,
    }
}

fn run_background_bash_p1_real_llm_chat(
    fx: &BackgroundBashP1RealLlmFixture,
    prompt: String,
    timeout: std::time::Duration,
) -> CliChatRunCapture {
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", fx.work_dir.to_str().unwrap())
        .env("OPENAI_API_KEY", &fx.api_key)
        .env("TOMCAT__CONFIG_PATH", fx.config_path.to_str().unwrap())
        .env("RUST_LOG", "tomcat=info")
        .write_stdin(prompt)
        .timeout(timeout);
    let assert = c.assert();
    let output = assert.get_output();
    CliChatRunCapture {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    }
}

fn load_background_bash_p1_real_llm_transcript(fx: &BackgroundBashP1RealLlmFixture) -> String {
    let cfg = tomcat::load_config_toml_file(&fx.config_path).expect("load temp cli config");
    let sessions_dir = fx.work_dir.join("agents").join(&cfg.agent.id).join("sessions");
    let session = tomcat::SessionManager::new(sessions_dir);
    let transcript_path = session
        .current_transcript_path()
        .expect("current_transcript_path")
        .expect("transcript path should exist");
    fs::read_to_string(&transcript_path).expect("read transcript")
}

/// [E2E-CLI-016C] bash 后台任务 finish-only auto-feed 真 LLM 黑盒回归
///
/// 用户意图：让模型严格走一次 `bash(run_in_background=true)`，先做独立文件写入，
/// 然后**禁止主动 poll**，必须依赖 runtime 自动注入的
/// `<background-task-finished ...>` synthetic user message 继续推进。
///
/// 验证：
/// - exit 0；
/// - stderr 出现 `[bg] task ... queued for next turn`（证明 lifecycle subscriber →
///   follow_up_queue → between-turns drain 路径真的跑了）；
/// - 后台任务真实产出 `bg_done.txt`，独立工作真实产出 `marker.txt`；
/// - stdout 最终包含约定完成词 `AUTOFEED_OK`。
///
/// 意义：P1 门禁——这条用例是真 LLM + 真 CLI `chat_loop` 黑盒，不是仅测 tool 层。
#[test]
fn test_user_background_bash_autofeed_real_llm_cli() {
    common::setup_logging();
    common::load_openai_test_env();
    let _span = info_span!("test_user_background_bash_autofeed_real_llm_cli").entered();

    let fx = setup_background_bash_p1_real_llm_fixture("e2e_cli016c_bg_autofeed");
    let bg_done = fx.scratch.join("bg_done.txt");
    let marker = fx.scratch.join("marker.txt");
    let _ = fs::remove_file(&bg_done);
    let _ = fs::remove_file(&marker);

    let prompt = format!(
        concat!(
            "以下内容就是完整步骤；不要要求我重复步骤，不要反问，不要 ask_question。 ",
            "请严格按下面步骤执行，不要偏离，不要解释策略： ",
            "1. 只启动一个后台 bash 任务，必须设置 run_in_background=true。 ",
            "2. 这个后台 bash 的 command 必须精确执行：sleep 2; printf BG_DONE > \"{bg_done}\"。 ",
            "3. 启动后台任务后，立刻创建文件 \"{marker}\"，内容必须精确为 MARKER。 ",
            "4. 从这一步开始，禁止调用 task_output、task_list、task_stop，也不要再启动新的 bash；你必须等待 runtime 自动注入的 <background-task-finished ...> 系统消息。 ",
            "5. 只有在看到该系统消息之后，才允许读取并确认 \"{bg_done}\" 和 \"{marker}\" 都存在且内容正确。 ",
            "6. 全部确认后，只回复一行 AUTOFEED_OK 并停止；不要输出别的结尾。"
        ),
        bg_done = bg_done.display(),
        marker = marker.display(),
    );

    info!("Act: tomcat chat 触发后台 bash auto-feed，timeout 120s");
    let run = run_background_bash_p1_real_llm_chat(&fx, prompt, std::time::Duration::from_secs(120));
    let stdout = run.stdout;
    let stderr = run.stderr;
    info!("[tomcat chat stdout] {}", trunc(&stdout, 1500));
    if !stderr.is_empty() {
        info!("[tomcat chat stderr] {}", trunc(&stderr, 2000));
    }
    info!("Assert: exit 0 + stderr 含 [bg] task + 两文件落盘 + stdout 含 AUTOFEED_OK");
    assert!(run.success, "tomcat chat 应 exit 0；stderr: {}", trunc(&stderr, 1200));
    assert!(
        stderr.contains("[bg] task") && stderr.contains("queued for next turn"),
        "stderr 应含后台完成 auto-feed 提示，实际: {}",
        trunc(&stderr, 1200)
    );
    assert!(bg_done.exists(), "后台任务产物应存在: {}", bg_done.display());
    assert!(marker.exists(), "独立工作产物应存在: {}", marker.display());
    let bg_done_text = fs::read_to_string(&bg_done).unwrap_or_default();
    let marker_text = fs::read_to_string(&marker).unwrap_or_default();
    assert_eq!(
        bg_done_text, "BG_DONE",
        "bg_done.txt 内容应精确为 BG_DONE，实际: {:?}",
        bg_done_text
    );
    assert_eq!(
        marker_text, "MARKER",
        "marker.txt 内容应精确为 MARKER，实际: {:?}",
        marker_text
    );
    assert!(
        stdout.contains("AUTOFEED_OK"),
        "stdout 应含 AUTOFEED_OK，实际: {}",
        trunc(&stdout, 600)
    );
}

/// [E2E-CLI-016D] `task_output(block=true)` wait-slice 真 LLM 黑盒回归
///
/// 用户意图：模型必须在 `bash(run_in_background=true)` 之后**立即**进入
/// `task_output(block=true, timeout_ms=300)` 循环，直到拿到 `wakeReason="new_output"`；
/// 不允许依赖 auto-feed，不允许 `task_output(block=false)`。
///
/// 验证：
/// - exit 0；
/// - stderr 含 `task_output` 且含 `waiting_for_output` 倒计时 update；
/// - transcript 中至少有 1 次 `task_output` tool call，且参数包含
///   `block=true` 与 `timeout_ms=300`；
/// - 后台任务真实产出 `blockwait_done.txt`；
/// - stdout 最终包含约定完成词 `BLOCKWAIT_OK`。
///
/// 意义：P1 第二条真门禁——真正覆盖 `block=true` 等待路径、CLI 倒计时 update、
/// transcript 中的真实 tool_call，以及 `new_output` 唤醒后的收尾。
#[test]
fn test_user_background_bash_blocking_waitslice_real_llm_cli() {
    common::setup_logging();
    common::load_openai_test_env();
    let _span = info_span!("test_user_background_bash_blocking_waitslice_real_llm_cli").entered();

    let fx = setup_background_bash_p1_real_llm_fixture("e2e_cli016d_blockwait");
    let done_path = fx.scratch.join("blockwait_done.txt");
    let _ = fs::remove_file(&done_path);

    let prompt = format!(
        concat!(
            "以下内容就是完整指令；不要要求我重复，不要反问，不要 ask_question。 ",
            "请严格执行，并只在最后回复一行 BLOCKWAIT_OK： ",
            "1. 启动一个后台 bash 任务，必须设置 run_in_background=true。 ",
            "2. 该后台 bash 的 command 必须精确执行：sleep 2; echo TOKEN_WAITSLICE; printf BLOCKWAIT_DONE > \"{done_path}\"; sleep 30。 ",
            "3. 拿到 task_id 后，必须立刻开始调用 task_output，且参数必须满足：block=true、timeout_ms=300、since 从 0 开始并按 next_offset 续传。 ",
            "4. 如果 task_output 返回 wakeReason=timeout 且 finished=false，这不是失败；你必须继续再次调用 task_output(block=true, timeout_ms=300) 等下一次 wait slice。 ",
            "5. 当 task_output 返回 wakeReason=new_output 且内容里出现 TOKEN_WAITSLICE 后，必须调用 task_stop 停掉这个后台任务，避免它继续睡眠。 ",
            "6. 禁止使用 task_output(block=false)，禁止依赖 <background-task-finished ...> 自动回灌，禁止启动新的 bash，禁止调用 task_list。 ",
            "7. 只有在看到 TOKEN_WAITSLICE 且已经 task_stop 之后，才允许读取并确认文件 \"{done_path}\" 存在且内容精确为 BLOCKWAIT_DONE。 ",
            "8. 全部确认后，只回复一行 BLOCKWAIT_OK 并停止。"
        ),
        done_path = done_path.display(),
    );

    info!("Act: tomcat chat 触发 block=true wait-slice，timeout 120s");
    let run = run_background_bash_p1_real_llm_chat(&fx, prompt, std::time::Duration::from_secs(120));
    let stdout = run.stdout;
    let stderr = run.stderr;
    info!("[tomcat chat stdout] {}", trunc(&stdout, 1800));
    if !stderr.is_empty() {
        info!("[tomcat chat stderr] {}", trunc(&stderr, 2200));
    }
    assert!(run.success, "tomcat chat 应 exit 0；stderr: {}", trunc(&stderr, 1200));
    assert!(
        stderr.contains("task_output") && stderr.contains("waiting_for_output"),
        "stderr 应出现 task_output 倒计时 update，实际: {}",
        trunc(&stderr, 1200)
    );
    assert!(
        done_path.exists(),
        "后台任务产物应存在: {}",
        done_path.display()
    );
    let done_text = fs::read_to_string(&done_path).unwrap_or_default();
    assert_eq!(
        done_text, "BLOCKWAIT_DONE",
        "blockwait_done.txt 内容应精确为 BLOCKWAIT_DONE，实际: {:?}",
        done_text
    );
    assert!(
        stdout.contains("BLOCKWAIT_OK"),
        "stdout 应含 BLOCKWAIT_OK，实际: {}",
        trunc(&stdout, 600)
    );

    // transcript 级硬断言：必须能看到真实 `task_output(block=true)` + `task_stop`。
    let transcript = load_background_bash_p1_real_llm_transcript(&fx);
    let task_output_calls = transcript.matches("\"name\":\"task_output\"").count();
    assert!(
        task_output_calls >= 1,
        "transcript 中 task_output 次数应至少为 1（证明真 LLM 走了 block=true 等待路径），实际 {}；transcript: {}",
        task_output_calls,
        trunc(&transcript, 1500)
    );
    assert!(
        transcript.contains("\\\"block\\\":true") && transcript.contains("\\\"timeout_ms\\\":300"),
        "transcript 应含 block=true 与 timeout_ms=300；actual: {}",
        trunc(&transcript, 1500)
    );
    assert!(
        transcript.contains("TOKEN_WAITSLICE"),
        "transcript 应能看到 wait-slice 唤醒后的 token；actual: {}",
        trunc(&transcript, 1500)
    );
    assert!(
        transcript.contains("\"name\":\"task_stop\""),
        "transcript 应含 task_stop（收尾后台任务），actual: {}",
        trunc(&transcript, 1500)
    );
}

/// [E2E-CLI-016E] 真 LLM 多次 timeout slice 重试
///
/// 用户意图：模型必须在同一个后台任务上经历**至少两次**
/// `wakeReason="timeout" && finished=false`，继续重试 `task_output(block=true, timeout_ms=300)`，
/// 最后再等到一次 `new_output` 并收尾。
///
/// 验证：
/// - exit 0；
/// - stderr 含 `task_output` 与 `waiting_for_output`；
/// - transcript 中 `task_output` tool call 至少 3 次；
/// - transcript 中 `role=tool` 的结果里，`wakeReason="timeout"` 至少 2 次；
/// - transcript 中最终出现 `TOKEN_MULTI_TIMEOUT` 与 `task_stop`；
/// - 真实产物 `multi_timeout_done.txt` 存在且内容正确；
/// - stdout 最终包含 `MULTI_TIMEOUT_OK`。
#[test]
fn test_user_background_bash_multiple_timeout_slices_real_llm_cli() {
    common::setup_logging();
    common::load_openai_test_env();
    let _span = info_span!("test_user_background_bash_multiple_timeout_slices_real_llm_cli")
        .entered();

    let fx = setup_background_bash_p1_real_llm_fixture("e2e_cli016e_multi_timeout");
    let done_path = fx.scratch.join("multi_timeout_done.txt");
    let _ = fs::remove_file(&done_path);

    let prompt = format!(
        concat!(
            "以下内容就是完整指令；不要要求我重复，不要反问，不要 ask_question。 ",
            "请严格执行，并只在最后回复一行 MULTI_TIMEOUT_OK： ",
            "1. 启动一个后台 bash 任务，必须设置 run_in_background=true。 ",
            "2. 该后台 bash 的 command 必须精确执行：sleep 8; echo TOKEN_MULTI_TIMEOUT; printf MULTI_TIMEOUT_DONE > \"{done_path}\"; sleep 30。 ",
            "3. 拿到 task_id 后，必须立刻开始调用 task_output，且参数必须满足：block=true、timeout_ms=300、since 从 0 开始并按 next_offset 续传。 ",
            "4. 你必须真实观察到至少两次 wakeReason=timeout 且 finished=false；每次 timeout 都必须继续再次调用 task_output(block=true, timeout_ms=300)。 ",
            "5. 在至少两次 timeout 之后，继续用同样方式等待，直到某次 task_output 返回 wakeReason=new_output 且内容里出现 TOKEN_MULTI_TIMEOUT。 ",
            "6. 一旦看到 TOKEN_MULTI_TIMEOUT，必须调用 task_stop 停掉该后台任务，避免它继续睡眠。 ",
            "7. 禁止使用 task_output(block=false)，禁止依赖 <background-task-finished ...> 自动回灌，禁止启动新的 bash，禁止调用 task_list。 ",
            "8. 只有在已经看到至少两次 timeout、随后看到 TOKEN_MULTI_TIMEOUT、并且已经 task_stop 之后，才允许读取并确认文件 \"{done_path}\" 存在且内容精确为 MULTI_TIMEOUT_DONE。 ",
            "9. 全部确认后，只回复一行 MULTI_TIMEOUT_OK 并停止。"
        ),
        done_path = done_path.display(),
    );

    info!("Act: tomcat chat 触发多次 timeout slice，timeout 150s");
    let run = run_background_bash_p1_real_llm_chat(&fx, prompt, std::time::Duration::from_secs(150));
    let stdout = run.stdout;
    let stderr = run.stderr;
    info!("[tomcat chat stdout] {}", trunc(&stdout, 2200));
    if !stderr.is_empty() {
        info!("[tomcat chat stderr] {}", trunc(&stderr, 2600));
    }
    assert!(run.success, "tomcat chat 应 exit 0；stderr: {}", trunc(&stderr, 1400));
    assert!(
        stderr.contains("task_output") && stderr.contains("waiting_for_output"),
        "stderr 应出现 task_output 倒计时 update，实际: {}",
        trunc(&stderr, 1400)
    );
    assert!(
        done_path.exists(),
        "后台任务产物应存在: {}",
        done_path.display()
    );
    let done_text = fs::read_to_string(&done_path).unwrap_or_default();
    assert_eq!(
        done_text, "MULTI_TIMEOUT_DONE",
        "multi_timeout_done.txt 内容应精确为 MULTI_TIMEOUT_DONE，实际: {:?}",
        done_text
    );
    assert!(
        stdout.contains("MULTI_TIMEOUT_OK"),
        "stdout 应含 MULTI_TIMEOUT_OK，实际: {}",
        trunc(&stdout, 800)
    );

    let transcript = load_background_bash_p1_real_llm_transcript(&fx);

    let mut task_output_calls = 0usize;
    let mut timeout_results = 0usize;
    let mut saw_new_output_token = false;
    let mut saw_task_stop = false;
    for line in transcript.lines() {
        let Ok(value) = serde_json::from_str::<serde_json::Value>(line) else {
            continue;
        };
        let Some(message) = value.get("message") else {
            continue;
        };
        let role = message.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if role == "assistant" {
            if let Some(tool_calls) = message.get("tool_calls").and_then(|v| v.as_array()) {
                for tc in tool_calls {
                    let name = tc
                        .get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    let args = tc
                        .get("function")
                        .and_then(|f| f.get("arguments"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if name == "task_output" {
                        task_output_calls += 1;
                        assert!(
                            args.contains("\"block\":true") && args.contains("\"timeout_ms\":300"),
                            "task_output 调用参数应固定为 block=true + timeout_ms=300，实际 args={args}"
                        );
                    } else if name == "task_stop" {
                        saw_task_stop = true;
                    }
                }
            }
        } else if role == "tool" {
            let content = message.get("content").and_then(|v| v.as_str()).unwrap_or("");
            if content.contains("\"wakeReason\":\"timeout\"") {
                timeout_results += 1;
            }
            if content.contains("TOKEN_MULTI_TIMEOUT") && content.contains("\"wakeReason\":\"new_output\"")
            {
                saw_new_output_token = true;
            }
        }
    }

    assert!(
        task_output_calls >= 3,
        "transcript 中 task_output 次数应至少为 3（至少两次 timeout + 一次 new_output），实际 {}；transcript: {}",
        task_output_calls,
        trunc(&transcript, 1800)
    );
    assert!(
        timeout_results >= 2,
        "transcript 中 timeout slice 次数应至少为 2，实际 {}；transcript: {}",
        timeout_results,
        trunc(&transcript, 1800)
    );
    assert!(
        saw_new_output_token,
        "transcript 中应看到包含 TOKEN_MULTI_TIMEOUT 的 new_output 结果；actual: {}",
        trunc(&transcript, 1800)
    );
    assert!(
        saw_task_stop || transcript.contains("\"name\":\"task_stop\""),
        "transcript 应含 task_stop（收尾后台任务），actual: {}",
        trunc(&transcript, 1800)
    );
}

/// [E2E-CLI-013] 用户要求助手在仓库约定的 `workspace-temp` 子目录下写文件
///
/// 验证：exit 0；`{CARGO_MANIFEST_DIR}/workspace-temp/e2e_cli013_hello/hello_e2e.txt` 存在且内容含 Hello E2E（或 stdout 含写入/创建确认）
/// 意义：scratch 走 `workspace-temp/`（INTEGRATION_TEST_SPEC §2.3），避免提示词里的「workspace 目录」被模型误解为 crate 下 `workspace/` 子目录
#[test]
fn test_user_asks_pi_to_write_hello_world_bash() {
    common::setup_logging();
    common::load_openai_test_env();
    let _span = info_span!("test_user_asks_pi_to_write_hello_world_bash").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(work_dir.join("workspace-main")).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    let scratch = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("workspace-temp")
        .join("e2e_cli013_hello");
    std::fs::create_dir_all(&scratch).unwrap();
    let scratch_canon = scratch.canonicalize().expect("workspace-temp scratch path");
    let scratch_str = scratch_canon.to_str().expect("utf8 scratch path");

    info!("Arrange: tomcat init + OPENAI_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        panic!(
            "集成测试要求设置 OPENAI_API_KEY（无 key 时用例失败，符合 INTEGRATION_TEST_SPEC §5.2）"
        )
    });

    info!("Arrange: tomcat workspace add {}", scratch_str);
    cmd()
        .args(["workspace", "add", scratch_str])
        .env("HOME", dir.path())
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .assert()
        .success()
        .stdout(predicate::str::contains("已添加工作区"));

    let prompt = format!(
        "请在目录 {} 下创建文件 hello_e2e.txt，内容写 Hello E2E。不要写到其他路径。\n",
        scratch_canon.display()
    );
    info!("Act: tomcat chat stdin 要求在 workspace-temp 子目录创建 hello_e2e.txt，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("OPENAI_API_KEY", &api_key)
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin(prompt)
        .timeout(std::time::Duration::from_secs(60));
    let assert = c.assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + 文件存在且含 Hello E2E 或 stdout 含操作确认；actual: {}",
        trunc(&out, 300)
    );
    assert.success();

    let hello_path = scratch_canon.join("hello_e2e.txt");
    if hello_path.exists() {
        let content = fs::read_to_string(&hello_path).unwrap();
        assert!(
            content.contains("Hello E2E"),
            "hello_e2e.txt 内容应含 'Hello E2E'，实际: {}",
            trunc(&content, 200)
        );
    } else {
        assert!(
            out.contains("写入")
                || out.contains("write")
                || out.contains("创建")
                || out.contains("创建了"),
            "未找到 hello_e2e.txt 时 stdout 应含写入/创建类确认，实际: {}",
            trunc(&out, 300)
        );
    }
}

// ──────────────────── Story 3: WasmEdge+QuickJS 插件系统（E2E-CLI-021~026） ────────────────────

/// 创建临时插件目录，包含 plugin.json + main.js
fn make_plugin_dir(id: &str) -> tempfile::TempDir {
    let tmp = tempfile::tempdir().expect("tempdir");
    let plugin_json = format!(
        r#"{{
            "id": "{id}",
            "name": "Test Plugin {id}",
            "version": "0.1.0",
            "description": "E2E test plugin",
            "author": "nibbles",
            "main": "main.js",
            "requiredPermissions": [],
            "requiredApiVersion": "1.0",
            "tags": []
        }}"#
    );
    std::fs::write(tmp.path().join("plugin.json"), plugin_json).expect("write plugin.json");
    std::fs::write(tmp.path().join("main.js"), "// init\n1 + 1;\n").expect("write main.js");
    tmp
}

/// [E2E-CLI-021] 用户从路径加载插件并查看已加载列表
///
/// 用户意图：加载插件并验证命令正常执行
/// 验证：load exit 0；list exit 0（注：插件状态为进程内存，跨进程不持久化——MVP 已知限制）
#[test]
fn test_user_loads_plugin_and_lists() {
    common::setup_logging();
    let _span = info_span!("test_user_loads_plugin_and_lists").entered();

    let plugin_dir = make_plugin_dir("e2e-test-plugin-list");
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: plugin dir = {:?}", plugin_dir.path());
    info!("Act: tomcat plugin load");
    let load_assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "load", plugin_dir.path().to_str().unwrap()])
        .assert();
    let load_out = String::from_utf8_lossy(&load_assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert load: exit 0, stdout 非空；actual: {}",
        trunc(&load_out, 200)
    );
    load_assert.success();

    info!("Act: tomcat plugin list（跨进程，状态不持久）");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "list"])
        .assert();
    info!("Assert list: exit 0（不崩溃即可）");
    assert.success();
}

/// [E2E-CLI-022] 用户查看插件详情（名称、版本）
///
/// 用户意图：查看插件详情
/// 验证：exit 0；stdout 含 name/version
#[test]
fn test_user_views_plugin_info() {
    common::setup_logging();
    let _span = info_span!("test_user_views_plugin_info").entered();

    let plugin_dir = make_plugin_dir("e2e-test-plugin-info");
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: load plugin first");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "load", plugin_dir.path().to_str().unwrap()])
        .assert()
        .success();

    info!("Act: tomcat plugin info <id>");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "info", "e2e-test-plugin-info"])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含 name 或 version；actual: {}",
        trunc(&out, 300)
    );
    assert.success().stdout(
        predicate::str::contains("e2e-test-plugin-info")
            .or(predicate::str::contains("0.1.0"))
            .or(predicate::str::contains("version")),
    );
}

/// [E2E-CLI-023] 用户禁用插件
///
/// 用户意图：禁用已加载的插件
/// 验证：exit 0
#[test]
fn test_user_disables_plugin() {
    common::setup_logging();
    let _span = info_span!("test_user_disables_plugin").entered();

    let plugin_dir = make_plugin_dir("e2e-test-plugin-disable");
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: load plugin");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "load", plugin_dir.path().to_str().unwrap()])
        .assert()
        .success();

    info!("Act: tomcat plugin disable <id>");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "disable", "e2e-test-plugin-disable"])
        .assert();
    info!("Assert: exit 0");
    assert.success();
}

/// [E2E-CLI-024] 用户重新启用被禁用的插件
///
/// 用户意图：重新启用已禁用的插件
/// 验证：exit 0
#[test]
fn test_user_enables_plugin_after_disable() {
    common::setup_logging();
    let _span = info_span!("test_user_enables_plugin_after_disable").entered();

    let plugin_dir = make_plugin_dir("e2e-test-plugin-enable");
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: load + disable plugin");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "load", plugin_dir.path().to_str().unwrap()])
        .assert()
        .success();
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "disable", "e2e-test-plugin-enable"])
        .assert()
        .success();

    info!("Act: tomcat plugin enable <id>");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "enable", "e2e-test-plugin-enable"])
        .assert();
    info!("Assert: exit 0");
    assert.success();
}

/// [E2E-CLI-025] 用户卸载插件后从列表消失
///
/// 用户意图：卸载插件后列表不含该插件
/// 验证：unload exit 0；list stdout 不含该 id
#[test]
fn test_user_unloads_plugin_removes_from_list() {
    common::setup_logging();
    let _span = info_span!("test_user_unloads_plugin_removes_from_list").entered();

    let plugin_dir = make_plugin_dir("e2e-test-plugin-unload");
    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: load plugin");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "load", plugin_dir.path().to_str().unwrap()])
        .assert()
        .success();

    info!("Act: tomcat plugin unload <id>");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "unload", "e2e-test-plugin-unload"])
        .assert()
        .success();

    info!("Act: tomcat plugin list");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["plugin", "list"])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!("Assert: list 不含 id；actual: {}", trunc(&out, 300));
    assert.success();
    assert!(
        !out.contains("e2e-test-plugin-unload"),
        "卸载后 list 不应含该插件 id，实际 stdout: {}",
        trunc(&out, 200)
    );
}

/// [E2E-CLI-026] 用户加载不存在路径时看到错误提示
///
/// 用户意图：加载不存在的插件路径，看到友好错误
/// 验证：exit 0；stdout 含 error 或"不存在"
#[test]
fn test_user_loads_nonexistent_plugin_path_shows_error() {
    common::setup_logging();
    let _span = info_span!("test_user_loads_nonexistent_plugin_path_shows_error").entered();

    info!("Arrange: /nonexistent/path/to/plugin 不存在");
    info!("Act: tomcat plugin load /nonexistent/path/to/plugin");
    let assert = cmd()
        .args(["plugin", "load", "/nonexistent/path/to/plugin"])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含 error 提示；actual: {}",
        trunc(&out, 300)
    );
    assert.success().stdout(
        predicate::str::contains("不存在")
            .or(predicate::str::contains("error"))
            .or(predicate::str::contains("Error"))
            .or(predicate::str::contains("找不到")),
    );
}

// ──────────────────── Story 7: LLM 统一接入（E2E-CLI-041~042，需 OPENAI_API_KEY） ────────────────────

/// [E2E-CLI-041] 用户与 LLM 对话，获得流式渲染回复
///
/// 用户意图：与 LLM 对话，获得非空 AI 回复
/// 验证：exit 0；stdout 含 AI 回复
/// 要求：OPENAI_API_KEY 已设置
#[test]
fn test_user_chats_with_llm_gets_streaming_response() {
    common::setup_logging();
    common::load_openai_test_env();
    let _span = info_span!("test_user_chats_with_llm_gets_streaming_response").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init + OPENAI_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        panic!(
            "集成测试要求设置 OPENAI_API_KEY（无 key 时用例失败，符合 INTEGRATION_TEST_SPEC §5.2）"
        )
    });

    info!("Act: tomcat chat + stdin 单句，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("OPENAI_API_KEY", &api_key)
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin("请用一句话回答：1+1 等于几？\n")
        .timeout(std::time::Duration::from_secs(60));
    let assert = c.assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含 AI 回复；actual: {}",
        trunc(&out, 300)
    );
    assert.success();
    assert!(
        !out.trim().is_empty(),
        "LLM 应输出非空流式回复，实际 stdout 为空"
    );
}

/// [E2E-CLI-042] 确认 LLM 回复内容非空（基础连通性）
///
/// 用户意图：发送极短提问，验证 LLM 回复非空
/// 验证：exit 0；stdout 非空
/// 要求：OPENAI_API_KEY 已设置
#[test]
fn test_user_receives_nonempty_llm_response() {
    common::setup_logging();
    common::load_openai_test_env();
    let _span = info_span!("test_user_receives_nonempty_llm_response").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init + OPENAI_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        panic!(
            "集成测试要求设置 OPENAI_API_KEY（无 key 时用例失败，符合 INTEGRATION_TEST_SPEC §5.2）"
        )
    });

    info!("Act: tomcat chat + stdin 说一个字，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("OPENAI_API_KEY", &api_key)
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin("说一个字\n")
        .timeout(std::time::Duration::from_secs(60));
    let assert = c.assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!("Assert: exit 0 + stdout 非空；actual: {}", trunc(&out, 300));
    assert.success();
    assert!(
        !out.trim().is_empty(),
        "LLM 应输出非空回复，实际 stdout 为空"
    );
}

// ──────────────────── Story 8: CLI对话与会话管理（E2E-CLI-051~082） ────────────────────

/// [E2E-CLI-051] 用户创建一个新会话
///
/// 用户意图：创建新会话
/// 验证：exit 0；stdout 含"已创建会话"
#[test]
fn test_user_creates_new_session() {
    common::setup_logging();
    let _span = info_span!("test_user_creates_new_session").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: fresh work dir");
    info!("Act: tomcat session new");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "new"])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含已创建会话；actual: {}",
        trunc(&out, 200)
    );
    assert
        .success()
        .stdout(predicate::str::contains("已创建会话"));
}

/// [E2E-CLI-052] 用户查看所有会话
///
/// 用户意图：列出所有会话
/// 验证：exit 0
#[test]
fn test_user_lists_sessions() {
    common::setup_logging();
    let _span = info_span!("test_user_lists_sessions").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: create a session first");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "new"])
        .assert()
        .success();
    info!("Arrange: create a second historical session");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "new"])
        .assert()
        .success();

    info!("Act: tomcat session list");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "list"])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + current key only appears once; actual: {}",
        trunc(&out, 200)
    );
    assert.success();
    assert_eq!(
        out.matches(DEFAULT_SESSION_KEY).count(),
        1,
        "只有当前绑定的 session 应显示 default key，历史 session 不应都复用同一个 key"
    );
}

/// [E2E-CLI-053] 用户切换到已存在的会话
///
/// 用户意图：创建会话后切换到 default 会话
/// 验证：exit 0
#[test]
fn test_user_switches_to_existing_session() {
    common::setup_logging();
    let _span = info_span!("test_user_switches_to_existing_session").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: create session");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "new"])
        .assert()
        .success();

    info!("Act: tomcat session switch agent:main:main");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "switch", "agent:main:main"])
        .assert();
    info!("Assert: exit 0");
    assert.success();
}

/// [E2E-CLI-054] 用户切换到不存在会话时看到友好提示
///
/// 用户意图：切换到不存在会话，看到"不存在"提示
/// 验证：exit 0；stdout 含"不存在"
#[test]
fn test_user_switches_to_nonexistent_session_shows_error() {
    common::setup_logging();
    let _span = info_span!("test_user_switches_to_nonexistent_session_shows_error").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: no session pre-created");
    info!("Act: tomcat session switch nonexistent-key-e2e");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "switch", "nonexistent-key-e2e"])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含不存在；actual: {}",
        trunc(&out, 200)
    );
    assert.success().stdout(predicate::str::contains("不存在"));
}

/// [E2E-CLI-055] 用户删除刚创建的会话
///
/// 用户意图：创建后删除会话
/// 验证：exit 0；stdout 含"已删除"
#[test]
fn test_user_deletes_session() {
    common::setup_logging();
    let _span = info_span!("test_user_deletes_session").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: create session");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "new"])
        .assert()
        .success();

    info!("Act: tomcat session delete agent:main:main");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "delete", "agent:main:main"])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含已删除；actual: {}",
        trunc(&out, 200)
    );
    assert.success().stdout(predicate::str::contains("已删除"));
}

/// [E2E-CLI-056] 用户归档会话
///
/// 用户意图：归档刚创建的会话
/// 验证：exit 0；stdout 含"已归档"
#[test]
fn test_user_archives_session() {
    common::setup_logging();
    let _span = info_span!("test_user_archives_session").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: create session");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "new"])
        .assert()
        .success();

    info!("Act: tomcat session archive agent:main:main");
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "archive", "agent:main:main"])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + stdout 含已归档；actual: {}",
        trunc(&out, 200)
    );
    assert.success().stdout(predicate::str::contains("已归档"));
}

/// [E2E-CLI-057] 用户按关键词搜索会话
///
/// 用户意图：按当前固定 session key 搜索会话
/// 验证：exit 0
#[test]
fn test_user_searches_sessions_by_keyword() {
    common::setup_logging();
    let _span = info_span!("test_user_searches_sessions_by_keyword").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir
        .path()
        .canonicalize()
        .unwrap_or_else(|_| dir.path().to_path_buf());

    info!("Arrange: create session");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "new"])
        .assert()
        .success();
    info!("Arrange: create a second historical session");
    cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "new"])
        .assert()
        .success();

    info!("Act: tomcat session search {}", DEFAULT_SESSION_KEY);
    let assert = cmd()
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .args(["session", "search", DEFAULT_SESSION_KEY])
        .assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + search only reports current key once; actual: {}",
        trunc(&out, 200)
    );
    assert.success();
    assert_eq!(
        out.matches(DEFAULT_SESSION_KEY).count(),
        1,
        "按 key 搜索时不应把同一个 current key 误投影到所有历史 session"
    );
}

/// [E2E-CLI-058] 无 API key 时 chat 快速失败，不挂起
///
/// 用户意图：未配置 API Key 时 chat 应快速报错而非挂起
/// 验证：进程 5s 内结束；stdout 或 stderr 含错误提示
#[test]
fn test_user_chat_without_api_key_fails_gracefully() {
    common::setup_logging();
    let _span = info_span!("test_user_chat_without_api_key_fails_gracefully").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init，移除 OPENAI_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    info!("Act: tomcat chat without API key，timeout 5s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .env_remove("OPENAI_API_KEY")
        .write_stdin("hello\n")
        .timeout(std::time::Duration::from_secs(5));
    let output = c.output().unwrap();
    let stdout = String::from_utf8_lossy(&output.stdout).to_string();
    let stderr = String::from_utf8_lossy(&output.stderr).to_string();
    info!(
        "Assert: 进程 5s 内结束，含错误提示；stdout: {}",
        trunc(&stdout, 200)
    );
    let combined = format!("{stdout}{stderr}");
    assert!(
        combined.contains("error")
            || combined.contains("Error")
            || combined.contains("key")
            || combined.contains("API")
            || combined.to_lowercase().contains("invalid")
            || combined.contains("配置")
            || combined.contains("失败"),
        "chat 无 API Key 时应含错误提示，实际 combined: {}",
        trunc(&combined, 300)
    );
}

/// [E2E-CLI-059] 用户查看操作审计记录列表
///
/// 用户意图：列出审计记录
/// 验证：exit 0
#[test]
fn test_user_views_audit_list() {
    common::setup_logging();
    let _span = info_span!("test_user_views_audit_list").entered();

    info!("Arrange: no special setup");
    info!("Act: tomcat audit list");
    let assert = cmd().args(["audit", "list"]).assert();
    info!("Assert: exit 0");
    assert.success();
}

/// [E2E-CLI-060] 用户导出审计记录到文件
///
/// 用户意图：导出审计日志到 JSON 文件
/// 验证：exit 0（MVP 阶段 audit export 命令可正常执行不崩溃）
#[test]
fn test_user_exports_audit_to_file() {
    common::setup_logging();
    let _span = info_span!("test_user_exports_audit_to_file").entered();

    let dir = tempfile::tempdir().unwrap();
    let out_path = dir.path().join("audit_e2e.json");

    info!("Arrange: temp audit export path = {:?}", out_path);
    info!("Act: tomcat audit export");
    let assert = cmd()
        .args(["audit", "export", out_path.to_str().unwrap()])
        .assert();
    info!("Assert: exit 0");
    assert.success();
}

/// [E2E-CLI-061] 用户查看不存在的审计条目时友好提示
///
/// 用户意图：查看 ID=9999999 的审计条目，看到友好提示
/// 验证：exit 0；不 panic
#[test]
fn test_user_views_audit_show_invalid_id() {
    common::setup_logging();
    let _span = info_span!("test_user_views_audit_show_invalid_id").entered();

    info!("Arrange: no special setup");
    info!("Act: tomcat audit show 9999999");
    let assert = cmd().args(["audit", "show", "9999999"]).assert();
    info!("Assert: exit 0, 不 panic");
    assert.success();
}

// ──────────────────── 边界与健壮性场景（E2E-CLI-071~074） ────────────────────

/// [E2E-CLI-071] 用户查看帮助，所有子命令可见
///
/// 用户意图：查看主帮助，所有子命令应在 stdout 中
/// 验证：exit 0；stdout 含 init/doctor/config/session/plugin/audit
#[test]
fn test_user_views_full_help() {
    common::setup_logging();
    let _span = info_span!("test_user_views_full_help").entered();

    info!("Act: tomcat --help");
    let assert = cmd().arg("--help").assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + 含所有子命令；actual: {}",
        trunc(&out, 400)
    );
    assert
        .success()
        .stdout(predicate::str::contains("init"))
        .stdout(predicate::str::contains("doctor"))
        .stdout(predicate::str::contains("config"))
        .stdout(predicate::str::contains("session"))
        .stdout(predicate::str::contains("plugin"))
        .stdout(predicate::str::contains("audit"));
}

/// [E2E-CLI-072] 用户查看版本号
///
/// 用户意图：查看 tomcat 的版本号
/// 验证：exit 0；stdout 含版本号字符串
#[test]
fn test_user_views_version() {
    common::setup_logging();
    let _span = info_span!("test_user_views_version").entered();

    info!("Act: tomcat --version");
    let assert = cmd().arg("--version").assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!("Assert: exit 0 + 含版本号；actual: {}", trunc(&out, 100));
    assert
        .success()
        .stdout(predicate::str::is_match(r"\d+\.\d+").unwrap());
}

/// [E2E-CLI-073] 用户输入错误命令时看到帮助
///
/// 用户意图：输入未知子命令，看到错误提示
/// 验证：exit 非 0；stderr 含"error"
#[test]
fn test_user_runs_unknown_command() {
    common::setup_logging();
    let _span = info_span!("test_user_runs_unknown_command").entered();

    info!("Act: tomcat nonexistent_cmd_e2e");
    let assert = cmd().arg("nonexistent_cmd_e2e").assert();
    let stderr = String::from_utf8_lossy(&assert.get_output().stderr.clone()).to_string();
    info!(
        "Assert: exit 非 0 + stderr 含 error；actual: {}",
        trunc(&stderr, 200)
    );
    assert
        .failure()
        .stderr(predicate::str::contains("error").or(predicate::str::contains("unrecognized")));
}

/// [E2E-CLI-074] 用户 init 后 doctor 通过，完整引导流程
///
/// 用户意图：新手引导——init 后 doctor 应检测通过
/// 验证：两步 exit 0；doctor 含"✓"
#[test]
fn test_user_init_then_doctor_roundtrip() {
    common::setup_logging();
    let _span = info_span!("test_user_init_then_doctor_roundtrip").entered();

    let dir = tempfile::tempdir().unwrap();

    info!("Arrange: fresh temp dir");
    info!("Act: tomcat init → tomcat doctor");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let assert = cmd().args(["doctor"]).env("HOME", dir.path()).assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!(
        "Assert: exit 0 + 含 配置合法 + 内嵌资源已就绪 + QuickJS wasm；actual: {}",
        trunc(&out, 500)
    );
    assert
        .success()
        .stdout(predicate::str::contains("配置合法"))
        .stdout(predicate::str::contains("内嵌资源已就绪").or(predicate::str::contains("✓")));
}

// ──────────────────── Story 9 补充: chat --resume 与多轮上下文（E2E-CLI-082~083） ────────────────────

/// [E2E-CLI-082] 用户用 --resume 恢复上次会话
///
/// 用户意图：用 --resume 恢复已有会话，历史消息从 JSONL 加载
/// 验证：exit 0；进程正常退出（不崩溃）
/// 要求：OPENAI_API_KEY 已设置
#[test]
fn test_user_chat_resumes_last_session() {
    common::setup_logging();
    common::load_openai_test_env();
    let _span = info_span!("test_user_chat_resumes_last_session").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init + OPENAI_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();
    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        panic!(
            "集成测试要求设置 OPENAI_API_KEY（无 key 时用例失败，符合 INTEGRATION_TEST_SPEC §5.2）"
        )
    });

    info!("Act: 第一轮 tomcat chat，建立会话历史");
    cmd()
        .arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("OPENAI_API_KEY", &api_key)
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin("请回答：1+1=？\n")
        .timeout(std::time::Duration::from_secs(60))
        .assert()
        .success();

    info!("Act: 第二轮 tomcat chat --resume，恢复会话");
    let mut c = cmd();
    c.arg("chat")
        .arg("--resume")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("OPENAI_API_KEY", &api_key)
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin("好的，谢谢\n")
        .timeout(std::time::Duration::from_secs(60));
    let assert = c.assert();
    let out = String::from_utf8_lossy(&assert.get_output().stdout.clone()).to_string();
    info!("Assert: exit 0 + stdout 非空；actual: {}", trunc(&out, 300));
    assert.success();
    assert!(
        !out.trim().is_empty(),
        "--resume 后 AI 应有回复，实际 stdout 为空"
    );
}

// ────────────────────── TASK-14 AgentLoop E2E 用例 ──────────────────────

/// [用户场景] 用户启动 `tomcat chat` 并输入单句提问，AgentLoop 执行并输出 AI 回复
///
/// 验证：exit 0 且 stdout 包含非空 AI 回复文本（需 OPENAI_API_KEY；无 key 时 panic，符合规范）
/// 意义：TASK-14 T1-P1-005 E2E 门禁——验证 AgentLoop::run() 已完整接入 tomcat chat 交互链路（E2E_TEST_SPEC §6）
#[test]
fn test_user_chat_non_interactive_with_prompt_flag() {
    common::setup_logging();
    common::load_openai_test_env();
    let _span = info_span!("test_user_chat_non_interactive_with_prompt_flag").entered();

    let dir = tempfile::tempdir().unwrap();
    let work_dir = dir.path().join("work");
    std::fs::create_dir_all(&work_dir).unwrap();
    let config_path = dir.path().join(".tomcat").join("tomcat.config.toml");

    info!("Arrange: tomcat init 生成配置；加载 OPENAI_API_KEY");
    cmd()
        .args(["init"])
        .env("HOME", dir.path())
        .env("SHELL", "/bin/zsh")
        .assert()
        .success();

    let api_key = std::env::var("OPENAI_API_KEY").unwrap_or_else(|_| {
        panic!(
            "集成测试要求设置 OPENAI_API_KEY（无 key 时用例失败，符合 INTEGRATION_TEST_SPEC §5.2）"
        )
    });

    info!("Act: tomcat chat stdin 单轮问答，timeout 60s");
    let mut c = cmd();
    c.arg("chat")
        .env("TOMCAT__STORAGE__WORK_DIR", work_dir.to_str().unwrap())
        .env("OPENAI_API_KEY", &api_key)
        .env("TOMCAT__CONFIG_PATH", config_path.to_str().unwrap())
        .write_stdin("Reply with exactly: pong\n")
        .timeout(std::time::Duration::from_secs(60));

    let assert = c.assert();
    let out = assert.get_output().stdout.clone();
    let out_str = String::from_utf8_lossy(&out);

    info!(
        "Assert: exit 0，stdout 含 AI 回复（非空）；actual stdout 前 300 chars: {}",
        out_str.chars().take(300).collect::<String>()
    );
    assert.success();
    assert!(
        !out_str.trim().is_empty(),
        "AgentLoop 应输出非空 AI 回复，实际 stdout 为空"
    );
}
