use super::super::*;

/// 仅控制台、成功路径。init_logging 内部会 init 全局 subscriber，进程内只能成功一次；
/// 全量测试若出现 "global default trace subscriber already set" 可单独跑：
/// cargo test -p pi_wasm -j 1 infra::logging::tests -- --test-threads=1
#[test]
fn a_init_logging_console_only_succeeds() {
    let cfg = LogConfig {
        level: "info".to_string(),
        file_enabled: false,
    };
    let r = init_logging(&cfg, None);
    assert!(r.is_ok(), "init_logging(console only) should succeed");
}

#[test]
fn log_config_default_level() {
    let cfg = LogConfig::default();
    assert_eq!(cfg.level, "info");
}

#[test]
fn invalid_log_level_returns_error() {
    let cfg = LogConfig {
        level: "not_a_level".to_string(),
        file_enabled: false,
    };
    let r = init_logging(&cfg, None);
    assert!(r.is_err());
}
