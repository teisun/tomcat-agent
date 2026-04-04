//! OpenAI Chat Completions 消息链：落盘前校验（规则 A–E）与从 transcript tail 收集连续 Message 内层 JSON。

use serde_json::Value;

use super::transcript::TranscriptEntry;

/// 从 transcript 尾部条目中收集连续的 Message 内层 `message` 对象（旧→新）。
pub(crate) fn collect_recent_chat_messages_from_tail(entries: &[TranscriptEntry]) -> Vec<Value> {
    let mut msgs: Vec<Value> = entries
        .iter()
        .rev()
        .filter_map(|e| {
            if let TranscriptEntry::Message(me) = e {
                Some(me.message.clone())
            } else {
                None
            }
        })
        .collect();
    msgs.reverse();
    msgs
}

/// 校验即将追加的消息是否满足 OpenAI 消息链约束（规则 A–E）。
/// 返回 Ok(()) 表示合法，Err(reason) 表示违规。
pub(crate) fn validate_append_message(
    incoming: &Value,
    recent_messages: &[Value],
) -> Result<(), String> {
    let role = incoming.get("role").and_then(|v| v.as_str()).unwrap_or("");

    match role {
        "tool" => validate_tool(incoming, recent_messages),
        "assistant" => validate_assistant(incoming, recent_messages),
        "user" | "system" => validate_user_or_system(role, recent_messages),
        "" => Err("message missing 'role' field".to_string()),
        other => Err(format!("unknown role '{other}'")),
    }
}

// ── Rule A: tool ──────────────────────────────────────────────────────────

fn validate_tool(incoming: &Value, recent: &[Value]) -> Result<(), String> {
    let prev = recent.last().ok_or("tool message as first entry")?;
    let prev_role = prev.get("role").and_then(|v| v.as_str()).unwrap_or("");
    let prev_ok = prev_role == "tool"
        || (prev_role == "assistant" && has_nonempty_tool_calls(prev));
    if !prev_ok {
        return Err(format!(
            "tool must follow assistant+tool_calls or tool, got '{prev_role}'"
        ));
    }

    let tc_id = incoming
        .get("tool_call_id")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if tc_id.is_empty() {
        return Err("tool message missing or empty 'tool_call_id'".to_string());
    }

    let (asst, tools_between) = find_owning_assistant(recent)?;
    let tc_arr = asst
        .get("tool_calls")
        .and_then(|v| v.as_array())
        .ok_or("owning assistant has no tool_calls array")?;
    let valid_ids: Vec<&str> = tc_arr
        .iter()
        .filter_map(|tc| tc.get("id").and_then(|v| v.as_str()))
        .collect();
    if !valid_ids.contains(&tc_id) {
        return Err(format!(
            "tool_call_id '{tc_id}' not found in owning assistant's tool_calls {valid_ids:?}"
        ));
    }

    for t in &tools_between {
        let existing_id = t.get("tool_call_id").and_then(|v| v.as_str()).unwrap_or("");
        if existing_id == tc_id {
            return Err(format!("duplicate tool result for tool_call_id '{tc_id}'"));
        }
    }

    Ok(())
}

fn find_owning_assistant(recent: &[Value]) -> Result<(&Value, Vec<&Value>), String> {
    let mut tools = Vec::new();
    for msg in recent.iter().rev() {
        let r = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if r == "tool" {
            tools.push(msg);
        } else if r == "assistant" && has_nonempty_tool_calls(msg) {
            return Ok((msg, tools));
        } else {
            return Err(format!(
                "expected assistant+tool_calls before tool sequence, got '{r}'"
            ));
        }
    }
    Err("no owning assistant+tool_calls found before tool sequence".to_string())
}

// ── Rule B & C: assistant ─────────────────────────────────────────────────

fn validate_assistant(incoming: &Value, recent: &[Value]) -> Result<(), String> {
    let has_tc = has_nonempty_tool_calls(incoming);

    if has_tc {
        validate_tool_calls_shape(incoming)?;
    }

    if let Some(prev) = recent.last() {
        let prev_role = prev.get("role").and_then(|v| v.as_str()).unwrap_or("");
        if prev_role == "assistant" && has_nonempty_tool_calls(prev) {
            return Err(
                "cannot append assistant after assistant+tool_calls without tool results"
                    .to_string(),
            );
        }
    }

    Ok(())
}

