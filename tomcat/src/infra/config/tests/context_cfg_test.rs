//! # `ContextConfig` 与 `compute_context_budget_chars`
//!
//! 覆盖：
//!
//! - `ContextConfig::default` 的全部字段（context_window / max_output_tokens /
//!   keep_recent_turns / compaction_model / layer0_single_result_max_chars /
//!   layer0_placeholder_threshold_chars / current_tail_compactable_min_chars /
//!   current_tail_single_result_max_chars / compaction_max_tokens）。
//! - `compute_context_budget_chars` 在 GPT-5.4 默认配置、`max_output_tokens=0`
//!   与 `context_window<max_output_tokens` 三种边界场景下的输出。
//! - `[context]` 段的 toml override 能正确传到 `cfg.context` 字段。

use super::super::*;
use std::io::Write;

#[test]
fn context_config_default_values() {
    let cfg = ContextConfig::default();
    assert_eq!(cfg.context_window, 400_000);
    assert_eq!(cfg.max_output_tokens, 128_000);
    assert_eq!(cfg.keep_recent_turns, 5);
    assert_eq!(cfg.compaction_model, DEFAULT_LLM_MODEL);
    assert_eq!(cfg.layer0_single_result_max_chars, 50_000);
    assert_eq!(cfg.layer0_placeholder_threshold_chars, 10_000);
    assert_eq!(cfg.current_tail_compactable_min_chars, 1);
    assert_eq!(cfg.current_tail_single_result_max_chars, 10_000);
    assert_eq!(cfg.compaction_max_tokens, 10_000);
}

#[test]
fn context_budget_chars_gpt52() {
    let cfg = ContextConfig {
        context_window: 400_000,
        max_output_tokens: 128_000,
        ..Default::default()
    };
    let budget = compute_context_budget_chars(&cfg);
    assert_eq!(budget, 1_088_000);
}

#[test]
fn context_budget_chars_zero_output() {
    let cfg = ContextConfig {
        context_window: 100_000,
        max_output_tokens: 0,
        ..Default::default()
    };
    let budget = compute_context_budget_chars(&cfg);
    assert_eq!(budget, 400_000);
}

#[test]
fn context_budget_chars_overflow_protection() {
    let cfg = ContextConfig {
        context_window: 10,
        max_output_tokens: 100,
        ..Default::default()
    };
    let budget = compute_context_budget_chars(&cfg);
    assert_eq!(budget, 0);
}

#[test]
fn context_config_toml_override() {
    let dir = std::env::temp_dir().join("tomcat_ctx_config_test");
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("config.toml");
    let mut f = std::fs::File::create(&path).unwrap();
    f.write_all(b"[context]\ncontext_window = 200000\nmax_output_tokens = 64000\ncompaction_model = \"gpt-4o-mini\"\n").unwrap();
    drop(f);
    let r = load_config(Some(path.as_path()));
    assert!(r.is_ok());
    let cfg = r.unwrap();
    assert_eq!(cfg.context.context_window, 200_000);
    assert_eq!(cfg.context.max_output_tokens, 64_000);
    assert_eq!(cfg.context.compaction_model, "gpt-4o-mini");
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}
