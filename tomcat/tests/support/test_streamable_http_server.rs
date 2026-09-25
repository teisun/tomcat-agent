//! Reusable local Streamable HTTP MCP/OAuth fixture.
//!
//! This intentionally mirrors the useful part of Codex's
//! `test_streamable_http_server`: it is a real rmcp HTTP server, but its OAuth
//! authorization endpoint deterministically redirects to the callback so tests
//! never require a human or a real account.

mod streamable_http_faults;

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};

use axum::extract::Request;
use axum::extract::{Form, Query, State};
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::Redirect;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use base64::Engine;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, ContentBlock, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::StreamableHttpServerConfig;
use rmcp::transport::StreamableHttpService;
use rmcp::ErrorData;
use serde::Deserialize;
use serde_json::json;
use sha2::{Digest, Sha256};
use tokio::sync::Mutex;
use tokio_util::sync::CancellationToken;

const ACCESS_TOKEN: &str = "fake-access-token";
const REFRESH_TOKEN: &str = "fake-refresh-token";

#[derive(Clone)]
struct FakeMcpServer;

impl ServerHandler for FakeMcpServer {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_instructions("Deterministic fake MCP server for Tomcat connector tests.")
    }

    async fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::ListToolsResult, ErrorData> {
        Ok(rmcp::model::ListToolsResult {
            tools: vec![Tool::new(
                "echo",
                "Echo a message for connector integration tests.",
                Arc::new(
                    json!({
                        "type": "object",
                        "properties": { "message": { "type": "string" } },
                        "required": ["message"]
                    })
                    .as_object()
                    .expect("object schema")
                    .clone(),
                ),
            )],
            ..Default::default()
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<rmcp::RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let message = request
            .arguments
            .and_then(|mut arguments| arguments.remove("message"))
            .and_then(|value| value.as_str().map(str::to_owned))
            .unwrap_or_default();
        Ok(
            rmcp::model::CallToolResult::success(vec![ContentBlock::text(format!(
                "fake echo: {message}"
            ))])
            .into(),
        )
    }
}

#[derive(Clone, Default)]
struct OAuthState {
    pending: Arc<Mutex<HashMap<String, PendingAuthorization>>>,
    refreshes: Arc<Mutex<usize>>,
    initializations: Arc<AtomicUsize>,
}

#[derive(Clone)]
struct PendingAuthorization {
    redirect_uri: String,
    challenge: String,
}

#[derive(Deserialize)]
struct AuthorizeQuery {
    redirect_uri: String,
    state: String,
    code_challenge: String,
}

#[derive(Deserialize)]
struct TokenForm {
    grant_type: String,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    code_verifier: Option<String>,
    #[serde(default)]
    refresh_token: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let port = std::env::args()
        .skip_while(|arg| arg != "--port")
        .nth(1)
        .and_then(|value| value.parse::<u16>().ok())
        .unwrap_or(0);
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, port)).await?;
    let address = listener.local_addr()?;
    if let Some(path) = std::env::var_os("MCP_STREAMABLE_HTTP_BOUND_ADDR_FILE") {
        std::fs::write(path, address.to_string())?;
    }
    eprintln!("fake Streamable HTTP MCP server: http://{address}/mcp");

    if std::env::args().any(|arg| arg == "--faults") {
        axum::serve(listener, streamable_http_faults::router()).await?;
        return Ok(());
    }

    let oauth = OAuthState::default();
    let discovery_unavailable =
        std::env::var_os("MCP_STREAMABLE_HTTP_DISCOVERY_UNAVAILABLE").is_some();
    let cancellation = CancellationToken::new();
    let service = StreamableHttpService::new(
        || Ok(FakeMcpServer),
        Arc::new(LocalSessionManager::default()),
        StreamableHttpServerConfig::default().with_cancellation_token(cancellation.child_token()),
    );
    let base = format!("http://{address}");
    let metadata_state = oauth.clone();
    let router = Router::new()
        .route(
            "/.well-known/oauth-protected-resource",
            get(move || {
                let base = base.clone();
                let discovery_unavailable = discovery_unavailable;
                async move {
                    if discovery_unavailable {
                        return Err(StatusCode::SERVICE_UNAVAILABLE);
                    }
                    Ok::<_, StatusCode>(axum::Json(json!({
                        "resource": format!("{base}/mcp"),
                        "authorization_servers": [base],
                        "scopes_supported": ["fake:read"]
                    })))
                }
            }),
        )
        .route(
            "/.well-known/oauth-authorization-server",
            get({
                let base = format!("http://{address}");
                move || {
                    let base = base.clone();
                    async move {
                        axum::Json(json!({
                            "issuer": base,
                            "authorization_endpoint": format!("{base}/authorize"),
                            "token_endpoint": format!("{base}/token"),
                            "registration_endpoint": format!("{base}/register"),
                            "code_challenge_methods_supported": ["S256"],
                            "scopes_supported": ["fake:read"]
                        }))
                    }
                }
            }),
        )
        .route("/authorize", get(authorize))
        .route("/token", post(token))
        .route("/register", post(register))
        .nest_service("/mcp", service)
        .layer(middleware::from_fn_with_state(
            oauth.clone(),
            require_mcp_bearer,
        ))
        .with_state(metadata_state);

    axum::serve(listener, router)
        .with_graceful_shutdown(async move { cancellation.cancelled().await })
        .await?;
    Ok(())
}

