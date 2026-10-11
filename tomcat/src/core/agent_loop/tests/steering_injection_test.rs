use super::super::steering_injection::SteeringInbox;
use crate::core::llm::ChatMessage;

#[test]
fn close_if_empty_and_admission_share_one_boundary() {
    let mut inbox = SteeringInbox::default();
    inbox.push(ChatMessage::steering("first"));
    assert!(!inbox.close_if_empty());
    inbox.clear();
    assert!(inbox.close_if_empty());
    inbox.push(ChatMessage::steering("too late"));
    assert!(inbox.is_empty());
    inbox.open();
    inbox.push(ChatMessage::steering("next run"));
    assert_eq!(inbox.len(), 1);
    inbox.close_and_clear();
    assert!(inbox.is_empty());
    assert!(!inbox.is_open());
}
