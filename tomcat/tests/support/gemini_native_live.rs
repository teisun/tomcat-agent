//! Live native-Gemini roundtrip. The loopback relay records JSON bodies, never auth headers.
use super::*;
use std::sync::Mutex;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tomcat::core::llm::{build_provider, Credential, ModelEntry};

struct RecordingRelay {
    base: String,
    requests: Arc<Mutex<Vec<Value>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for RecordingRelay {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl RecordingRelay {
    async fn start(upstream: String, key: String) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let task = tokio::spawn(async move {
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(180))
                .build()
                .unwrap();
            loop {
                let (mut socket, _) = listener.accept().await.unwrap();
                // All callers are the test's reqwest provider; JSON bodies have Content-Length.
                let mut raw = Vec::new();
                let end = loop {
                    let mut chunk = [0; 8192];
                    let n = socket.read(&mut chunk).await.unwrap();
                    if n == 0 {
                        return;
                    }
                    raw.extend_from_slice(&chunk[..n]);
                    if let Some(end) = raw.windows(4).position(|s| s == b"\r\n\r\n") {
                        break end;
                    }
                    assert!(raw.len() < 65536);
                };
                let headers = String::from_utf8_lossy(&raw[..end]);
                let path = headers
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap()
                    .to_string();
                assert!(path.starts_with("/v1beta/models/gemini-4-carbon:"));
                let length: usize = headers
                    .lines()
                    .find_map(|line| {
                        let (name, value) = line.split_once(':')?;
                        name.eq_ignore_ascii_case("content-length")
                            .then(|| value.trim().parse().unwrap())
                    })
                    .expect("Content-Length");
                while raw.len() < end + 4 + length {
                    let mut chunk = [0; 8192];
                    let n = socket.read(&mut chunk).await.unwrap();
                    assert!(n > 0);
                    raw.extend_from_slice(&chunk[..n]);
                }
                let body: Value = serde_json::from_slice(&raw[end + 4..end + 4 + length]).unwrap();
                let request_number = {
                    let mut requests = captured.lock().unwrap();
                    requests.push(body.clone());
                    requests.len()
                };
                let artifacts =
                    std::env::var_os("TOMCAT_REAL_LLM_E2E_ARTIFACTS").map(std::path::PathBuf::from);
                if let Some(dir) = &artifacts {
                    std::fs::create_dir_all(dir).unwrap();
                    std::fs::write(
                        dir.join(format!("request-{request_number}.json")),
                        serde_json::to_vec_pretty(&body).unwrap(),
                    )
                    .unwrap();
                }
                let url = format!("{}{}", upstream, path.strip_prefix("/v1beta").unwrap());
                let response = client
                    .post(url)
                    .header("x-goog-api-key", &key)
                    .json(&body)
                    .send()
                    .await
                    .unwrap();
                let status = response.status().as_u16();
                let content_type = response
                    .headers()
                    .get("content-type")
                    .and_then(|s| s.to_str().ok())
                    .unwrap_or("application/json")
                    .to_string();
                let response_body = response.bytes().await.unwrap();
                if let Some(dir) = &artifacts {
                    std::fs::write(
                        dir.join(format!("response-{request_number}.txt")),
                        &response_body,
                    )
                    .unwrap();
                    std::fs::write(
                        dir.join(format!("response-{request_number}-status.json")),
                        serde_json::to_vec_pretty(&json!({
                            "status": status,
                            "content_type": content_type,
                        }))
                        .unwrap(),
                    )
                    .unwrap();
                }
                let header = format!("HTTP/1.1 {status} Relay\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",response_body.len());
                socket.write_all(header.as_bytes()).await.unwrap();
                socket.write_all(&response_body).await.unwrap();
                socket.shutdown().await.unwrap();
            }
        });
        Self {
            base,
            requests,
            task,
        }
    }
}

