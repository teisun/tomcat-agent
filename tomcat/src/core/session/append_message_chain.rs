//! OpenAI Chat Completions 消息链：落盘前校验（规则 A–E）与从 transcript tail 收集连续 Message 内层 JSON。

use serde_json::Value;

use crate::core::llm::ChatMessage;

use super::manager::PENDING_TOOL_RESULT_TEXT;
use super::transcript::TranscriptEntry;

/// A durable ask_question result that may be replaced by the recovery path. Keeping this
/// classification in one place prevents hydrate, resume, and append-gate code from drifting on
/// the legacy `host_disconnected` compatibility rule.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ResumableAskQuestionResult {
    Pending,
    LegacyHostDisconnected,
}

pub(crate) fn classify_resumable_ask_question_result(
    content: &str,
) -> Option<ResumableAskQuestionResult> {
    if content == PENDING_TOOL_RESULT_TEXT {
        return Some(ResumableAskQuestionResult::Pending);
    }
    let payload = serde_json::from_str::<Value>(content).ok()?;
    (payload.get("outcome").and_then(Value::as_str) == Some("host_disconnected")
        && payload.get("cancelled").and_then(Value::as_bool) == Some(true)
        && payload
            .get("answers")
            .and_then(Value::as_array)
            .is_some_and(|answers| answers.is_empty()))
    .then_some(ResumableAskQuestionResult::LegacyHostDisconnected)
}

/// 从 transcript 尾部条目中收集连续的 Message 内层 `message` 对象（旧→新）。
pub(crate) fn collect_recent_chat_messages_from_tail(entries: &[TranscriptEntry]) -> Vec<Value> {
    let mut msgs: Vec<Value> = entries
        .iter()
        .rev()
        .filter_map(|e| {
            if let TranscriptEntry::Message(me) = e {
                // Hydration already excludes soft-deleted transcript messages. Append
                // validation must consume the identical logical chain; otherwise it can
                // reject a valid append based on a result that the next LLM request will
                // never see.
                (me.message.get("superseded").and_then(Value::as_bool) != Some(true))
                    .then(|| me.message.clone())
            } else {
                None
            }
        })
        .collect();
    msgs.reverse();
    msgs
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct DanglingToolCall {
    pub id: String,
    pub name: String,
    /// 已由 assistant message chain 校验过的函数参数，供可恢复工具在 restart 后重放。
    /// 缺失/损坏的历史参数不得影响普通工具既有的 interrupted 收口。
    pub arguments: Option<Value>,
}

/// 从尾部消息序列中找出尚未闭合的 tool calls（若尾巴合法或无法安全判断则返回 None）。
///
/// 语义约束与 hydrate 自愈保持一致：
/// - 只在尾部是 `assistant.tool_calls` / `tool*` 连续块时返回缺失 ids；
/// - 若尾部中间夹杂 `user/assistant(without tool_calls)/system` 等非 tool 序列，返回 None；
/// - 返回顺序与 owning assistant 的 `tool_calls` 顺序一致。
pub(crate) fn find_dangling_tail_tool_calls(recent: &[Value]) -> Option<Vec<DanglingToolCall>> {
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
                let missing: Vec<DanglingToolCall> = tool_calls
                    .iter()
                    .skip(trailing_tool_ids.len())
                    .map(|tool_call| {
                        Some(DanglingToolCall {
                            id: tool_call.get("id")?.as_str()?.to_string(),
                            name: tool_call
                                .get("function")?
                                .get("name")?
                                .as_str()?
                                .to_string(),
                            arguments: tool_call
                                .get("function")
                                .and_then(|function| function.get("arguments"))
                                .and_then(Value::as_str)
                                .and_then(|arguments| serde_json::from_str(arguments).ok()),
                        })
                    })
                    .collect::<Option<Vec<_>>>()?;
                return (!missing.is_empty()).then_some(missing);
            }
            _ => return None,
        }
    }
    None
}

pub(crate) fn find_dangling_tail_tool_call_ids(recent: &[Value]) -> Option<Vec<String>> {
    find_dangling_tail_tool_calls(recent)
        .map(|calls| calls.into_iter().map(|call| call.id).collect())
}

fn is_tool_media(message: &Value) -> bool {
    message.get("role").and_then(Value::as_str) == Some("user")
        && message.get("kind").and_then(Value::as_str) == Some("tool_media")
}

/// 当前 transcript 是否以一组完整、仍有效的 tool results 收尾。
///
/// `Resume` 是“模型已拿到工具结果、只差继续作答”的专用入口，因此不能只看投影后的
/// 最后一条 active message：失败的 user 输入可能已被 supersede，而更早一轮恰好以 tool
/// result 收尾。先要求原始 transcript 的最后一个 message 本身就是 active tool result，
/// 再在统一的 active-message 投影中校验 owning assistant 的每个 call 都恰好有一个、
/// 顺序匹配的 result。
pub(crate) fn has_complete_tail_tool_results(entries: &[TranscriptEntry]) -> bool {
    let Some(TranscriptEntry::Message(last_raw_message)) = entries
        .iter()
        .rev()
        .find(|entry| matches!(entry, TranscriptEntry::Message(m) if !is_tool_media(&m.message)))
    else {
        return false;
    };
    if last_raw_message.message.get("role").and_then(Value::as_str) != Some("tool")
        || last_raw_message
            .message
            .get("superseded")
            .and_then(Value::as_bool)
            == Some(true)
    {
        return false;
    }

    let recent = collect_recent_chat_messages_from_tail(entries);
    let mut trailing_results_rev = Vec::new();
    for message in recent.iter().rev().filter(|m| !is_tool_media(m)) {
        match message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "tool" => trailing_results_rev.push(message),
            "assistant" => {
                let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) else {
                    return false;
                };
                if tool_calls.is_empty() || tool_calls.len() != trailing_results_rev.len() {
                    return false;
                }
                return tool_calls
                    .iter()
                    .zip(trailing_results_rev.iter().rev())
                    .all(|(call, result)| {
                        call.get("id").and_then(Value::as_str)
                            == result.get("tool_call_id").and_then(Value::as_str)
                    });
            }
            _ => return false,
        }
    }
    false
}

