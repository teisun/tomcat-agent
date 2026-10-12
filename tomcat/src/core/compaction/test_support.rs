use std::collections::HashSet;

use crate::core::llm::ChatMessage;

pub(crate) fn durable_ids(messages: &[ChatMessage], start_idx: usize) -> Vec<String> {
    messages
        .get(start_idx..)
        .unwrap_or_default()
        .iter()
        .filter_map(|message| message.msg_id.clone())
        .collect()
}

/// A compaction operation may replace history, but it must not manufacture a
/// new current-tail message or duplicate a durable transcript message id.
pub(crate) fn assert_tail_invariant(
    before_tail_ids: &[String],
    messages: &[ChatMessage],
    start_idx: usize,
) {
    let after_tail_ids = durable_ids(messages, start_idx);
    let before_tail_id_set: HashSet<_> = before_tail_ids.iter().collect();
    assert!(
        after_tail_ids
            .iter()
            .all(|id| before_tail_id_set.contains(id)),
        "a compaction operation must not move a new durable message into the active tail: before={before_tail_ids:?}, after={after_tail_ids:?}"
    );

    let durable_ids: Vec<_> = messages
        .iter()
        .filter_map(|message| message.msg_id.as_deref())
        .collect();
    assert_eq!(
        durable_ids.iter().collect::<HashSet<_>>().len(),
        durable_ids.len(),
        "a compaction operation must not duplicate durable message ids: {durable_ids:?}"
    );
}
