//! Opt-in fault controls for the shared HTTP fixture, never a production route.
//!
//! Raw responses are deliberate: an rmcp server sends SSE headers immediately,
//! which cannot reproduce a blocked response header or a partial JSON body.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::body::Body;
use axum::extract::State;
use axum::http::{header::CONTENT_TYPE, HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use bytes::Bytes;
use futures_util::{stream, StreamExt};
use serde::Serialize;
use serde_json::{json, Value};
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Clone, Default)]
struct Fixture(Arc<Mutex<FixtureData>>);

#[derive(Default)]
struct FixtureData {
    generation: u64,
    initialized: usize,
    init_failures: usize,
    init_status: Option<u16>,
    list_failures: usize,
    list_status: Option<u16>,
    list_gate: Option<String>,
    extra_tool: Option<String>,
    sessions: BTreeSet<String>,
    init_gate: Option<String>,
    cancel_gate: Option<String>,
    protocol: Option<String>,
    notification_stream: bool,
    active_notification_streams: usize,
    gates: BTreeMap<String, CancellationToken>,
    calls: Vec<Call>,
    requests: Vec<Value>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Call {
    id: Value,
    name: String,
    label: String,
    session: String,
    progress_token: Value,
    cancellations: usize,
    progress: usize,
    completed: bool,
    active_response: bool,
    #[serde(skip)]
    cancelled: CancellationToken,
}

struct ResponseGuard(Fixture, usize);
impl Drop for ResponseGuard {
    fn drop(&mut self) {
        self.0 .0.lock().expect("fixture lock").calls[self.1].active_response = false;
    }
}

impl Fixture {
    fn gate(&self, name: Option<&str>) -> CancellationToken {
        match name {
            Some(name) => self
                .0
                .lock()
                .expect("fixture lock")
                .gates
                .entry(name.into())
                .or_default()
                .clone(),
            None => {
                let ready = CancellationToken::new();
                ready.cancel();
                ready
            }
        }
    }
}

pub fn router() -> Router {
    Router::new()
        .route("/mcp", post(mcp).get(notifications).delete(delete_session))
        .route("/__test/state", get(snapshot))
        .route("/__test/control", post(control))
        .with_state(Fixture::default())
}

struct NotificationGuard(Fixture);
impl Drop for NotificationGuard {
    fn drop(&mut self) {
        self.0
             .0
            .lock()
            .expect("fixture lock")
            .active_notification_streams -= 1;
    }
}

async fn notifications(State(state): State<Fixture>) -> Response {
    {
        let mut data = state.0.lock().expect("fixture lock");
        if !data.notification_stream {
            return StatusCode::METHOD_NOT_ALLOWED.into_response();
        }
        data.active_notification_streams += 1;
    }
    let guard = NotificationGuard(state);
    let body = stream::once(async {
        Ok::<_, std::io::Error>(Bytes::from_static(b": idle notification stream\n\n"))
    })
    .chain(stream::pending())
    .map(move |item| {
        let _ = &guard;
        item
    });
    (
        [(CONTENT_TYPE, "text/event-stream")],
        Body::from_stream(body),
    )
        .into_response()
}

async fn delete_session(State(state): State<Fixture>, headers: HeaderMap) -> StatusCode {
    if let Some(session) = headers
        .get("mcp-session-id")
        .and_then(|value| value.to_str().ok())
    {
        state
            .0
            .lock()
            .expect("fixture lock")
            .sessions
            .remove(session);
    }
    StatusCode::NO_CONTENT
}

async fn snapshot(State(state): State<Fixture>) -> Json<Value> {
    let state = state.0.lock().expect("fixture lock");
    Json(json!({
        "generation": state.generation,
        "initialized": state.initialized,
        "calls": state.calls,
        "requests": state.requests,
        "activeNotificationStreams": state.active_notification_streams,
        "activeResponses": state.calls.iter().filter(|call| call.active_response).count(),
    }))
}

async fn control(State(state): State<Fixture>, Json(command): Json<Value>) -> StatusCode {
    let mut data = state.0.lock().expect("fixture lock");
    if let Some(enabled) = command["notificationStream"].as_bool() {
        data.notification_stream = enabled;
    }
    if let Some(gate) = command["release"].as_str() {
        data.gates.entry(gate.into()).or_default().cancel();
    }
    if command["expireSession"].as_bool() == Some(true) {
        data.generation += 1;
        data.sessions.clear();
    }
    if let Some(failures) = command["initFailures"].as_u64() {
        data.init_failures = failures as usize;
    }
    if let Some(status) = command["initStatus"].as_u64() {
        data.init_status = Some(status as u16);
    }
    if let Some(status) = command["listStatus"].as_u64() {
        data.list_status = Some(status as u16);
    }
    if let Some(failures) = command["listFailures"].as_u64() {
        data.list_failures = failures as usize;
    }
    if let Some(value) = command.get("listGate") {
        data.list_gate = value.as_str().map(str::to_owned);
    }
    if let Some(value) = command.get("extraTool") {
        data.extra_tool = value.as_str().map(str::to_owned);
    }
    if let Some(protocol) = command["protocolVersion"].as_str() {
        data.protocol = Some(protocol.into());
    }
    if let Some(value) = command.get("initGate") {
        data.init_gate = value.as_str().map(str::to_owned);
    }
    if let Some(value) = command.get("cancelGate") {
        data.cancel_gate = value.as_str().map(str::to_owned);
    }
    StatusCode::NO_CONTENT
}

async fn mcp(
    State(state): State<Fixture>,
    headers: HeaderMap,
    Json(request): Json<Value>,
) -> Response {
    let method = request["method"].as_str().unwrap_or_default();
    let id = request["id"].clone();
    state.0.lock().expect("fixture lock").requests.push(json!({
        "method": method, "id": id,
        "session": headers.get("mcp-session-id").and_then(|value| value.to_str().ok())
    }));
    if method == "initialize" || method == "server/discover" {
        let (gate, failed, generation, protocol, sequence, status) = {
            let mut data = state.0.lock().expect("fixture lock");
            data.initialized += 1;
            let failed = data.init_failures > 0;
            data.init_failures = data.init_failures.saturating_sub(1);
            (
                data.init_gate.clone(),
                failed,
                data.generation,
                data.protocol.clone().unwrap_or_else(|| "2025-11-25".into()),
                data.initialized,
                data.init_status.unwrap_or(503),
            )
        };
        state.gate(gate.as_deref()).cancelled().await;
        if failed {
            return (
                StatusCode::from_u16(status).unwrap(),
                "SECRET_STARTUP_RESPONSE",
            )
                .into_response();
        }
        if method == "server/discover" {
            let result = rmcp::model::DiscoverResult::new(
                vec![rmcp::model::ProtocolVersion::V_2026_07_28],
                rmcp::model::ServerCapabilities::builder()
                    .enable_tools()
                    .build(),
            );
            return Json(json!({"jsonrpc":"2.0", "id":id, "result":result})).into_response();
        }
        let session = format!("fixture-{generation}-{sequence}");
        {
            let mut data = state.0.lock().expect("fixture lock");
            if data.generation == generation {
                data.sessions.insert(session.clone());
            }
        }
        return (
            [("mcp-session-id", session)],
            Json(json!({"jsonrpc":"2.0", "id": id, "result": {
                "protocolVersion": protocol,
                "capabilities": {"tools": {}},
                "serverInfo": {"name":"tomcat-fault-fixture", "version":"1.0.0"}
            }})),
        )
            .into_response();
    }
    let session = headers
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("");
    if !session.is_empty()
        && !state
            .0
            .lock()
            .expect("fixture lock")
            .sessions
            .contains(session)
    {
        return StatusCode::NOT_FOUND.into_response();
    }
    if method == "notifications/cancelled" {
        let gate = {
            let mut data = state.0.lock().expect("fixture lock");
            for call in &mut data.calls {
                if call.id == request["params"]["requestId"] && call.session == session {
                    call.cancellations += 1;
                    call.cancelled.cancel();
                }
            }
            data.cancel_gate.clone()
        };
        state.gate(gate.as_deref()).cancelled().await;
        return StatusCode::ACCEPTED.into_response();
    }
    if method.starts_with("notifications/") {
        return StatusCode::ACCEPTED.into_response();
    }
    if method == "tools/list" {
        let (gate, failed, status) = {
            let mut data = state.0.lock().expect("fixture lock");
            let failed = data.list_failures > 0;
            data.list_failures = data.list_failures.saturating_sub(1);
            (
                data.list_gate.clone(),
                failed,
                data.list_status.unwrap_or(503),
            )
        };
        state.gate(gate.as_deref()).cancelled().await;
        if failed {
            return (
                StatusCode::from_u16(status).unwrap(),
                "SECRET_CATALOG_RESPONSE",
            )
                .into_response();
        }
        let extra_tool = state.0.lock().expect("fixture lock").extra_tool.clone();
        let tools: Vec<_> = ["echo", "echo_other"]
            .into_iter()
            .chain(extra_tool.as_deref())
            .map(|name| {
                json!({
                    "name": name,
                    "description": "Controlled MCP test tool",
                    "inputSchema": {"type":"object", "properties": {"label":{"type":"string"}}}
                })
            })
            .collect();
        return Json(json!({"jsonrpc":"2.0", "id":id, "result":{"tools":tools}})).into_response();
    }
    if method == "resources/list" || method == "ping" {
        let result = if method == "resources/list" {
            json!({"resources":[]})
        } else {
            json!({})
        };
        return Json(json!({"jsonrpc":"2.0", "id":id, "result":result})).into_response();
    }
    if method != "tools/call" {
        return Json(
            json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32601,"message":"unknown method"}}),
        )
        .into_response();
    }

    let args = &request["params"]["arguments"];
    let label = args["label"].as_str().unwrap_or("unlabelled").to_owned();
    let cancelled = CancellationToken::new();
    let token = request["params"]["_meta"]["progressToken"].clone();
    let index = {
        let mut data = state.0.lock().expect("fixture lock");
        let index = data.calls.len();
        data.calls.push(Call {
            id: id.clone(),
            name: request["params"]["name"].as_str().unwrap_or("").into(),
            label: label.clone(),
            session: session.into(),
            progress_token: token.clone(),
            cancellations: 0,
            progress: 0,
            completed: false,
            active_response: true,
            cancelled: cancelled.clone(),
        });
        index
    };
    let guard = ResponseGuard(state.clone(), index);
    if let Some(status) = args["httpStatus"].as_u64() {
        let mut response = (
            StatusCode::from_u16(status as u16).unwrap(),
            args["responseBody"]
                .as_str()
                .unwrap_or("fixture HTTP failure")
                .to_owned(),
        )
            .into_response();
        if let Some(challenge) = args["challenge"].as_str() {
            response.headers_mut().insert(
                axum::http::header::WWW_AUTHENTICATE,
                challenge.parse().unwrap(),
            );
        }
        drop(guard);
        return response;
    }
    if args["malformed"].as_bool() == Some(true) {
        drop(guard);
        return ([(CONTENT_TYPE, "application/json")], "SECRET_RESPONSE_BODY").into_response();
    }
    let gate = state.gate(args["gate"].as_str());
    let ready_at = Instant::now() + Duration::from_millis(args["delayMs"].as_u64().unwrap_or(0));
    let ignore_cancel = args["ignoreCancellation"].as_bool().unwrap_or(false);
    let result = if args["rpcError"].as_bool() == Some(true) {
        json!({"jsonrpc":"2.0", "id":id, "error":{"code":-32602,"message":"fixture business error"}})
    } else {
        json!({"jsonrpc":"2.0", "id":id, "result": {
            "content":[{"type":"text", "text":format!("fixture result: {label}")}],
            "isError":args["businessError"].as_bool().unwrap_or(false)
        }})
    };
    let shape = args["response"].as_str().unwrap_or("sse");
    if shape == "headers" {
        tokio::select! {
            _ = async { gate.cancelled().await; tokio::time::sleep_until(ready_at).await; } => {}
            _ = cancelled.cancelled(), if !ignore_cancel => {
                return StatusCode::ACCEPTED.into_response();
            }
        }
        state.0.lock().expect("fixture lock").calls[index].completed = true;
        drop(guard);
        return Json(result).into_response();
    }

    let json_body = shape == "json";
    let pulse_ms = args["progressMs"].as_u64().filter(|v| *v > 0);
    let progress_token = if args["wrongToken"].as_bool() == Some(true) {
        json!("unrelated-progress-token")
    } else {
        token
    };
    let heartbeat = args["heartbeat"].as_bool().unwrap_or(false);
    let truncate = args["truncate"].as_bool().unwrap_or(false);
    let data = ResponseStream {
        state,
        index,
        _guard: guard,
        gate,
        ready_at,
        cancelled,
        ignore_cancel,
        result,
        json_body,
        pulse_ms,
        progress_token,
        heartbeat,
        truncate,
        next_pulse: Instant::now() + Duration::from_millis(pulse_ms.unwrap_or(1)),
        finished: false,
    };
    let first = if json_body {
        "{\"jsonrpc\":\"2.0\","
    } else {
        ": open\n\n"
    };
    let body = stream::once(async move { Ok::<_, std::io::Error>(Bytes::from_static(first.as_bytes())) })
        .chain(stream::unfold(data, |mut data| async move {
            if data.finished { return None; }
            tokio::select! {
                _ = async { data.gate.cancelled().await; tokio::time::sleep_until(data.ready_at).await; } => {
                    data.finished = true;
                    if data.truncate { return None; }
                    data.state.0.lock().expect("fixture lock").calls[data.index].completed = true;
                    let chunk = if data.json_body {
                        let mut object = data.result.as_object().expect("response object").clone();
                        object.remove("jsonrpc");
                        serde_json::to_string(&object).expect("JSON")[1..].to_owned()
                    } else { format!("event: message\ndata: {}\n\n", data.result) };
                    Some((Ok(Bytes::from(chunk)), data))
                }
                _ = data.cancelled.cancelled(), if !data.ignore_cancel => None,
                _ = tokio::time::sleep_until(data.next_pulse), if data.pulse_ms.is_some() && !data.json_body => {
                    data.next_pulse = Instant::now() + Duration::from_millis(data.pulse_ms.expect("enabled"));
                    let chunk = if data.heartbeat { ": heartbeat\n\n".to_owned() } else {
                        let count = {
                            let mut state = data.state.0.lock().expect("fixture lock");
                            state.calls[data.index].progress += 1;
                            state.calls[data.index].progress
                        };
                        format!("event: message\ndata: {}\n\n", json!({
                            "jsonrpc":"2.0", "method":"notifications/progress", "params":{
                                "progressToken":data.progress_token, "progress":count
                            }
                        }))
                    };
                    Some((Ok(Bytes::from(chunk)), data))
                }
            }
        }));
    (
        [(
            CONTENT_TYPE,
            if json_body {
                "application/json"
            } else {
                "text/event-stream"
            },
        )],
        Body::from_stream(body),
    )
        .into_response()
}

struct ResponseStream {
    state: Fixture,
    index: usize,
    _guard: ResponseGuard,
    gate: CancellationToken,
    ready_at: Instant,
    cancelled: CancellationToken,
    ignore_cancel: bool,
    result: Value,
    json_body: bool,
    pulse_ms: Option<u64>,
    next_pulse: Instant,
    progress_token: Value,
    heartbeat: bool,
    truncate: bool,
    finished: bool,
}
