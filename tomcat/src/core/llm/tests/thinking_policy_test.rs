//! `thinking_policy` 单测：覆盖 ThinkingLevel/Format 解析与 resolve_request_fields 映射表。

use super::super::thinking_policy::{
    resolve_anthropic_request, resolve_request_fields, should_persist_thinking,
    should_strip_on_resend, strip_anthropic_thinking_blocks, thinking_format_for_api,
    thinking_format_for_model, ThinkingFormat, ThinkingLevel, ThinkingRequestFields,
};
use crate::infra::config::ThinkingConfig;

fn cfg_with(enabled: bool, level: &str) -> ThinkingConfig {
    ThinkingConfig {
        enabled,
        level: level.to_string(),
        ..ThinkingConfig::default()
    }
}

#[test]
fn level_parse_known_strings() {
    assert_eq!(
        ThinkingLevel::parse_or_medium("off"),
        (ThinkingLevel::Off, true)
    );
    assert_eq!(
        ThinkingLevel::parse_or_medium("MINIMAL"),
        (ThinkingLevel::Minimal, true)
    );
    assert_eq!(
        ThinkingLevel::parse_or_medium("Medium"),
        (ThinkingLevel::Medium, true)
    );
    assert_eq!(
        ThinkingLevel::parse_or_medium("xhigh"),
        (ThinkingLevel::Xhigh, true)
    );
    assert_eq!(
        ThinkingLevel::parse_or_medium("x-high"),
        (ThinkingLevel::Xhigh, true)
    );
}

#[test]
fn level_parse_unknown_falls_back_to_medium_and_signals_false() {
    assert_eq!(
        ThinkingLevel::parse_or_medium("turbo"),
        (ThinkingLevel::Medium, false)
    );
}

#[test]
fn format_resolve_auto_by_wire_api() {
    assert_eq!(
        ThinkingFormat::Auto.resolve_for_api("openai"),
        ThinkingFormat::Openai
    );
    assert_eq!(
        ThinkingFormat::Auto.resolve_for_api("openai-responses"),
        ThinkingFormat::Openai
    );
    assert_eq!(
        ThinkingFormat::Auto.resolve_for_api("anthropic-messages"),
        ThinkingFormat::Anthropic
    );
    assert_eq!(thinking_format_for_api("openai"), ThinkingFormat::Openai);
    assert_eq!(
        thinking_format_for_api("openai-responses"),
        ThinkingFormat::Openai
    );
    assert_eq!(
        thinking_format_for_api("deepseek"),
        ThinkingFormat::Deepseek
    );
    assert_eq!(thinking_format_for_api("doubao"), ThinkingFormat::Doubao);
    // 已显式指定的 format 不会被改写
    assert_eq!(
        ThinkingFormat::Doubao.resolve_for_api("openai"),
        ThinkingFormat::Doubao
    );
}

#[test]
fn format_resolve_auto_by_model_name() {
    assert_eq!(
        thinking_format_for_model("deepseek-v4-pro"),
        ThinkingFormat::Deepseek
    );
    assert_eq!(
        thinking_format_for_model("deepseek-v4-flash"),
        ThinkingFormat::Deepseek
    );
    assert_eq!(thinking_format_for_model("gpt-5"), ThinkingFormat::Openai);
    // MiMo 走豆包系 thinking 线格式（thinking: {"type":"enabled"}）。
    assert_eq!(
        thinking_format_for_model("mimo-v2.5-pro"),
        ThinkingFormat::Doubao
    );
    assert_eq!(
        thinking_format_for_model("claude-opus-4-6"),
        ThinkingFormat::Anthropic
    );
}

#[test]
fn explicit_format_wins_over_wire_auto_detection() {
    assert_eq!(
        ThinkingFormat::Openai.resolve_for_api("anthropic-messages"),
        ThinkingFormat::Openai
    );
    assert_eq!(
        ThinkingFormat::Auto.resolve_for_api("openai"),
        ThinkingFormat::Openai
    );
}

#[test]
fn disabled_or_off_yields_no_fields() {
    let off = resolve_request_fields(&cfg_with(false, "high"), ThinkingFormat::Openai);
    assert_eq!(off, ThinkingRequestFields::default());
    let off2 = resolve_request_fields(&cfg_with(true, "off"), ThinkingFormat::Openai);
    assert_eq!(off2, ThinkingRequestFields::default());
}

#[test]
fn openai_level_maps_to_reasoning_effort() {
    let r = resolve_request_fields(&cfg_with(true, "minimal"), ThinkingFormat::Openai);
    assert_eq!(r.reasoning_effort.as_deref(), Some("low"));
    assert!(r.thinking.is_none());
    let r = resolve_request_fields(&cfg_with(true, "medium"), ThinkingFormat::Openai);
    assert_eq!(r.reasoning_effort.as_deref(), Some("medium"));
    let r = resolve_request_fields(&cfg_with(true, "xhigh"), ThinkingFormat::Openai);
    assert_eq!(
        r.reasoning_effort.as_deref(),
        Some("high"),
        "xhigh 默认降级到 high，避免在不支持的模型上 400"
    );
}

#[test]
fn doubao_level_maps_to_thinking_object() {
    let r = resolve_request_fields(&cfg_with(true, "high"), ThinkingFormat::Doubao);
    let v = r.thinking.expect("doubao 应返回 thinking 对象");
    assert_eq!(v["type"], "enabled");
    assert!(v.get("max_tokens").is_none());
    assert!(
        r.reasoning_effort.is_none(),
        "互斥：豆包不应出 reasoning_effort"
    );
}

#[test]
fn doubao_max_tokens_propagates_when_set() {
    let mut c = cfg_with(true, "high");
    c.max_tokens = Some(2048);
    let r = resolve_request_fields(&c, ThinkingFormat::Doubao);
    assert_eq!(r.thinking.as_ref().unwrap()["max_tokens"], 2048);
}

