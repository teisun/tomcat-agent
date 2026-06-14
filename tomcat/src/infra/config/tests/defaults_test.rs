//! # `AppConfig` / `SecurityConfig` 默认值与序列化
//!
//! - 默认配置可通过 `serde_json::to_string` / `from_str` 完成 round-trip。
//! - `SecurityConfig::default` 不 panic。
//! - 仅传入 `{ "security": {} }` 时，缺省字段由 `default_*` 帮助函数填充
//!   （`enable_audit_log = true` / `audit_log_retention_days = 90`）。
//! - `AppConfig::default` 默认就包含 `context` 子配置。

use super::super::*;

#[test]
fn default_app_config_roundtrip() {
    let cfg = AppConfig::default();
    let j = serde_json::to_string(&cfg).unwrap();
    let _: AppConfig = serde_json::from_str(&j).unwrap();
}

#[test]
fn security_config_default() {
    let _ = SecurityConfig::default();
}

#[test]
fn deserialize_security_config_uses_default_helpers() {
    let s = r#"{"security":{}}"#;
    let cfg: AppConfig = serde_json::from_str(s).unwrap();
    assert!(cfg.security.enable_audit_log);
    assert_eq!(cfg.security.audit_log_retention_days, 90);
}

#[test]
fn app_config_includes_context() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.context.context_window, 400_000);
    assert_eq!(cfg.context.resume_hydration_mode, ResumeHydrationMode::Auto);
    assert_eq!(cfg.context.resume_lazy_threshold, 2_000);
}

#[test]
fn llm_files_default_expires_after_seconds_is_86400() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.llm.files.expires_after_seconds, 86_400);
    assert_eq!(cfg.llm.vision_model, None);
    assert_eq!(cfg.llm.title_model, None);
}

#[test]
fn llm_timeout_defaults_match_three_layer_policy() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.llm.stream_timeout_sec, 180);
    assert_eq!(cfg.llm.non_stream_stale_timeout_sec, 300);
    assert_eq!(cfg.llm.http_read_timeout_sec, 120);
}

#[test]
fn plugin_config_defaults_are_wired_into_app_config() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.plugin.auto_load, Vec::<String>::new());
    assert_eq!(cfg.plugin.js_heap_mb, 16);
    assert_eq!(cfg.plugin.call_timeout_ms, 30_000);
    assert_eq!(cfg.plugin.interrupt_budget, 5_000_000);
    assert_eq!(cfg.plugin.event_channel_capacity, 64);
    assert_eq!(cfg.plugin.idle_ttl_ms, 5 * 60 * 1000);
}

#[test]
fn plugin_config_toml_overrides_parse_correctly() {
    let toml_src = r#"
[plugin]
auto_load = ["demo"]
js_heap_mb = 8
call_timeout_ms = 1234
interrupt_budget = 777
event_channel_capacity = 9
idle_ttl_ms = 4567
"#;
    let cfg: AppConfig = toml::from_str(toml_src).expect("plugin config toml should parse");
    assert_eq!(cfg.plugin.auto_load, vec!["demo".to_string()]);
    assert_eq!(cfg.plugin.js_heap_mb, 8);
    assert_eq!(cfg.plugin.call_timeout_ms, 1234);
    assert_eq!(cfg.plugin.interrupt_budget, 777);
    assert_eq!(cfg.plugin.event_channel_capacity, 9);
    assert_eq!(cfg.plugin.idle_ttl_ms, 4567);
}

#[test]
fn plugin_engine_defaults_match_app_config_defaults() {
    let cfg = AppConfig::default();
    assert_eq!(crate::ext::DEFAULT_QUICKJS_HEAP_MB, cfg.plugin.js_heap_mb);
    assert_eq!(
        crate::ext::DEFAULT_PLUGIN_CALL_TIMEOUT_MS,
        cfg.plugin.call_timeout_ms
    );
    assert_eq!(
        crate::ext::DEFAULT_PLUGIN_INTERRUPT_BUDGET,
        cfg.plugin.interrupt_budget
    );
    assert_eq!(
        crate::ext::DEFAULT_PLUGIN_IDLE_TTL_MS,
        cfg.plugin.idle_ttl_ms
    );
}

