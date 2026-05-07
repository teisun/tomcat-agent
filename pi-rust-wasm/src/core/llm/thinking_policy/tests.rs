//! `thinking_policy` 单测：覆盖 ThinkingLevel/Format 解析与 resolve_request_fields 映射表。

use super::{
    resolve_request_fields, ThinkingFormat, ThinkingLevel, ThinkingRequestFields,
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
fn format_resolve_auto_by_provider_id() {
    assert_eq!(ThinkingFormat::Auto.resolve("openai"), ThinkingFormat::Openai);
    assert_eq!(
        ThinkingFormat::Auto.resolve("openai-responses"),
        ThinkingFormat::Openai
    );
    assert_eq!(
        ThinkingFormat::Auto.resolve("deepseek"),
        ThinkingFormat::Deepseek
    );
    assert_eq!(
        ThinkingFormat::Auto.resolve("doubao"),
        ThinkingFormat::Doubao
    );
    // 已显式指定的 format 不会被改写
    assert_eq!(
        ThinkingFormat::Doubao.resolve("openai"),
        ThinkingFormat::Doubao
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
    assert!(r.reasoning_effort.is_none(), "互斥：豆包不应出 reasoning_effort");
}

#[test]
fn doubao_max_tokens_propagates_when_set() {
    let mut c = cfg_with(true, "high");
    c.max_tokens = Some(2048);
    let r = resolve_request_fields(&c, ThinkingFormat::Doubao);
    assert_eq!(r.thinking.as_ref().unwrap()["max_tokens"], 2048);
}

#[test]
fn deepseek_qwen_have_no_request_field() {
    assert_eq!(
        resolve_request_fields(&cfg_with(true, "high"), ThinkingFormat::Deepseek),
        ThinkingRequestFields::default()
    );
    assert_eq!(
        resolve_request_fields(&cfg_with(true, "high"), ThinkingFormat::Qwen),
        ThinkingRequestFields::default()
    );
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
