pub(super) fn simulate_apply_edits(
    original: &str,
    edits: &[crate::core::tools::primitive::EditOperation],
) -> String {
    use crate::core::tools::primitive::{
        EDIT_INSERT_AFTER_MARKER, EDIT_INSERT_BEFORE_MARKER, EDIT_REPLACE_ALL_MARKER,
    };

    let mut cur = original.to_string();
    for op in edits {
        let Some(raw_old) = op.old_content.as_deref() else {
            continue;
        };
        let (replace_all, old_text) =
            if let Some(stripped) = raw_old.strip_prefix(EDIT_REPLACE_ALL_MARKER) {
                (true, stripped)
            } else {
                (false, raw_old)
            };
        if old_text.is_empty() {
            continue;
        }
        if replace_all {
            cur = cur.replace(old_text, &op.new_content);
        } else if let Some(anchor) = old_text.strip_prefix(EDIT_INSERT_BEFORE_MARKER) {
            cur = cur.replacen(anchor, &format!("{}{}", op.new_content, anchor), 1);
        } else if let Some(anchor) = old_text.strip_prefix(EDIT_INSERT_AFTER_MARKER) {
            cur = cur.replacen(anchor, &format!("{}{}", anchor, op.new_content), 1);
        } else {
            cur = cur.replacen(old_text, &op.new_content, 1);
        }
    }
    cur
}
