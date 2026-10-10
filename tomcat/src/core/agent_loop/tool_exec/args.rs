use crate::core::tools::primitive::{EditOperation, EditOperationType};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum EditStringMode {
    Replace,
    InsertBefore,
    InsertAfter,
}

pub(super) fn parse_optional_u64(args: &serde_json::Value, key: &str) -> Option<u64> {
    let v = args.get(key)?;
    if v.is_null() {
        return None;
    }
    v.as_u64()
}

pub(super) fn parse_load_skill_args(
    args: &serde_json::Value,
) -> Result<(&str, Option<&str>), String> {
    let name = args
        .get("name")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .ok_or_else(|| "load_skill: missing required field `name`".to_string())?;
    let file = match args.get("file") {
        None => None,
        Some(value) if value.is_null() => None,
        Some(value) => Some(
            value
                .as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .ok_or_else(|| "load_skill: `file` must be a string or null".to_string())?,
        ),
    };
    Ok((name, file))
}

pub(super) fn parse_edit_args(
    args: &serde_json::Value,
) -> Result<(&str, Vec<EditOperation>), String> {
    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .ok_or_else(|| "edit: missing required field `path`".to_string())?;

    let ops = parse_edit_ops(args, "edit")?;
    Ok((path, ops))
}

/// 一条什么都不改的编辑段：`old_content` 与 `new_content` 相同。
///
/// 走 strict schema 的模型会把它用不到的那套字段也一并填上 —— 用 `files` 批量改 5 个
/// 文件时，顶层照样得给个 `edits`。实测它有两种填法，都是 no-op：两边留空，或者两边
/// 都写字面量 `"placeholder"`。这种空壳必须在判定形态之前丢掉，否则「`path`/`edits`
/// 与 `files` 互斥」这条校验会把每一次批量编辑都判死（实测 16 次连续失败）。
///
/// 用 `old == new` 而不是「两边都空」当判据，是因为它说的正是「这段不改任何东西」，
/// 与模型拿什么字符串来填无关。删除内容是 `old` 非空、`new` 为空，不会命中。
fn is_placeholder_segment(seg: &serde_json::Value) -> bool {
    let text = |key: &str| seg.get(key).and_then(|v| v.as_str()).unwrap_or("");
    seg.get("mode")
        .and_then(|value| value.as_str())
        .is_none_or(|mode| mode == "replace")
        && text("old_content") == text("new_content")
}

/// 这层参数里的编辑段：`edits` 数组形态，或单段的 `old_content` / `new_content`。
pub(super) fn edit_segments(args: &serde_json::Value) -> Vec<&serde_json::Value> {
    match args.get("edits").and_then(|v| v.as_array()) {
        Some(arr) => arr.iter().collect(),
        None => vec![args],
    }
}

/// 一条编辑段的身份：模式、改什么、改成什么。用于判断两处写的是不是同一个意图。
pub(super) fn segment_identity(seg: &serde_json::Value) -> (String, String, String) {
    let text = |key: &str| {
        seg.get(key)
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string()
    };
    let mode = seg
        .get("mode")
        .and_then(|value| value.as_str())
        .unwrap_or("replace")
        .to_string();
    (mode, text("old_content"), text("new_content"))
}

/// `edits` 数组里有没有非空壳的段。
fn has_real_edits_in(edits: &serde_json::Value) -> bool {
    edits
        .as_array()
        .is_some_and(|arr| arr.iter().any(|seg| !is_placeholder_segment(seg)))
}

/// 这层参数里有没有**真的**编辑内容（用于形态判定，不做校验）。
pub(super) fn has_real_edits(args: &serde_json::Value) -> bool {
    args.get("edits").is_some_and(has_real_edits_in) || !is_placeholder_segment(args)
}