fn validate_tool_calls_shape(msg: &Value) -> Result<(), String> {
    let arr = msg
        .get("tool_calls")
        .and_then(|v| v.as_array())
        .ok_or("tool_calls is not an array")?;
    if arr.is_empty() {
        return Err("tool_calls array is empty".to_string());
    }
    for (i, tc) in arr.iter().enumerate() {
        if !tc.is_object() {
            return Err(format!("tool_calls[{i}] is not an object"));
        }
        let id = tc.get("id").and_then(|v| v.as_str()).unwrap_or("");
        if id.is_empty() {
            return Err(format!("tool_calls[{i}].id is missing or empty"));
        }
        let func = tc.get("function");
        let func_obj = func
            .and_then(|v| v.as_object())
            .ok_or(format!("tool_calls[{i}].function is not an object"))?;
        let name = func_obj.get("name").and_then(|v| v.as_str()).unwrap_or("");
        if name.is_empty() {
            return Err(format!("tool_calls[{i}].function.name is missing or empty"));
        }
    }
    Ok(())
}

// ── Rule D: user / system ─────────────────────────────────────────────────

fn validate_user_or_system(role: &str, recent: &[Value]) -> Result<(), String> {
    if is_in_pending_tool_round(recent) {
        return Err(format!(
            "cannot append '{role}' while tool round is incomplete"
        ));
    }
    Ok(())
}

fn is_in_pending_tool_round(recent: &[Value]) -> bool {
    let last = match recent.last() {
        Some(m) => m,
        None => return false,
    };
    let last_role = last.get("role").and_then(|v| v.as_str()).unwrap_or("");

    if last_role == "assistant" && has_nonempty_tool_calls(last) {
        return true;
    }

    if last_role == "tool" {
        let mut tool_count = 0usize;
        for msg in recent.iter().rev() {
            let r = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
            if r == "tool" {
                tool_count += 1;
            } else if r == "assistant" && has_nonempty_tool_calls(msg) {
                let tc_count = msg
                    .get("tool_calls")
                    .and_then(|v| v.as_array())
                    .map(|a| a.len())
                    .unwrap_or(0);
                return tool_count < tc_count;
            } else {
                return false;
            }
        }
    }

    false
}

