//! # `ContextConfig` 与 `compute_context_budget_chars`
//!
//! 覆盖：
//!
//! - `ContextConfig::default` 的全部字段（context_window_fallback / output_reserve_tokens /
//!   keep_recent_turns / compaction_model / layer0_single_result_max_chars /
//!   layer0_placeholder_threshold_chars / current_tail_compactable_min_chars /
//!   current_tail_single_result_max_chars / compaction_max_tokens /
//!   resume_hydration_mode / resume_lazy_threshold）。
//! - 兼容 fallback 预算与旧 `[context]` key 的 alias。
//! - `[context]` 段的 toml override 能正确传到 `cfg.context` 字段。

use super::super::*;
use std::io::Write;

#[test]
fn context_config_default_values() {
    let cfg = ContextConfig::default();
    assert_eq!(cfg.context_window_fallback, 400_000);
    assert_eq!(cfg.output_reserve_tokens, None);
    assert_eq!(cfg.keep_recent_turns, 5);
    assert_eq!(cfg.compaction_model, "gpt-5.2");
    assert_eq!(cfg.layer0_single_result_max_chars, 50_000);
    assert_eq!(cfg.layer0_placeholder_threshold_chars, 10_000);
    assert_eq!(cfg.current_tail_compactable_min_chars, 1);
    assert_eq!(cfg.current_tail_single_result_max_chars, 10_000);
    assert_eq!(cfg.compaction_max_tokens, 10_000);
    assert_eq!(cfg.resume_hydration_mode, ResumeHydrationMode::Auto);
    assert_eq!(cfg.resume_lazy_threshold, 2_000);
}

#[test]
fn context_budget_chars_gpt52() {
    let cfg = ContextConfig {
        context_window_fallback: 400_000,
        output_reserve_tokens: Some(128_000),
        ..Default::default()
    };
    let budget = compute_context_budget_chars(&cfg);
    assert_eq!(budget, 1_088_000);
}

#[test]
fn context_budget_chars_unknown_model_keeps_conservative_reserve() {
    let cfg = ContextConfig {
        context_window_fallback: 100_000,
        output_reserve_tokens: Some(0),
        ..Default::default()
    };
    let budget = compute_context_budget_chars(&cfg);
    assert_eq!(budget, 300_000);
}

#[test]
fn context_budget_chars_overflow_protection() {
    let cfg = ContextConfig {
        context_window_fallback: 10,
        output_reserve_tokens: Some(100),
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
    f.write_all(b"[context]\ncontext_window = 200000\nmax_output_tokens = 64000\ncompaction_model = \"gpt-5.4\"\nresume_hydration_mode = \"full\"\nresume_lazy_threshold = 4096\n").unwrap();
    drop(f);
    let r = load_config(Some(path.as_path()));
    assert!(r.is_ok());
    let cfg = r.unwrap();
    assert_eq!(cfg.context.context_window_fallback, 200_000);
    assert_eq!(cfg.context.output_reserve_tokens, Some(64_000));
    assert_eq!(cfg.context.compaction_model, "gpt-5.4");
    assert_eq!(cfg.context.resume_hydration_mode, ResumeHydrationMode::Full);
    assert_eq!(cfg.context.resume_lazy_threshold, 4_096);
    let rewritten = toml::to_string(&cfg.context).expect("context config serializes");
    assert!(
        rewritten.contains("context_window_fallback = 200000"),
        "new key must be written: {rewritten}"
    );
    assert!(
        rewritten.contains("output_reserve_tokens = 64000"),
        "new key must be written: {rewritten}"
    );
    assert!(
        !rewritten.contains("\ncontext_window =") && !rewritten.contains("\nmax_output_tokens ="),
        "legacy aliases must never be written back: {rewritten}"
    );
    let _ = std::fs::remove_file(&path);
    let _ = std::fs::remove_dir(&dir);
}

#[test]
fn context_config_accepts_new_limit_keys_and_rejects_mixed_aliases() {
    let dir = tempfile::tempdir().unwrap();
    let new_path = dir.path().join("new.toml");
    std::fs::write(
        &new_path,
        "[context]\ncontext_window_fallback = 200000\noutput_reserve_tokens = 64000\n",
    )
    .unwrap();
    let cfg = load_config(Some(new_path.as_path())).expect("new context keys load");
    assert_eq!(cfg.context.context_window_fallback, 200_000);
    assert_eq!(cfg.context.output_reserve_tokens, Some(64_000));

    let mixed_path = dir.path().join("mixed.toml");
    std::fs::write(
        &mixed_path,
        "[context]\ncontext_window = 200000\ncontext_window_fallback = 400000\n",
    )
    .unwrap();
    assert!(
        load_config(Some(mixed_path.as_path())).is_err(),
        "old and new names must not silently choose a winner"
    );

    let mixed_output_path = dir.path().join("mixed-output.toml");
    std::fs::write(
        &mixed_output_path,
        "[context]\nmax_output_tokens = 64000\noutput_reserve_tokens = 32000\n",
    )
    .unwrap();
    assert!(
        load_config(Some(mixed_output_path.as_path())).is_err(),
        "max_output_tokens and output_reserve_tokens must not silently choose a winner"
    );
}
