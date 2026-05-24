pub(super) fn simulate_apply_edits(
    original: &str,
    edits: &[crate::core::tools::primitive::EditOperation],
) -> String {
    let marker = crate::core::tools::primitive::EDIT_REPLACE_ALL_MARKER;
    let mut cur = original.to_string();
    for op in edits {
        let Some(raw_old) = op.old_content.as_deref() else {
            continue;
        };
        let (replace_all, old_text) = if let Some(stripped) = raw_old.strip_prefix(marker) {
            (true, stripped)
        } else {
            (false, raw_old)
        };
        if old_text.is_empty() {
            continue;
        }
        if replace_all {
            cur = cur.replace(old_text, &op.new_content);
        } else {
            cur = cur.replacen(old_text, &op.new_content, 1);
        }
    }
    cur
}
