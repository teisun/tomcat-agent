#[test]
fn foreground_wait_clamps_to_contract() {
    struct Values;
    assert_eq!(crate::infra::MIN_TOOLS_BASH_FOREGROUND_WAIT_MS, 8_000);
    assert_eq!(crate::infra::MAX_TOOLS_BASH_FOREGROUND_WAIT_MS, 16_000);
    let _ = Values;
}
