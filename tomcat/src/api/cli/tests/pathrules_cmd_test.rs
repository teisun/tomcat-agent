use super::*;

#[test]
fn parse_mode_recognises_canonical_forms() {
    assert!(matches!(parse_mode("deny"), Ok(PathRuleMode::Deny)));
    assert!(matches!(parse_mode("DENY"), Ok(PathRuleMode::Deny)));
    assert!(matches!(parse_mode("readonly"), Ok(PathRuleMode::Readonly)));
    assert!(matches!(
        parse_mode("read-only"),
        Ok(PathRuleMode::Readonly)
    ));
    assert!(matches!(parse_mode("ro"), Ok(PathRuleMode::Readonly)));
}

#[test]
fn parse_mode_rejects_unknown() {
    match parse_mode("allow") {
        Err(AppError::Config(msg)) => assert!(msg.contains("未识别")),
        other => panic!("expected Config error, got {:?}", other),
    }
}
