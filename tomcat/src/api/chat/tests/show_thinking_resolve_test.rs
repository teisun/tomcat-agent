//! `resolve_initial_thinking_display` 单元测试：覆盖
//! 「`PI_CHAT_SHOW_THINKING` 已设置 → 用 env；否则 → `config.llm.thinking.show`」的优先级，
//! 以及新三档字符串与历史 bool env 的兼容映射。
//!
//! 用 `serial_test::serial` 序列化 env 变更，避免与其它使用同一变量的测试交错。

use super::super::resolve_initial_thinking_display;
use crate::infra::config::{ThinkingConfig, ThinkingDisplay};
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
fn env_unset_falls_back_to_config_show_summary() {
    set_env(None);
    let cfg = ThinkingConfig {
        show: ThinkingDisplay::Summary,
        ..ThinkingConfig::default()
    };
    assert_eq!(
        resolve_initial_thinking_display(&cfg),
        ThinkingDisplay::Summary
    );
}

#[test]
#[serial(pi_chat_show_thinking_env)]
fn env_unset_falls_back_to_config_show_full() {
    set_env(None);
    let cfg = ThinkingConfig {
        show: ThinkingDisplay::Full,
        ..ThinkingConfig::default()
    };
    assert_eq!(
        resolve_initial_thinking_display(&cfg),
        ThinkingDisplay::Full
    );
}

#[test]
#[serial(pi_chat_show_thinking_env)]
fn env_explicit_modes_override_config() {
    let cfg = ThinkingConfig {
        show: ThinkingDisplay::Full,
        ..ThinkingConfig::default()
    };
    set_env(Some("minimal"));
    assert_eq!(
        resolve_initial_thinking_display(&cfg),
        ThinkingDisplay::Minimal
    );
    set_env(Some("summary"));
    assert_eq!(
        resolve_initial_thinking_display(&cfg),
        ThinkingDisplay::Summary
    );
    set_env(Some("full"));
    assert_eq!(
        resolve_initial_thinking_display(&cfg),
        ThinkingDisplay::Full
    );
    set_env(None);
}

#[test]
#[serial(pi_chat_show_thinking_env)]
fn env_legacy_bool_values_remain_compatible() {
    let cfg = ThinkingConfig {
        show: ThinkingDisplay::Minimal,
        ..ThinkingConfig::default()
    };
    for truthy in ["1", "true", "TRUE", "True", "yes", "on"] {
        set_env(Some(truthy));
        assert_eq!(
            resolve_initial_thinking_display(&cfg),
            ThinkingDisplay::Full,
            "env={truthy} 应当兼容历史 true -> full"
        );
    }
    for falsy in ["0", "false", "no", "off", ""] {
        set_env(Some(falsy));
        assert_eq!(
            resolve_initial_thinking_display(&cfg),
            ThinkingDisplay::Summary,
            "env={falsy:?} 应当兼容历史 false -> summary"
        );
    }
    set_env(None);
}
