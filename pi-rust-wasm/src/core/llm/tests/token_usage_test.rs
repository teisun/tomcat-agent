//! # `SessionTokenUsage::add` 累加正确性
//!
//! 验证多次调用 `add` 后 `input_tokens` / `output_tokens` 累加结果一致。

use super::super::token_usage::SessionTokenUsage;

#[test]
fn session_token_usage_add() {
    let mut u = SessionTokenUsage::default();
    u.add(10, 20);
    assert_eq!(u.input_tokens, 10);
    assert_eq!(u.output_tokens, 20);
    u.add(5, 15);
    assert_eq!(u.input_tokens, 15);
    assert_eq!(u.output_tokens, 35);
}