#[tokio::test]
#[ignore = "manual: paid native Gemini protocol test, synthetic data only"]
#[serial_test::serial]
async fn gemini_native_roundtrip_replays_parts() -> Result<(), Box<dyn std::error::Error>> {
    assert_eq!(std::env::var("TOMCAT_REAL_LLM_E2E").as_deref(), Ok("1"));
    let models = common::real_models_toml_path();
    let env = common::real_runtime_env_path();
    let fixture = base_continuity_config("gemini_native");
    let mut config = fixture.config.clone();
    common::apply_models_toml_entry_app_config(
        &mut config,
        &models,
        &env,
        "fcodex/gemini-4-carbon",
    )?;
    config.llm.reasoning_continuity.enabled = true;
    config.llm.thinking.enabled = true;
    config.llm.thinking.level = "high".into();
    config.llm.retry_count = 0; // Availability failures must not burn repeated paid requests.
    let path = std::path::Path::new(config.storage.work_dir.as_ref().unwrap()).join("models.toml");
    let catalog: toml::Value = std::fs::read_to_string(path)?.parse()?;
    let mut entry: ModelEntry = catalog["models"][0].clone().try_into()?;
    assert_eq!(entry.api, "google-gemini");
    let key_env = entry.api_key_env.clone().unwrap();
    let credential = Credential {
        provider: entry.provider.clone(),
        env_name: key_env.clone(),
        value: std::env::var(key_env)?,
    };
    let base = entry.base_url.as_ref().unwrap().trim_end_matches('/');
    let upstream = if base.ends_with("/v1beta") || base.ends_with("/v1alpha") {
        base.to_string()
    } else {
        format!("{}/v1beta", base.strip_suffix("/v1").unwrap_or(base))
    };
    let relay = RecordingRelay::start(upstream, credential.value.clone()).await;
    entry.base_url = Some(format!("{}/v1beta", relay.base));
    let provider = build_provider(&entry, &config.llm.runtime(), &credential)?;
    assert_eq!(provider.provider_name(), "google-gemini");
    let mut messages = vec![ChatMessage::user(
        "Use lookup_weather for Tokyo and Paris on tomorrow, making both calls in parallel first. After BOTH results arrive, call lookup_weather once more for Rome tomorrow. After that result, summarize all three in one sentence. Do not skip any call and do not ask questions.")];
    let mut stored_parts = Vec::new();
    let mut counts = Vec::new();
    for step in 0..4 {
        let captured = capture_stream_turn(
            provider.clone(),
            ChatRequest {
                model: entry.id.clone(),
                messages: messages.clone(),
                tools: Some(weather_tool_definitions()),
                max_tokens: Some(8192),
                ..Default::default()
            },
        )
        .await?;
        assert!(matches!(
            captured.finish_reason.as_deref(),
            Some("stop" | "tool_calls")
        ));
        eprintln!(
            "GEMINI_ROUNDTRIP step={step} calls={} signed={} thinking={} finish={:?}",
            captured.tool_calls.len(),
            captured.reasoning_continuation.is_some(),
            captured.thinking_text.is_some(),
            captured.finish_reason
        );
        let continuation = captured
            .reasoning_continuation
            .as_ref()
            .expect("signed native parts required");
        assert!(continuation.opaque_payload.is_array());
        stored_parts.push(continuation.opaque_payload.clone());
        let count = captured.tool_calls.len();
        counts.push(count);
        let assistant = if count == 0 {
            ChatMessage::assistant(captured.assistant_text.clone())
        } else {
            ChatMessage::assistant_with_tool_calls(
                Some(&captured.assistant_text),
                captured.tool_calls.clone(),
            )
        }
        .with_reasoning_state(
            captured.thinking_text,
            captured.reasoning_continuation,
            captured.continuity,
        );
        messages.push(assistant);
        if count == 0 {
            break;
        }
        assert!(step < 3, "tool sequence did not finish");
        for call in captured.tool_calls {
            let args: Value =
                serde_json::from_str(call["function"]["arguments"].as_str().unwrap())?;
            messages.push(ChatMessage::tool(
                call["id"].as_str().unwrap(),
                &json!({"city":args["city"],"day":args["day"],"forecast":"sunny 25C"}).to_string(),
            ));
        }
    }
    assert_eq!(
        counts,
        vec![2, 1, 0],
        "must exercise parallel, sequential and text-only replies"
    );
    let dir = tempfile::tempdir()?;
    let session = SessionManager::new(dir.path().to_path_buf());
    session.create_session(session.current_session_key(), None)?;
    for message in &messages {
        append_chat_message(&session, message)?;
    }
    let restarted = SessionManager::new(dir.path().to_path_buf());
    let mut history = init_context_state(&restarted, &ContextConfig::default(), "sys")?.messages;
    let restored: Vec<Value> = history
        .iter()
        .filter_map(|m| {
            m.reasoning_continuation
                .as_ref()
                .map(|c| c.opaque_payload.clone())
        })
        .collect();
    assert_eq!(restored, stored_parts);
    history.push(ChatMessage::user(
        "Without calling tools, name the three cities again in one short sentence.",
    ));
    let second = capture_stream_turn(
        provider.clone(),
        ChatRequest {
            model: entry.id.clone(),
            messages: history,
            tools: Some(weather_tool_definitions()),
            max_tokens: Some(8192),
            ..Default::default()
        },
    )
    .await?;
    assert_eq!(second.finish_reason.as_deref(), Some("stop"));
    assert!(!second.assistant_text.trim().is_empty());
    let requests = relay.requests.lock().unwrap();
    let second_request = requests.last().unwrap();
    let replayed: Vec<&Value> = second_request["contents"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["role"] == "model")
        .map(|c| &c["parts"])
        .collect();
    assert_eq!(replayed.len(), stored_parts.len());
    for (actual, stored) in replayed.iter().zip(&stored_parts) {
        assert_eq!(serde_json::to_vec(actual)?, serde_json::to_vec(stored)?);
    }
    if let Some(dir) = std::env::var_os("TOMCAT_REAL_LLM_E2E_ARTIFACTS") {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(
            std::path::Path::new(&dir).join("gemini-synthetic-history.json"),
            serde_json::to_vec_pretty(second_request)?,
        )?;
    }
    println!("GEMINI_ROUNDTRIP parallel=2 sequential=1 text=1 restart=true new_user_replay_equal=true cache_read_tokens={:?}",second.cache_read_tokens);
    Ok(())
}
