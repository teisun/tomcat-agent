use super::*;

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
