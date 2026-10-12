use serde_json::Value;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum MockRequestClass {
    InitialUserTurn,
    BackgroundToolResult { task_id: String },
    WaitToolResult,
    StopToolResult,
    RecoveryWriteToolResult,
    ExactFollowup,
    PrematureFollowup { actual: String },
    Unexpected { summary: String },
}

pub(super) fn classify_mock_request(request: &str, expected_followup: &str) -> MockRequestClass {
    let Some((_, body)) = request.split_once("\r\n\r\n") else {
        return MockRequestClass::Unexpected {
            summary: "request has no HTTP body".to_string(),
        };
    };
    let Ok(payload) = serde_json::from_str::<Value>(body) else {
        return MockRequestClass::Unexpected {
            summary: "request body is not JSON".to_string(),
        };
    };
    let Some(messages) = payload.get("messages").and_then(Value::as_array) else {
        return MockRequestClass::Unexpected {
            summary: "request has no messages array".to_string(),
        };
    };

    let protocol_messages: Vec<&Value> = messages
        .iter()
        .filter(|message| {
            let content = message
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .trim_start();
            !(message.get("role").and_then(Value::as_str) == Some("user")
                && content.starts_with("<system_reminder"))
        })
        .collect();

    if let Some(last_message) = protocol_messages
        .last()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("tool"))
    {
        let content = last_message
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if let Ok(tool_payload) = serde_json::from_str::<Value>(content) {
            if let Some(task_id) = tool_payload.get("taskId").and_then(Value::as_str) {
                return MockRequestClass::BackgroundToolResult {
                    task_id: task_id.to_string(),
                };
            }
        }
        match last_message.get("tool_call_id").and_then(Value::as_str) {
            Some("call_stop") => return MockRequestClass::StopToolResult,
            Some("call_recovery_write") => return MockRequestClass::RecoveryWriteToolResult,
            Some("call_wait") => return MockRequestClass::WaitToolResult,
            _ => {}
        }
    }

    let user_messages: Vec<&str> = protocol_messages
        .iter()
        .filter(|message| message.get("role").and_then(Value::as_str) == Some("user"))
        .filter_map(|message| message.get("content").and_then(Value::as_str))
        .collect();
    match user_messages.last().copied() {
        Some(content) if content == expected_followup => MockRequestClass::ExactFollowup,
        Some(content) if user_messages.len() >= 2 => MockRequestClass::PrematureFollowup {
            actual: content.to_string(),
        },
        Some(_) => MockRequestClass::InitialUserTurn,
        None => MockRequestClass::Unexpected {
            summary: "request has no user message".to_string(),
        },
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn request(messages: Value) -> String {
        format!(
            "POST /chat HTTP/1.1\r\nContent-Type: application/json\r\n\r\n{}",
            json!({ "messages": messages }),
        )
    }

    #[test]
    fn classifies_checkpoint_requests_by_latest_protocol_message() {
        let cases = [
            (
                "initial",
                json!([{ "role": "user", "content": "run slow tool" }]),
                MockRequestClass::InitialUserTurn,
            ),
            (
                "background result",
                json!([
                    { "role": "user", "content": "run slow tool" },
                    { "role": "tool", "tool_call_id": "call_bg", "content": "{\"taskId\":\"task-7\"}" }
                ]),
                MockRequestClass::BackgroundToolResult {
                    task_id: "task-7".into(),
                },
            ),
            (
                "wait result",
                json!([
                    { "role": "user", "content": "run slow tool" },
                    { "role": "tool", "tool_call_id": "call_wait", "content": "[interrupted]" }
                ]),
                MockRequestClass::WaitToolResult,
            ),
            (
                "exact followup",
                json!([
                    { "role": "user", "content": "run slow tool" },
                    { "role": "tool", "tool_call_id": "call_wait", "content": "[interrupted]" },
                    { "role": "user", "content": "continue after interrupt" }
                ]),
                MockRequestClass::ExactFollowup,
            ),
            (
                "premature followup",
                json!([
                    { "role": "user", "content": "run slow tool" },
                    { "role": "user", "content": "wrong followup" }
                ]),
                MockRequestClass::PrematureFollowup {
                    actual: "wrong followup".into(),
                },
            ),
            (
                "stop result wins over historical wait",
                json!([
                    { "role": "tool", "tool_call_id": "call_wait", "content": "[interrupted]" },
                    { "role": "user", "content": "continue after interrupt" },
                    { "role": "tool", "tool_call_id": "call_stop", "content": "stopped" }
                ]),
                MockRequestClass::StopToolResult,
            ),
            (
                "recovery write result",
                json!([
                    { "role": "tool", "tool_call_id": "call_recovery_write", "content": "wrote file" }
                ]),
                MockRequestClass::RecoveryWriteToolResult,
            ),
        ];

        for (name, messages, expected) in cases {
            assert_eq!(
                classify_mock_request(&request(messages), "continue after interrupt"),
                expected,
                "case={name}",
            );
        }
    }

    #[test]
    fn malformed_requests_are_rejected_instead_of_advancing_state() {
        assert!(matches!(
            classify_mock_request("POST /chat HTTP/1.1\r\n\r\nnot-json", "followup"),
            MockRequestClass::Unexpected { .. }
        ));
    }
}
