use super::super::*;
use crate::SessionEntry;
use std::io::{Read, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

fn spawn_single_response_server(
    status: u16,
    body: &'static str,
) -> (String, Arc<AtomicUsize>, std::thread::JoinHandle<()>) {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(AtomicUsize::new(0));
    let hits_clone = Arc::clone(&hits);
    let handle = std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let _ = stream.set_read_timeout(Some(Duration::from_secs(3)));
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            hits_clone.fetch_add(1, Ordering::SeqCst);
            let reason = match status {
                200 => "OK",
                404 => "Not Found",
                _ => "Unknown",
            };
            let resp = format!(
                "HTTP/1.1 {} {}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status,
                reason,
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    (format!("http://{}", addr), hits, handle)
}

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
    assert!(names.contains(&"read".to_string()));
    assert!(!names.contains(&"read_file".to_string()));
    assert!(names.contains(&"write".to_string()));
    assert!(!names.contains(&"write_file".to_string()));
    assert!(names.contains(&"edit".to_string()));
    assert!(!names.contains(&"edit_file".to_string()));
    assert!(names.contains(&"bash".to_string()));
    assert!(!names.contains(&"execute_bash".to_string()));
    assert!(names.contains(&"list_dir".to_string()));
}

#[test]
fn build_tool_definitions_contains_config_tools() {
    let defs = build_tool_definitions();
    let names: Vec<String> = defs
        .iter()
        .filter_map(|d| d["function"]["name"].as_str().map(String::from))
        .collect();
    assert!(
        names.contains(&"config_get".to_string()),
        "config_get tool must be registered (PR-7)"
    );
    assert!(
        names.contains(&"config_set".to_string()),
        "config_set tool must be registered (PR-7)"
    );
}

#[test]
fn chat_message_assistant_with_tool_calls_has_tool_calls() {
    use crate::ChatMessage;
    let tc_json = vec![serde_json::json!({
        "id": "call_1",
        "type": "function",
        "function": {
            "name": "read",
            "arguments": r#"{"path":"/tmp/x"}"#
        }
    })];
    let msg = ChatMessage::assistant_with_tool_calls(Some("thinking..."), tc_json);
    assert!(msg.tool_calls.is_some());
    let tc_val = msg.tool_calls.as_ref().unwrap();
    assert_eq!(tc_val.len(), 1);
    assert_eq!(tc_val[0]["function"]["name"], "read");
}

#[test]
fn chat_message_assistant_tool_calls_null_content_when_empty() {
    use crate::ChatMessage;
    let tc_json = vec![serde_json::json!({
        "id": "call_2",
        "type": "function",
        "function": {
            "name": "list_dir",
            "arguments": r#"{"path":"."}"#
        }
    })];
    let msg = ChatMessage::assistant_with_tool_calls(None, tc_json);
    assert!(msg.content.is_none());
    assert!(msg.tool_calls.is_some());
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

/// T-017 硬验收：`AgentRunOutcome::Interrupted` 的持久化路径必须与 `Completed`
/// 一致——partial assistant + 已完成 tool_result 均落到 transcript JSONL。
///
/// 本测试不启动完整 `chat_loop`（依赖 rustyline / runtime），而是锁定
/// `chat_loop` 中"Completed/Interrupted 共用 `append_message` 循环"这一契约：
/// 给定 `AgentRunResult.new_messages`，SessionManager.append_message 能按
/// 顺序把每条消息 append 到 JSONL，读回后内容 / 角色完全对得上。
#[test]
fn interrupt_persists_transcript_hard_ack() {
    use crate::core::agent_loop::AgentRunResult;
    use crate::core::llm::ChatMessage;
    use std::io::{BufRead, BufReader};

    let dir = tempfile::tempdir().unwrap();
    let mgr = SessionManager::new(dir.path().to_path_buf());
    let key = mgr.current_session_key();
    mgr.create_session(key, None).unwrap();

    // 模拟中断时 AgentLoop::run 返回的 Interrupted 载荷：
    // - 1 条 partial assistant（承载 content_buf 截至中断的 delta）
    // - 1 条已完成的 tool_result（对应中断前已收到的 tool call）
    let tc_json = vec![serde_json::json!({
        "id": "call_1",
        "type": "function",
        "function": { "name": "read", "arguments": r#"{"path":"/x"}"# }
    })];
    let partial = AgentRunResult {
        final_text: "thinking about foo...".to_string(),
        new_messages: vec![
            ChatMessage::assistant_with_tool_calls(Some("thinking about foo..."), tc_json),
            ChatMessage::tool("call_1", "result_of_read"),
        ],
    };

    // 模拟 chat_loop 中 Completed/Interrupted 共用的持久化循环：
    for msg in &partial.new_messages {
        let json = serde_json::to_value(msg).expect("msg serialize");
        mgr.append_message(json).expect("append_message");
    }

    let path = mgr
        .current_transcript_path()
        .unwrap()
        .expect("transcript should exist");
    let file = std::fs::File::open(&path).expect("open transcript");
    let lines: Vec<String> = BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter(|l| !l.trim().is_empty())
        .collect();

    assert!(
        lines.len() >= 2,
        "transcript 应至少含 2 行（assistant + tool），实际 {} 行",
        lines.len()
    );

    let last_two: Vec<serde_json::Value> = lines
        .iter()
        .rev()
        .take(2)
        .map(|l| serde_json::from_str::<serde_json::Value>(l).unwrap())
        .collect();
    // TranscriptEntry 顶层 wrap 了 Message 类型，实际 ChatMessage 在 .message 下
    let tool_msg = last_two[0].get("message").unwrap();
    let assistant_msg = last_two[1].get("message").unwrap();

    assert_eq!(
        assistant_msg
            .get("role")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "assistant",
        "倒数第二行应为 partial assistant"
    );
    assert!(
        assistant_msg
            .get("content")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .contains("thinking about foo"),
        "partial assistant 应含 content_buf 累积文本"
    );
    assert_eq!(
        tool_msg.get("role").and_then(|v| v.as_str()).unwrap_or(""),
        "tool",
        "最后一行应为已完成 tool_result（中断前 tool 已跑完）"
    );
    assert_eq!(
        tool_msg
            .get("tool_call_id")
            .and_then(|v| v.as_str())
            .unwrap_or(""),
        "call_1",
        "tool_call_id 应与 assistant 发起的调用匹配"
    );
}

#[tokio::test]
async fn chat_cleanup_on_session_end_handles_delete_404_idempotently() {
    let (base_url, hits, handle) = spawn_single_response_server(404, r#"{"error":"not found"}"#);
    let old_no_proxy = std::env::var("NO_PROXY").ok();
    let old_no_proxy_lower = std::env::var("no_proxy").ok();
    // SAFETY: 测试作用域内确保本地 mock 地址不走代理，避免 127.0.0.1 请求被外部代理改写。
    unsafe {
        std::env::set_var("NO_PROXY", "127.0.0.1,localhost");
        std::env::set_var("no_proxy", "127.0.0.1,localhost");
    }
    let mut cfg = AppConfig::default();
    let dir = tempfile::tempdir().unwrap();
    cfg.storage.work_dir = Some(dir.path().to_string_lossy().to_string());
    cfg.llm.api_base = Some(base_url);
    cfg.llm.api_key_env = Some("TOMCAT_CHAT_CLEANUP_TEST_KEY".to_string());
    // SAFETY: 测试内部临时设置 env，结束后立即清理。
    unsafe { std::env::set_var("TOMCAT_CHAT_CLEANUP_TEST_KEY", "stub") };

    let ctx = ChatContext::from_config(cfg).expect("chat context should be created");
    let runtime = ctx
        .openai_files_runtime
        .as_ref()
        .expect("openai-responses should expose files runtime");
    runtime.enqueue_delete("file-chat-cleanup".to_string(), Some(10), Some(1), "test");
    assert!(runtime.pending_cleanup_count() >= 1);

    cleanup_openai_files_on_session_end(&ctx, "chat_test_end").await;
    assert_eq!(
        runtime.pending_cleanup_count(),
        0,
        "404 删除应按幂等成功清空队列"
    );
    assert_eq!(hits.load(Ordering::SeqCst), 1, "应发起 1 次 DELETE");
    handle.join().unwrap();
    // SAFETY: 清理测试环境变量。
    unsafe {
        std::env::remove_var("TOMCAT_CHAT_CLEANUP_TEST_KEY");
        match old_no_proxy {
            Some(v) => std::env::set_var("NO_PROXY", v),
            None => std::env::remove_var("NO_PROXY"),
        }
        match old_no_proxy_lower {
            Some(v) => std::env::set_var("no_proxy", v),
            None => std::env::remove_var("no_proxy"),
        }
    };
}
