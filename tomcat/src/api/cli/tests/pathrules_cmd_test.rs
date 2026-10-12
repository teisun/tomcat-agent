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
        Err(AppError::Config(msg)) => assert_eq!(
            msg,
            crate::infra::i18n::tr_in(
                crate::infra::i18n::Locale::En,
                "cli.pathrules.invalidMode",
                &[("mode", "allow")]
            )
        ),
        other => panic!("expected Config error, got {:?}", other),
    }
}
