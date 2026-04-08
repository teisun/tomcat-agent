use super::*;
use crate::SessionEntry;

#[test]
fn build_tool_definitions_is_non_empty() {
    let defs = build_tool_definitions();
    assert!(defs.len() >= 4);
    for d in &defs {
        assert!(d["function"]["name"].is_string());
    }
}

#[test]
fn build_tool_definitions_contains_all_primitives() {
    let defs = build_tool_definitions();
    let names: Vec<String> = defs
        .iter()
        .filter_map(|d| d["function"]["name"].as_str().map(String::from))
        .collect();
    assert!(names.contains(&"read_file".to_string()));
    assert!(names.contains(&"write_file".to_string()));
    assert!(names.contains(&"edit_file".to_string()));
    assert!(names.contains(&"execute_bash".to_string()));
    assert!(names.contains(&"list_dir".to_string()));
}

#[test]
fn convert_to_llm_format_assistant_with_tool_calls() {
    use crate::{convert_to_llm_format, AgentMessage, ToolCallInfo};
    let tcs = vec![ToolCallInfo {
        id: "call_1".into(),
        name: "read_file".into(),
        arguments: r#"{"path":"/tmp/x"}"#.into(),
    }];
    let messages = vec![AgentMessage::Assistant {
        text: "thinking...".into(),
        tool_calls: tcs,
    }];
    let out = convert_to_llm_format(&messages);
    assert_eq!(out.len(), 1);
    assert!(out[0].tool_calls.is_some());
    let tc_val = out[0].tool_calls.as_ref().unwrap();
    assert_eq!(tc_val.len(), 1);
    assert_eq!(tc_val[0]["function"]["name"], "read_file");
}

#[test]
fn convert_to_llm_format_assistant_tool_calls_null_content_when_empty() {
    use crate::{convert_to_llm_format, AgentMessage, ToolCallInfo};
    let tcs = vec![ToolCallInfo {
        id: "call_2".into(),
        name: "list_dir".into(),
        arguments: r#"{"path":"."}"#.into(),
    }];
    let messages = vec![AgentMessage::Assistant {
        text: String::new(),
        tool_calls: tcs,
    }];
    let out = convert_to_llm_format(&messages);
    assert_eq!(out.len(), 1);
    assert!(out[0].content.is_none());
    assert!(out[0].tool_calls.is_some());
}

#[test]
fn effective_model_uses_session_override() {
    let entry = SessionEntry {
        session_id: "s1".into(),
        updated_at: 0,
        session_file: None,
        cwd: None,
        thinking_level: None,
        model_override: Some("gpt-4o".to_string()),
        input_tokens: None,
        output_tokens: None,
        compaction_count: None,
        compaction_tokens_freed: None,
        tool_result_chars_persisted: None,
    };
    let config = AppConfig::default();
    let model = entry
        .model_override
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&config.llm.default_model);
    assert_eq!(model, "gpt-4o");
}

#[test]
fn effective_model_uses_global_when_no_override() {
    let entry = SessionEntry {
        session_id: "s2".into(),
        updated_at: 0,
        session_file: None,
        cwd: None,
        thinking_level: None,
        model_override: None,
        input_tokens: None,
        output_tokens: None,
        compaction_count: None,
        compaction_tokens_freed: None,
        tool_result_chars_persisted: None,
    };
    let config = AppConfig::default();
    let model = entry
        .model_override
        .as_deref()
        .filter(|s| !s.is_empty())
        .unwrap_or(&config.llm.default_model);
    assert_eq!(model, config.llm.default_model);
}

#[test]
fn ensure_session_creates_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let key = mgr.current_session_key();
    assert!(mgr.get_session(key).unwrap().is_none());

    if mgr.get_session(key).unwrap().is_none() {
        mgr.create_session(key, None).unwrap();
    }
    assert!(mgr.get_session(key).unwrap().is_some());
}
