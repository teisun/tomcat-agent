use super::*;

#[test]
fn unknown_control_request_is_cancelled_without_touching_the_reply() {
    let (input, mut commands) = mpsc::unbounded_channel();
    let bridge = Arc::new(PermissionBridge::new(input));
    let notification = bridge
        .forward(ControlFrame::request(
            "pending-question".to_owned(),
            "ask_question",
            Some("session".into()),
            json!({"questions":[{"prompt":"QUESTION_PAYLOAD_MUST_NOT_BECOME_REPLY_TEXT"}]}),
        ))
        .unwrap();
    assert!(
        notification.is_none(),
        "unsupported controls must not create assistant text"
    );
    match commands.try_recv().unwrap().command {
        ServeCommand::ControlCancel {
            request_id,
            session_id,
            payload,
        } => {
            assert_eq!(request_id, "pending-question");
            assert_eq!(session_id.as_deref(), Some("session"));
            assert_eq!(payload["reason"], "unsupported_acp_interaction");
            assert_eq!(payload["outcome"], "interrupted");
        }
        other => panic!("expected cancellation, got {other:?}"),
    }
    assert!(
        commands.try_recv().is_err(),
        "only one cancellation is sent"
    );
}
