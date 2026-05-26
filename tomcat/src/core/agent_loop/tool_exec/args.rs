use crate::core::tools::primitive::{EditOperation, EditOperationType};

pub(super) fn parse_optional_u64(args: &serde_json::Value, key: &str) -> Option<u64> {
    let v = args.get(key)?;
    if v.is_null() {
        return None;
    }
    v.as_u64()
}

pub(super) fn parse_edit_args(
    args: &serde_json::Value,
) -> Result<(&str, Vec<EditOperation>), String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "edit: 缺少必填字段 `path`".to_string())?;

    if let Some(edits_v) = args.get("edits") {
        let arr = edits_v
            .as_array()
            .ok_or_else(|| "edit: `edits` 必须是数组".to_string())?;
        if arr.is_empty() {
            return Err("edit: `edits` 至少需要一条编辑段".to_string());
        }
        let mut ops = Vec::with_capacity(arr.len());
        for (i, seg) in arr.iter().enumerate() {
            let old = seg
                .get("old_content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("edit: edits[{}].old_content 缺失或非字符串", i))?;
            let new_c = seg
                .get("new_content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| format!("edit: edits[{}].new_content 缺失或非字符串", i))?;
            let replace_all = seg
                .get("replace_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            ops.push(make_edit_op(old, new_c, replace_all));
        }
        return Ok((path, ops));
    }

    let old = args
        .get("old_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "edit: 缺少 `old_content`（或 `edits`）".to_string())?;
    let new_c = args
        .get("new_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "edit: 缺少 `new_content`".to_string())?;
    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    Ok((path, vec![make_edit_op(old, new_c, replace_all)]))
}

fn make_edit_op(old: &str, new_c: &str, replace_all: bool) -> EditOperation {
    let encoded_old = if replace_all {
        format!(
            "{}{}",
            crate::core::tools::primitive::EDIT_REPLACE_ALL_MARKER,
            old
        )
    } else {
        old.to_string()
    };
    EditOperation {
        operation_type: EditOperationType::Replace,
        start_line: None,
        end_line: None,
        old_content: Some(encoded_old),
        new_content: new_c.to_string(),
    }
}

pub(super) fn parse_hashline_edit_args(
    args: &serde_json::Value,
) -> Result<(&str, Vec<crate::core::tools::primitive::HashlineSegment>), String> {
    use crate::core::tools::primitive::{HashlineOp, HashlineSegment};

    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "hashline_edit: 缺少必填字段 `path`".to_string())?;
    let edits_v = args
        .get("edits")
        .ok_or_else(|| "hashline_edit: 缺少必填字段 `edits`".to_string())?;
    let arr = edits_v
        .as_array()
        .ok_or_else(|| "hashline_edit: `edits` 必须是数组".to_string())?;
    if arr.is_empty() {
        return Err("hashline_edit: `edits` 至少需要一条段".to_string());
    }
    let mut segments = Vec::with_capacity(arr.len());
    for (i, seg) in arr.iter().enumerate() {
        let op_str = seg
            .get("op")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("hashline_edit: edits[{}].op 缺失或非字符串", i))?;
        let op = match op_str {
            "replace" => HashlineOp::Replace,
            "insert" => HashlineOp::Insert,
            "delete" => HashlineOp::Delete,
            other => {
                return Err(format!(
                    "hashline_edit: edits[{}].op 必须是 replace|insert|delete，实际 `{}`",
                    i, other
                ))
            }
        };
        let pos = seg
            .get("pos")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("hashline_edit: edits[{}].pos 缺失或非字符串", i))?;
        let (start_line, start_hash) =
            HashlineSegment::parse_anchor(pos, i, "pos").map_err(|e| e.to_string())?;
        let (end_line, end_hash) = match seg.get("end").and_then(|v| v.as_str()) {
            Some(end_s) => {
                HashlineSegment::parse_anchor(end_s, i, "end").map_err(|e| e.to_string())?
            }
            None => (start_line, start_hash.clone()),
        };
        let lines = seg
            .get("lines")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        segments.push(HashlineSegment {
            op,
            start_line,
            start_hash,
            end_line,
            end_hash,
            lines,
        });
    }
    Ok((path, segments))
}