#[test]
fn checkpoint_config_defaults_are_wired_into_app_config() {
    let cfg = AppConfig::default();
    assert_eq!(cfg.checkpoint.retention_max, 50);
    assert_eq!(cfg.checkpoint.retention_days, 7);
    assert!(cfg.preflight.auto_install_search_tools);
    assert!(cfg.preflight.auto_install_git);
    assert!(!cfg.preflight.show_search_tools_ui);
    assert!(!cfg.preflight.show_git_ui);
}

#[test]
fn thinking_show_default_is_summary() {
    let cfg = AppConfig::default();
    assert!(cfg.llm.thinking.enabled, "thinking 默认仍应启用");
    assert!(
        cfg.llm.reasoning_continuity.enabled,
        "reasoning continuity 默认应启用"
    );
    assert!(
        matches!(cfg.llm.thinking.show, ThinkingDisplay::Summary),
        "ThinkingConfig::default().show 应为 summary"
    );
}

#[test]
fn reasoning_continuity_toml_false_overrides_default() {
    let toml_src = r#"
[llm.reasoning_continuity]
enabled = false
"#;
    let cfg: AppConfig =
        toml::from_str(toml_src).expect("reasoning_continuity.enabled=false 应可反序列化");
    assert!(
        !cfg.llm.reasoning_continuity.enabled,
        "显式配置 false 应覆盖默认开启"
    );
}

/// 旧版 toml 用 `show = false` 写法的硬契约：保证升级 d2a11cd 后老用户配置不破。
#[test]
fn thinking_show_toml_legacy_bool_false_maps_to_summary() {
    let toml_src = r#"
[llm.thinking]
enabled = true
show = false
"#;
    let cfg: AppConfig = toml::from_str(toml_src).expect("legacy show=false 应可反序列化");
    assert!(
        matches!(cfg.llm.thinking.show, ThinkingDisplay::Summary),
        "show=false 应映射到 summary，实际：{:?}",
        cfg.llm.thinking.show
    );
}

/// 旧版 toml 用 `show = true` 写法：映射到 full 档（与历史展开 raw thinking 一致）。
#[test]
fn thinking_show_toml_legacy_bool_true_maps_to_full() {
    let toml_src = r#"
[llm.thinking]
enabled = true
show = true
"#;
    let cfg: AppConfig = toml::from_str(toml_src).expect("legacy show=true 应可反序列化");
    assert!(
        matches!(cfg.llm.thinking.show, ThinkingDisplay::Full),
        "show=true 应映射到 full，实际：{:?}",
        cfg.llm.thinking.show
    );
}

/// 三档字符串 toml 直读：minimal / summary / full 均能成功解析为对应枚举。
#[test]
fn thinking_show_toml_string_modes_parse_correctly() {
    for (raw, expected) in [
        ("minimal", ThinkingDisplay::Minimal),
        ("summary", ThinkingDisplay::Summary),
        ("full", ThinkingDisplay::Full),
    ] {
        let toml_src = format!(
            r#"
[llm.thinking]
enabled = true
show = "{raw}"
"#
        );
        let cfg: AppConfig = toml::from_str(&toml_src)
            .unwrap_or_else(|e| panic!("show=\"{raw}\" 应可反序列化：{e}"));
        assert!(
            std::mem::discriminant(&cfg.llm.thinking.show) == std::mem::discriminant(&expected),
            "show=\"{raw}\" 应映射到 {expected:?}，实际：{:?}",
            cfg.llm.thinking.show
        );
    }
}

/// 拒识别错档位字符串：避免 silent 默认值掩盖配置错别字。
#[test]
fn thinking_show_toml_unknown_string_rejected() {
    let toml_src = r#"
[llm.thinking]
enabled = true
show = "verbose"
"#;
    let err = toml::from_str::<AppConfig>(toml_src).expect_err("非法 show 字段应反序列化失败");
    let msg = err.to_string();
    assert!(
        msg.contains("verbose") || msg.contains("minimal|summary|full"),
        "错误信息应说明合法值或回显错误值：{msg}"
    );
}