fn has_nonempty_tool_calls(msg: &Value) -> bool {
    msg.get("tool_calls")
        .and_then(|v| v.as_array())
        .map(|a| !a.is_empty())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::session::transcript::{CompactionEntry, MessageEntry, TranscriptEntry};

    fn mk_user(text: &str) -> Value {
        serde_json::json!({ "role": "user", "content": text })
    }
    fn mk_system(text: &str) -> Value {
        serde_json::json!({ "role": "system", "content": text })
    }
    fn mk_assistant(text: &str) -> Value {
        serde_json::json!({ "role": "assistant", "content": text })
    }
    fn mk_assistant_tc(ids: &[&str]) -> Value {
        let tcs: Vec<Value> = ids
            .iter()
            .map(|id| {
                serde_json::json!({
                    "id": *id,
                    "type": "function",
                    "function": { "name": "read_file", "arguments": "{}" }
                })
            })
            .collect();
        serde_json::json!({ "role": "assistant", "tool_calls": tcs })
    }
    fn mk_tool(tc_id: &str) -> Value {
        serde_json::json!({ "role": "tool", "tool_call_id": tc_id, "content": "ok" })
    }

    #[test]
    fn validate_empty_then_user() {
        assert!(validate_append_message(&mk_user("hi"), &[]).is_ok());
    }

    #[test]
    fn validate_empty_then_tool() {
        let r = validate_append_message(&mk_tool("c1"), &[]);
        assert!(r.is_err(), "tool as first entry should fail");
    }

    #[test]
    fn validate_empty_then_assistant_tc() {
        assert!(validate_append_message(&mk_assistant_tc(&["c1"]), &[]).is_ok());
    }

    #[test]
    fn validate_user_then_tool() {
        let recent = vec![mk_user("q")];
        let r = validate_append_message(&mk_tool("c1"), &recent);
        assert!(r.is_err());
    }

    #[test]
    fn validate_assistant_tc_then_matching_tool() {
        let recent = vec![mk_assistant_tc(&["c1", "c2"])];
        assert!(validate_append_message(&mk_tool("c1"), &recent).is_ok());
    }

    #[test]
    fn validate_assistant_tc_then_mismatched_tool() {
        let recent = vec![mk_assistant_tc(&["c1"])];
        let r = validate_append_message(&mk_tool("c99"), &recent);
        assert!(r.is_err());
    }

    #[test]
    fn validate_tool_missing_tool_call_id() {
        let recent = vec![mk_assistant_tc(&["c1"])];
        let bad = serde_json::json!({ "role": "tool", "content": "ok" });
        assert!(validate_append_message(&bad, &recent).is_err());
    }

    #[test]
    fn validate_duplicate_tool_call_id() {
        let recent = vec![mk_assistant_tc(&["c1", "c2"]), mk_tool("c1")];
        let r = validate_append_message(&mk_tool("c1"), &recent);
        assert!(r.is_err(), "duplicate tool_call_id should fail");
    }

    #[test]
    fn validate_assistant_tc_then_assistant() {
        let recent = vec![mk_assistant_tc(&["c1"])];
        assert!(validate_append_message(&mk_assistant("hi"), &recent).is_err());
        assert!(validate_append_message(&mk_assistant_tc(&["c2"]), &recent).is_err());
    }

    #[test]
    fn validate_tool_then_plain_assistant() {
        let recent = vec![mk_assistant_tc(&["c1"]), mk_tool("c1")];
        assert!(validate_append_message(&mk_assistant("done"), &recent).is_ok());
    }

    #[test]
    fn validate_bad_tool_calls_shape() {
        let bad = serde_json::json!({
            "role": "assistant",
            "tool_calls": [{ "id": "c1", "type": "function", "function": {} }]
        });
        assert!(validate_append_message(&bad, &[]).is_err());
    }

    #[test]
    fn validate_pending_tool_round_then_user() {
        let recent = vec![mk_assistant_tc(&["c1"])];
        assert!(validate_append_message(&mk_user("q"), &recent).is_err());
    }

    #[test]
    fn validate_pending_tool_round_then_system() {
        let recent = vec![mk_assistant_tc(&["c1"])];
        assert!(validate_append_message(&mk_system("sys"), &recent).is_err());
    }

    #[test]
    fn validate_partial_tool_round_then_user() {
        let recent = vec![mk_assistant_tc(&["c1", "c2"]), mk_tool("c1")];
        assert!(validate_append_message(&mk_user("q"), &recent).is_err());
    }

    #[test]
    fn validate_complete_tool_round_then_user() {
        let recent = vec![mk_assistant_tc(&["c1", "c2"]), mk_tool("c1"), mk_tool("c2")];
        assert!(validate_append_message(&mk_user("q"), &recent).is_ok());
    }

    #[test]
    fn validate_unknown_role() {
        let bad = serde_json::json!({ "role": "function", "content": "x" });
        assert!(validate_append_message(&bad, &[]).is_err());
    }

    #[test]
    fn validate_complete_round_then_user() {
        let recent = vec![
            mk_user("q"),
            mk_assistant_tc(&["c1"]),
            mk_tool("c1"),
            mk_assistant("done"),
        ];
        assert!(validate_append_message(&mk_user("next"), &recent).is_ok());
    }

    #[test]
    fn validate_multi_tool_consecutive() {
        let recent = vec![
            mk_assistant_tc(&["c1", "c2", "c3"]),
            mk_tool("c1"),
            mk_tool("c2"),
        ];
        assert!(validate_append_message(&mk_tool("c3"), &recent).is_ok());
    }

    #[test]
    fn collect_skips_non_message() {
        let entries = vec![
            TranscriptEntry::Message(MessageEntry {
                id: Some("1".into()),
                parent_id: None,
                timestamp: "t".into(),
                message: mk_user("a"),
            }),
            TranscriptEntry::Compaction(CompactionEntry {
                id: None,
                parent_id: None,
                timestamp: "t".into(),
                summary: Some("s".into()),
                covered_start_id: None,
                covered_end_id: None,
                covered_count: None,
                is_boundary: None,
            }),
            TranscriptEntry::Message(MessageEntry {
                id: Some("2".into()),
                parent_id: None,
                timestamp: "t".into(),
                message: mk_assistant("b"),
            }),
        ];
        let msgs = collect_recent_chat_messages_from_tail(&entries);
        assert_eq!(msgs.len(), 2);
        assert_eq!(msgs[0]["role"], "user");
        assert_eq!(msgs[1]["role"], "assistant");
    }

    #[test]
    fn pending_tool_round_detection() {
        assert!(is_in_pending_tool_round(&[mk_assistant_tc(&["c1"])]));
        assert!(is_in_pending_tool_round(&[
            mk_assistant_tc(&["c1", "c2"]),
            mk_tool("c1")
        ]));
        assert!(!is_in_pending_tool_round(&[
            mk_assistant_tc(&["c1"]),
            mk_tool("c1")
        ]));
        assert!(!is_in_pending_tool_round(&[]));
        assert!(!is_in_pending_tool_round(&[mk_user("q")]));
    }
}
