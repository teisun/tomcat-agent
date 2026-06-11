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

/// 从尾部消息序列中找出尚未闭合的 tool_call_ids（若尾巴合法或无法安全判断则返回 None）。
///
/// 语义约束与 hydrate 自愈保持一致：
/// - 只在尾部是 `assistant.tool_calls` / `tool*` 连续块时返回缺失 ids；
/// - 若尾部中间夹杂 `user/assistant(without tool_calls)/system` 等非 tool 序列，返回 None；
/// - 返回顺序与 owning assistant 的 `tool_calls` 顺序一致。
pub(crate) fn find_dangling_tail_tool_call_ids(recent: &[Value]) -> Option<Vec<String>> {
    let mut trailing_tool_ids_rev: Vec<&str> = Vec::new();
    for msg in recent.iter().rev() {
        let role = msg.get("role").and_then(|v| v.as_str()).unwrap_or("");
        match role {
            "tool" => {
                let tool_call_id = msg.get("tool_call_id").and_then(|v| v.as_str())?;
                trailing_tool_ids_rev.push(tool_call_id);
            }
            "assistant" => {
                let tool_calls = msg.get("tool_calls")?.as_array()?;
                if tool_calls.is_empty() {
                    return None;
                }
                let tool_call_ids: Vec<&str> = tool_calls
                    .iter()
                    .map(|tc| tc.get("id").and_then(|v| v.as_str()))
                    .collect::<Option<Vec<_>>>()?;
                let trailing_tool_ids: Vec<&str> =
                    trailing_tool_ids_rev.iter().rev().copied().collect();
                for (expected, actual) in tool_call_ids.iter().zip(trailing_tool_ids.iter()) {
                    if expected != actual {
                        return None;
                    }
                }
                let missing: Vec<String> = tool_call_ids
                    .iter()
                    .skip(trailing_tool_ids.len())
                    .map(|id| (*id).to_string())
                    .collect();
                return (!missing.is_empty()).then_some(missing);
            }
            _ => return None,
        }
    }
    None
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
    let prev_ok =
        prev_role == "tool" || (prev_role == "assistant" && has_nonempty_tool_calls(prev));
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
        let arguments = func_obj
            .get("arguments")
            .and_then(|v| v.as_str())
            .ok_or(format!(
                "tool_calls[{i}].function.arguments is missing or not a string"
            ))?;
        serde_json::from_str::<Value>(arguments).map_err(|err| {
            format!("tool_calls[{i}].function.arguments is not valid JSON: {err}")
        })?;
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
#[path = "tests/append_message_chain_test.rs"]
mod tests;