async fn require_mcp_bearer(
    State(state): State<OAuthState>,
    mut request: Request,
    next: Next,
) -> Response {
    let authorized = request
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        == Some("Bearer fake-access-token");
    if request.uri().path() == "/mcp" && request.method() == axum::http::Method::POST {
        let (parts, body) = request.into_parts();
        let body = match axum::body::to_bytes(body, 16 * 1024 * 1024).await {
            Ok(body) => body,
            Err(_) => return StatusCode::BAD_REQUEST.into_response(),
        };
        let message = serde_json::from_slice::<serde_json::Value>(&body).ok();
        if message
            .as_ref()
            .is_some_and(|message| message["method"] == "initialize")
        {
            let count = state.initializations.fetch_add(1, Ordering::SeqCst) + 1;
            if let Some(path) = std::env::var_os("MCP_STREAMABLE_HTTP_INIT_COUNT_FILE") {
                std::fs::write(path, count.to_string()).expect("record initialize");
            }
            if authorized
                && std::env::var_os("MCP_STREAMABLE_HTTP_FAIL_AUTHENTICATED_STARTUP").is_some()
            {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            }
        }
        request = Request::from_parts(parts, axum::body::Body::from(body));
    }
    if request.uri().path() == "/mcp" && !authorized {
        return (
            StatusCode::UNAUTHORIZED,
            [(
                axum::http::header::WWW_AUTHENTICATE,
                "Bearer resource_metadata=\"/.well-known/oauth-protected-resource\"",
            )],
        )
            .into_response();
    }
    next.run(request).await
}

async fn authorize(
    State(state): State<OAuthState>,
    Query(query): Query<AuthorizeQuery>,
) -> Result<Redirect, StatusCode> {
    state.pending.lock().await.insert(
        query.state.clone(),
        PendingAuthorization {
            redirect_uri: query.redirect_uri.clone(),
            challenge: query.code_challenge,
        },
    );
    Ok(Redirect::temporary(&format!(
        "{}?code=fake-code&state={}",
        query.redirect_uri, query.state
    )))
}

async fn register() -> axum::Json<serde_json::Value> {
    axum::Json(json!({ "client_id": "fake-client", "redirect_uris": ["http://127.0.0.1"] }))
}

async fn token(
    State(state): State<OAuthState>,
    Form(form): Form<TokenForm>,
) -> Result<axum::Json<serde_json::Value>, StatusCode> {
    if form.grant_type == "refresh_token" {
        if form.refresh_token.as_deref() != Some(REFRESH_TOKEN) {
            return Err(StatusCode::UNAUTHORIZED);
        }
        let refresh_count = {
            let mut refreshes = state.refreshes.lock().await;
            *refreshes += 1;
            *refreshes
        };
        if let Some(path) = std::env::var_os("MCP_STREAMABLE_HTTP_REFRESH_COUNT_FILE") {
            std::fs::write(path, refresh_count.to_string())
                .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        }
        return Ok(axum::Json(json!({
            "access_token": ACCESS_TOKEN,
            "refresh_token": REFRESH_TOKEN,
            "expires_in": 3600,
            "token_type": "Bearer"
        })));
    }
    if form.grant_type != "authorization_code" || form.code.as_deref() != Some("fake-code") {
        return Err(StatusCode::BAD_REQUEST);
    }
    let state_value = form.state.as_deref().ok_or(StatusCode::BAD_REQUEST)?;
    let pending = state
        .pending
        .lock()
        .await
        .remove(state_value)
        .ok_or(StatusCode::BAD_REQUEST)?;
    let verifier = form.code_verifier.ok_or(StatusCode::BAD_REQUEST)?;
    let challenge = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .encode(Sha256::digest(verifier.as_bytes()));
    if challenge != pending.challenge || pending.redirect_uri.is_empty() {
        return Err(StatusCode::BAD_REQUEST);
    }
    Ok(axum::Json(json!({
        "access_token": ACCESS_TOKEN,
        "refresh_token": REFRESH_TOKEN,
        "expires_in": 3600,
        "token_type": "Bearer"
    })))
}