/// 解析一个文件的编辑段：`edits` 数组形态，或单段的 `old_content` / `new_content`。
/// `scope` 只用于错误信息定位（单文件是 `edit`，批量是 `files[i]`）。
pub(super) fn parse_edit_ops(
    args: &serde_json::Value,
    scope: &str,
) -> Result<Vec<EditOperation>, String> {
    // 非数组要留给下面的类型错误报出来，只有「是数组但全是空壳」才当没给。
    if let Some(edits_v) = args
        .get("edits")
        .filter(|v| !v.is_array() || has_real_edits_in(v))
    {
        let arr = edits_v
            .as_array()
            .ok_or_else(|| format!("edit: `{scope}.edits` must be an array"))?;
        if arr.is_empty() {
            return Err(format!(
                "edit: `{scope}.edits` requires at least one edit segment"
            ));
        }
        let mut ops = Vec::with_capacity(arr.len());
        for (i, seg) in arr
            .iter()
            .enumerate()
            .filter(|(_, seg)| !is_placeholder_segment(seg))
        {
            let old = seg
                .get("old_content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    format!("edit: {scope}.edits[{i}].old_content is missing or not a string")
                })?;
            let new_c = seg
                .get("new_content")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    format!("edit: {scope}.edits[{i}].new_content is missing or not a string")
                })?;
            let replace_all = seg
                .get("replace_all")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            let mode = parse_edit_mode(seg, &format!("{scope}.edits[{i}]"))?;
            ops.push(make_edit_op(old, new_c, replace_all, mode, scope)?);
        }
        return Ok(ops);
    }

    let old = args
        .get("old_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("edit: {scope} requires `old_content` (or `edits`)"))?;
    let new_c = args
        .get("new_content")
        .and_then(|v| v.as_str())
        .ok_or_else(|| format!("edit: {scope} requires `new_content`"))?;
    let replace_all = args
        .get("replace_all")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let mode = parse_edit_mode(args, scope)?;
    Ok(vec![make_edit_op(old, new_c, replace_all, mode, scope)?])
}

fn parse_edit_mode(args: &serde_json::Value, scope: &str) -> Result<EditStringMode, String> {
    match args.get("mode") {
        None => Ok(EditStringMode::Replace),
        Some(value) => match value.as_str() {
            Some("replace") => Ok(EditStringMode::Replace),
            Some("insert_before") => Ok(EditStringMode::InsertBefore),
            Some("insert_after") => Ok(EditStringMode::InsertAfter),
            _ => Err(format!(
                "edit: {scope}.mode must be `replace`, `insert_before` or `insert_after`"
            )),
        },
    }
}

fn make_edit_op(
    old: &str,
    new_c: &str,
    replace_all: bool,
    mode: EditStringMode,
    scope: &str,
) -> Result<EditOperation, String> {
    if replace_all && mode != EditStringMode::Replace {
        return Err(format!(
            "edit: {scope}.replace_all may only be used with mode=`replace`"
        ));
    }
    let encoded_old = match mode {
        EditStringMode::Replace if replace_all => format!(
            "{}{}",
            crate::core::tools::primitive::EDIT_REPLACE_ALL_MARKER,
            old
        ),
        EditStringMode::Replace => old.to_string(),
        EditStringMode::InsertBefore => format!(
            "{}{}",
            crate::core::tools::primitive::EDIT_INSERT_BEFORE_MARKER,
            old
        ),
        EditStringMode::InsertAfter => format!(
            "{}{}",
            crate::core::tools::primitive::EDIT_INSERT_AFTER_MARKER,
            old
        ),
    };
    Ok(EditOperation {
        operation_type: EditOperationType::Replace,
        start_line: None,
        end_line: None,
        old_content: Some(encoded_old),
        new_content: new_c.to_string(),
    })
}

pub(super) fn parse_hashline_edit_args(
    args: &serde_json::Value,
) -> Result<(&str, Vec<crate::core::tools::primitive::HashlineSegment>), String> {
    use crate::core::tools::primitive::{HashlineOp, HashlineSegment};

    let path = args
        .get("path")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "hashline_edit: missing required field `path`".to_string())?;
    let edits_v = args
        .get("edits")
        .ok_or_else(|| "hashline_edit: missing required field `edits`".to_string())?;
    let arr = edits_v
        .as_array()
        .ok_or_else(|| "hashline_edit: `edits` must be an array".to_string())?;
    if arr.is_empty() {
        return Err("hashline_edit: `edits` requires at least one edit segment".to_string());
    }
    let mut segments = Vec::with_capacity(arr.len());
    for (i, seg) in arr.iter().enumerate() {
        let op_str = seg
            .get("op")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("hashline_edit: edits[{i}].op is missing or not a string"))?;
        let op = match op_str {
            "replace" => HashlineOp::Replace,
            "insert" => HashlineOp::Insert,
            "delete" => HashlineOp::Delete,
            other => {
                return Err(format!(
                    "hashline_edit: edits[{i}].op must be replace|insert|delete; received `{other}`"
                ));
            }
        };
        let pos = seg
            .get("pos")
            .and_then(|v| v.as_str())
            .ok_or_else(|| format!("hashline_edit: edits[{i}].pos is missing or not a string"))?;
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
