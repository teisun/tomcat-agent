use super::super::*;

#[tokio::test]
async fn allow_all_returns_true() {
    let p = AllowAllConfirmation;
    let ok = p
        .confirm(PrimitiveOperation::Write, "preview", "p1")
        .await
        .unwrap();
    assert!(ok);
}

#[tokio::test]
async fn deny_all_returns_false() {
    let p = DenyAllConfirmation;
    let ok = p
        .confirm(PrimitiveOperation::Bash, "cmd", "p1")
        .await
        .unwrap();
    assert!(!ok);
}
