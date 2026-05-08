//! `resolve_initial_show_thinking` 单元测试：覆盖
//! 「`PI_CHAT_SHOW_THINKING` 已设置 → 用 env；否则 → `config.llm.thinking.show`」的优先级（计划 §1.B/F）。
//!
//! 用 `serial_test::serial` 序列化 env 变更，避免与其它使用同一变量的测试交错。

use super::super::resolve_initial_show_thinking;
use crate::infra::config::ThinkingConfig;
use serial_test::serial;

const ENV_KEY: &str = "PI_CHAT_SHOW_THINKING";

fn set_env(v: Option<&str>) {
    // SAFETY: 单测内串行（serial_test 标注），env 改动仅本组测试感知。
    unsafe {
        match v {
            Some(s) => std::env::set_var(ENV_KEY, s),
            None => std::env::remove_var(ENV_KEY),
        }
    }
}

#[test]
#[serial(pi_chat_show_thinking_env)]
fn env_unset_falls_back_to_config_show_true() {
    set_env(None);
    let cfg = ThinkingConfig {
        show: true,
        ..ThinkingConfig::default()
    };
    assert!(resolve_initial_show_thinking(&cfg));
}

#[test]
#[serial(pi_chat_show_thinking_env)]
fn env_unset_falls_back_to_config_show_false() {
    set_env(None);
    let cfg = ThinkingConfig {
        show: false,
        ..ThinkingConfig::default()
    };
    assert!(!resolve_initial_show_thinking(&cfg));
}

#[test]
#[serial(pi_chat_show_thinking_env)]
fn env_truthy_overrides_config_show_false() {
    let cfg = ThinkingConfig {
        show: false,
        ..ThinkingConfig::default()
    };
    for truthy in ["1", "true", "TRUE", "True", "yes", "on"] {
        set_env(Some(truthy));
        assert!(
            resolve_initial_show_thinking(&cfg),
            "env={} 应当被识别为 true 并覆盖 config.show=false",
            truthy
        );
    }
    set_env(None);
}

#[test]
#[serial(pi_chat_show_thinking_env)]
fn env_falsy_overrides_config_show_true() {
    let cfg = ThinkingConfig {
        show: true,
        ..ThinkingConfig::default()
    };
    for falsy in ["0", "false", "no", "off", ""] {
        set_env(Some(falsy));
        assert!(
            !resolve_initial_show_thinking(&cfg),
            "env={:?} 应当被识别为 false 并覆盖 config.show=true",
            falsy
        );
    }
    set_env(None);
}