/// LLM 出站前的最终协议守卫：禁止把未配对的 tool call 发给 provider。
///
/// 正常路径会在 hydrate 时先补终态或 `[pending]`，因此这里触发代表某条新入口绕过了
/// session 恢复层。它宁可让本轮明确失败，也不能把 provider 必然拒绝的非法链发出去。
pub(crate) fn has_dangling_tool_calls_in_messages(messages: &[ChatMessage]) -> bool {
    let wire_messages = messages
        .iter()
        .filter_map(|message| serde_json::to_value(message).ok())
        .collect::<Vec<_>>();
    find_dangling_tail_tool_calls(&wire_messages).is_some()
}

/// 返回尾部完整 tool round 中仍为 `[pending]` 的可续跑调用。
///
/// 调用方在追加新的 user message 前把这些问题结算为 `skipped`；它只识别完整配对的
/// tail，绝不猜测已经部分执行的普通工具。
pub(crate) fn pending_replay_safe_tail_tool_call_ids(recent: &[Value]) -> Vec<String> {
    let mut trailing_tools = Vec::new();
    for message in recent.iter().rev() {
        match message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or_default()
        {
            "tool" => trailing_tools.push(message),
            "assistant" => {
                let Some(tool_calls) = message.get("tool_calls").and_then(Value::as_array) else {
                    return Vec::new();
                };
                if tool_calls.is_empty() || tool_calls.len() != trailing_tools.len() {
                    return Vec::new();
                }
                trailing_tools.reverse();
                return tool_calls
                    .iter()
                    .zip(trailing_tools)
                    .filter_map(|(tool_call, result)| {
                        let tool_call_id = tool_call.get("id").and_then(Value::as_str)?;
                        let tool_name = tool_call.get("function")?.get("name")?.as_str()?;
                        (result.get("tool_call_id").and_then(Value::as_str) == Some(tool_call_id)
                            && result.get("content").and_then(Value::as_str)
                                == Some(PENDING_TOOL_RESULT_TEXT)
                            && crate::core::tools::contract::catalog::is_replay_safe_tool(
                                tool_name,
                            ))
                        .then(|| tool_call_id.to_string())
                    })
                    .collect();
            }
            _ => return Vec::new(),
        }
    }
    Vec::new()
}

/// 指定调用的最新有效 tool result 是否仍是恢复占位 `[pending]`。
///
/// `superseded` 结果不是逻辑链的一部分，必须跳过；一旦后续真实结果存在，它会成为
/// 最新有效结果并自然关闭问题。这是 Rust 侧判断「问题是否仍开着」的唯一入口。
pub(crate) fn is_tool_call_pending(entries: &[TranscriptEntry], tool_call_id: &str) -> bool {
    entries
        .iter()
        .rev()
        .find_map(|entry| {
            let TranscriptEntry::Message(message) = entry else {
                return None;
            };
            if message.message.get("superseded").and_then(Value::as_bool) == Some(true) {
                return None;
            }
            (message.message.get("role").and_then(Value::as_str) == Some("tool")
                && message.message.get("tool_call_id").and_then(Value::as_str)
                    == Some(tool_call_id))
            .then(|| {
                matches!(
                    message
                        .message
                        .get("content")
                        .and_then(Value::as_str)
                        .and_then(classify_resumable_ask_question_result),
                    Some(ResumableAskQuestionResult::Pending)
                )
            })
        })
        .unwrap_or(false)
}

/// Transcript 的结构不变量：每个仍有效的 assistant tool call 都有且仅有一条仍有效的
/// tool result。它是 hydrate / pending 替换测试共同使用的验收尺，避免「看起来能继续，
/// 实际发到 provider 会因缺 result 或重复 result 被拒」。
#[cfg(test)]
pub(crate) fn assert_active_tool_result_integrity(
    entries: &[TranscriptEntry],
) -> Result<(), String> {
    let active_messages = entries
        .iter()
        .filter_map(|entry| {
            let TranscriptEntry::Message(message) = entry else {
                return None;
            };
            (message.message.get("superseded").and_then(Value::as_bool) != Some(true))
                .then_some(&message.message)
        })
        .collect::<Vec<_>>();

    for tool_call_id in active_messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("assistant"))
        .flat_map(|message| {
            message
                .get("tool_calls")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
        })
        .filter_map(|tool_call| tool_call.get("id").and_then(Value::as_str))
    {
        let result_count = active_messages
            .iter()
            .filter(|message| {
                message.get("role").and_then(Value::as_str) == Some("tool")
                    && message.get("tool_call_id").and_then(Value::as_str) == Some(tool_call_id)
            })
            .count();
        if result_count != 1 {
            return Err(format!(
                "tool call '{tool_call_id}' has {result_count} active results; expected exactly one"
            ));
        }
    }
    Ok(())
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
        if t.get("superseded").and_then(|value| value.as_bool()) == Some(true) {
            continue;
        }
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