#[test]
fn deepseek_writes_effort_and_thinking_enable_flag() {
    let r = resolve_request_fields(&cfg_with(true, "high"), ThinkingFormat::Deepseek);
    assert_eq!(r.reasoning_effort.as_deref(), Some("high"));
    assert_eq!(r.thinking.as_ref().unwrap()["type"], "enabled");
}

#[test]
fn deepseek_maps_lower_levels_to_high_and_xhigh_to_max() {
    let medium = resolve_request_fields(&cfg_with(true, "medium"), ThinkingFormat::Deepseek);
    assert_eq!(medium.reasoning_effort.as_deref(), Some("high"));

    let xhigh = resolve_request_fields(&cfg_with(true, "xhigh"), ThinkingFormat::Deepseek);
    assert_eq!(xhigh.reasoning_effort.as_deref(), Some("max"));
}

#[test]
fn qwen_has_no_request_field() {
    assert_eq!(
        resolve_request_fields(&cfg_with(true, "high"), ThinkingFormat::Qwen),
        ThinkingRequestFields::default()
    );
}

#[test]
fn anthropic_request_maps_to_enabled_budget_tokens() {
    let r = resolve_anthropic_request(&cfg_with(true, "high"), None);
    assert_eq!(r.max_tokens, 5120);
    assert_eq!(r.thinking.as_ref().unwrap()["type"], "enabled");
    assert_eq!(r.thinking.as_ref().unwrap()["budget_tokens"], 4096);
}

#[test]
fn anthropic_request_caps_budget_against_requested_max_tokens() {
    let r = resolve_anthropic_request(&cfg_with(true, "xhigh"), Some(1024));
    assert_eq!(r.max_tokens, 1024);
    assert_eq!(r.thinking.as_ref().unwrap()["budget_tokens"], 768);

    let off = resolve_anthropic_request(&cfg_with(true, "off"), Some(400));
    assert!(off.thinking.is_none());
    assert_eq!(off.max_tokens, 400);
}

#[test]
fn strip_on_resend_default_is_true_for_known_formats() {
    let cfg = ThinkingConfig::default();
    // 默认 strip_on_resend=true，已知 format 下具备「保留剥离意愿」语义。
    assert!(should_strip_on_resend(&cfg, ThinkingFormat::Openai));
    assert!(should_strip_on_resend(&cfg, ThinkingFormat::Deepseek));
    assert!(should_strip_on_resend(&cfg, ThinkingFormat::Doubao));
}

#[test]
fn strip_on_resend_off_when_explicitly_disabled() {
    let cfg = ThinkingConfig {
        strip_on_resend: false,
        ..ThinkingConfig::default()
    };
    assert!(!should_strip_on_resend(&cfg, ThinkingFormat::Openai));
    assert!(!should_strip_on_resend(&cfg, ThinkingFormat::Deepseek));
}

#[test]
fn strip_on_resend_returns_false_for_auto_unresolved_format() {
    let cfg = ThinkingConfig::default();
    // Auto 未推断时不下结论，留给 caller 显式 resolve 后再判。
    assert!(!should_strip_on_resend(&cfg, ThinkingFormat::Auto));
}

#[test]
fn persist_default_is_false_even_when_enabled() {
    let cfg = ThinkingConfig {
        enabled: true,
        persist: false,
        ..ThinkingConfig::default()
    };
    assert!(!should_persist_thinking(&cfg));
}

#[test]
fn persist_requires_both_enabled_and_persist_true() {
    let mut cfg = ThinkingConfig {
        enabled: false,
        persist: true,
        ..ThinkingConfig::default()
    };
    // 仅 persist=true、enabled=false → 不持久化（避免 thinking 关闭时落孤儿数据）。
    assert!(!should_persist_thinking(&cfg));
    cfg.enabled = true;
    assert!(should_persist_thinking(&cfg));
}

#[test]
fn anthropic_strip_removes_thinking_blocks() {
    let mut v = serde_json::json!([
        {"type": "thinking", "data": "internal"},
        {"type": "text", "text": "hello"},
        {"type": "thinking", "data": "more"},
    ]);
    let removed = strip_anthropic_thinking_blocks(&mut v);
    assert_eq!(removed, 2);
    let arr = v.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["type"], "text");
}

#[test]
fn anthropic_strip_is_noop_on_non_array() {
    let mut v = serde_json::json!({"type":"text", "text":"hi"});
    assert_eq!(strip_anthropic_thinking_blocks(&mut v), 0);
    let mut v = serde_json::json!("plain string");
    assert_eq!(strip_anthropic_thinking_blocks(&mut v), 0);
}

#[test]
fn anthropic_strip_keeps_unknown_types() {
    let mut v = serde_json::json!([
        {"type": "tool_use", "name": "bash"},
        {"type": "thinking", "data": "x"},
        {"unknown": true},
    ]);
    let removed = strip_anthropic_thinking_blocks(&mut v);
    assert_eq!(removed, 1, "只剥 type=thinking，其它包括无 type 的全保留");
    assert_eq!(v.as_array().unwrap().len(), 2);
}

#[test]
fn level_to_effort_table_is_stable() {
    let cases = [
        ("off", None),
        ("minimal", Some("low")),
        ("low", Some("low")),
        ("medium", Some("medium")),
        ("high", Some("high")),
        ("xhigh", Some("high")),
    ];
    for (level, expected) in cases {
        let r = resolve_request_fields(&cfg_with(true, level), ThinkingFormat::Openai);
        assert_eq!(
            r.reasoning_effort.as_deref(),
            expected,
            "level={} 不符: {:?}",
            level,
            r
        );
    }
}
