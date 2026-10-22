//! ACP method handling, using the existing native command/admission workflow.
//! Only outstanding replies live here: no completed-turn cache, replayable requests or retries.

use std::collections::{hash_map::Entry, HashMap};
use std::sync::{atomic::Ordering, Arc};

use agent_client_protocol::{
    schema::v1 as acp, Client, ConnectionTo, Dispatch, HandleDispatchFrom, Handled, UntypedMessage,
};
use parking_lot::Mutex;
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Value;
use tokio::sync::{mpsc, oneshot};

use super::{events, permissions::PermissionBridge};
use crate::api::serve::{stdin::QueuedCommand, types::*, ServeState};
use crate::core::connector::mcp::{config::McpServerConfig, manager::SessionMcpConfig};

pub(super) struct PendingPrompt {
    request_id: String,
    pub completion: Option<oneshot::Sender<acp::Result<acp::PromptResponse>>>,
    pub error: Option<String>,
    cancellation: tokio_util::sync::CancellationToken,
}

pub(super) struct Replies {
    pub commands: Mutex<HashMap<String, oneshot::Sender<ResponseFrame>>>,
    pub prompts: Mutex<HashMap<String, PendingPrompt>>,
    pub permissions: Arc<PermissionBridge>,
}

impl Replies {
    pub fn new(input: mpsc::UnboundedSender<QueuedCommand>) -> Self {
        Self {
            commands: Default::default(),
            prompts: Default::default(),
            permissions: Arc::new(PermissionBridge::new(input)),
        }
    }

    pub fn close(&self) {
        self.commands.lock().clear();
        self.prompts.lock().clear();
        self.permissions.close();
    }

    pub fn response(&self, frame: ResponseFrame) {
        let sender = frame
            .id
            .as_ref()
            .and_then(|id| self.commands.lock().remove(id));
        if let Some(sender) = sender {
            let _ = sender.send(frame);
        }
    }

    pub fn event(&self, event: &Value) {
        let Some(session_id) = event["sessionId"].as_str() else {
            return;
        };
        let mut pending = self.prompts.lock();
        if event["type"] == "agent_idle" {
            if let Some(prompt) = pending.get_mut(session_id) {
                let Some(completion) = prompt.completion.take() else {
                    return;
                };
                let result = match event["outcome"].as_str() {
                    Some("completed") => Ok(acp::PromptResponse::new(acp::StopReason::EndTurn)),
                    Some("interrupted") => Ok(acp::PromptResponse::new(acp::StopReason::Cancelled)),
                    _ => Err(failed(
                        prompt
                            .error
                            .take()
                            .unwrap_or_else(|| "Agent turn failed".into()),
                    )),
                };
                let _ = completion.send(result);
            }
        } else if matches!(event["type"].as_str(), Some("agent_end" | "llm_error")) {
            if let (Some(prompt), Some(error)) = (
                pending.get_mut(session_id),
                event["error"]
                    .as_str()
                    .or_else(|| event["errorMessage"].as_str()),
            ) {
                prompt.error = Some(error.to_owned());
            }
        }
    }
}

#[derive(Clone)]
pub(super) struct Adapter {
    pub state: Arc<ServeState>,
    pub input: mpsc::UnboundedSender<QueuedCommand>,
    pub replies: Arc<Replies>,
}

impl HandleDispatchFrom<Client> for Adapter {
    async fn handle_dispatch_from(
        &mut self,
        message: Dispatch,
        connection: ConnectionTo<Client>,
    ) -> acp::Result<Handled<Dispatch>> {
        self.replies.permissions.attach(connection.clone());
        match message {
            Dispatch::Request(request, responder) if request.method == "initialize" => {
                responder.respond_with_result(self.initialize(request.params))?;
            }
            Dispatch::Request(request, responder) if request.method == "session/prompt" => {
                // Register cancellation in receive order, before spawning any work.
                match decode(request.params).and_then(|request| self.prompt(request)) {
                    Ok(prompt) => {
                        connection.spawn(async move {
                            responder.respond_with_result(prompt.await.and_then(json))
                        })?;
                    }
                    Err(error) => {
                        responder.respond_with_result(Err::<Value, _>(error))?;
                    }
                }
            }
            Dispatch::Request(request, responder) => {
                let adapter = self.clone();
                // Do not block the SDK's input loop: cancel, permission replies and EOF
                // must still be observed while bootstrap or a prompt is awaiting its result.
                connection.spawn(async move {
                    responder.respond_with_result(adapter.request(request).await)
                })?;
            }
            Dispatch::Notification(notification) if notification.method == "session/cancel" => {
                self.ready()?;
                let request: acp::CancelNotification = decode(notification.params)?;
                if let Some(prompt) = self
                    .replies
                    .prompts
                    .lock()
                    .get(&request.session_id.to_string())
                {
                    prompt.cancellation.cancel();
                }
                self.input
                    .send(
                        ServeCommand::Interrupt {
                            id: None,
                            session_id: Some(request.session_id.to_string()),
                        }
                        .into(),
                    )
                    .map_err(|_| failed("connection_closed"))?;
            }
            Dispatch::Notification(_) => {}
            Dispatch::Response(result, router) => {
                router.route_with_result(result)?;
            }
        }
        Ok(Handled::Yes)
    }

    fn describe_chain(&self) -> impl std::fmt::Debug {
        "Tomcat ACP v1"
    }
}

impl Adapter {
    fn ready(&self) -> acp::Result<()> {
        if self.state.lifetime.is_cancelled() {
            return Err(failed("connection_closed"));
        }
        if !self.state.initialized.load(Ordering::Acquire) {
            return Err(failed("not_initialized"));
        }
        Ok(())
    }

    fn initialize(&self, params: Value) -> acp::Result<Value> {
        let request: acp::InitializeRequest = decode(params)?;
        if self.state.initialized.swap(true, Ordering::SeqCst) {
            return Err(invalid("already_initialized"));
        }
        self.state
            .set_acp_client_name(request.client_info.map(|info| info.name));
        let mut capabilities = acp::AgentCapabilities::default();
        capabilities.load_session = true;
        capabilities.mcp_capabilities.http = true;
        capabilities.prompt_capabilities.embedded_context = true;
        json(
            acp::InitializeResponse::new(agent_client_protocol::schema::ProtocolVersion::V1)
                .agent_capabilities(capabilities)
                .agent_info(acp::Implementation::new(
                    "tomcat",
                    env!("CARGO_PKG_VERSION"),
                )),
        )
    }

    async fn request(&self, request: UntypedMessage) -> acp::Result<Value> {
        self.ready()?;
        match request.method.as_str() {
            "session/new" => {
                let request: acp::NewSessionRequest = decode(request.params)?;
                let session_mcp =
                    validate_setup(&request.mcp_servers, &request.additional_directories)?;
                let cwd = request.cwd.to_string_lossy().into_owned();
                let response = self
                    .call(|id| {
                        QueuedCommand::from(ServeCommand::NewSession {
                            id: Some(id),
                            params: NewSessionParams {
                                cwd: Some(cwd),
                                mode: Some(ServeSessionMode::Code),
                                ..Default::default()
                            },
                        })
                        .with_session_mcp(session_mcp)
                    })
                    .await?;
                let id = response
                    .session_id
                    .ok_or_else(|| failed("missing_session_id"))?;
                let options = self.config_options(&id).await?;
                json(acp::NewSessionResponse::new(id).config_options(options))
            }
            "session/load" => {
                let request: acp::LoadSessionRequest = decode(request.params)?;
                let session_mcp =
                    validate_setup(&request.mcp_servers, &request.additional_directories)?;
                let id = request.session_id.to_string();
                self.call(|request_id| {
                    QueuedCommand::from(ServeCommand::LoadExternalSession {
                        id: Some(request_id),
                        session_id: id.clone(),
                        cwd: request.cwd.to_string_lossy().into_owned(),
                    })
                    .with_session_mcp(session_mcp)
                })
                .await?;
                self.replay(&id).await?;
                let options = self.config_options(&id).await?;
                json(acp::LoadSessionResponse::new().config_options(options))
            }
            "session/set_config_option" => {
                let request: acp::SetSessionConfigOptionRequest = decode(request.params)?;
                if request.config_id.to_string() != "model" {
                    return Err(invalid("unknown_config_option"));
                }
                let model = request
                    .value
                    .as_value_id()
                    .ok_or_else(|| invalid("model must be a model ID"))?
                    .to_string();
                let session_id = request.session_id.to_string();
                self.call(|id| ServeCommand::SetModel {
                    id: Some(id),
                    session_id: Some(session_id.clone()),
                    model,
                })
                .await?;
                json(acp::SetSessionConfigOptionResponse::new(
                    self.config_options(&session_id).await?,
                ))
            }
            _ => Err(acp::Error::method_not_found()),
        }
    }

    async fn call<C: Into<QueuedCommand> + Send>(
        &self,
        command: impl FnOnce(String) -> C,
    ) -> acp::Result<ResponseFrame> {
        self.native(command(uuid::Uuid::new_v4().to_string())).await
    }

    async fn native(&self, command: impl Into<QueuedCommand>) -> acp::Result<ResponseFrame> {
        self.ready()?;
        let command = command.into();
        let id = command
            .command
            .command_id()
            .ok_or_else(|| failed("missing_internal_request_id"))?
            .to_owned();
        let (sender, response) = oneshot::channel();
        self.replies.commands.lock().insert(id.clone(), sender);
        if self.input.send(command).is_err() {
            self.replies.commands.lock().remove(&id);
            return Err(failed("connection_closed"));
        }
        let response = response.await.map_err(|_| failed("connection_closed"))?;
        if !response.success {
            return Err(failed(
                response.error.unwrap_or_else(|| "Command failed".into()),
            ));
        }
        Ok(response)
    }

    fn prompt(
        &self,
        request: acp::PromptRequest,
    ) -> acp::Result<
        impl std::future::Future<Output = acp::Result<acp::PromptResponse>> + Send + 'static,
    > {
        self.ready()?;
        let text = prompt_text(request.prompt)?;
        let session_id = request.session_id.to_string();
        let id = uuid::Uuid::new_v4().to_string();
        let (completion, result) = oneshot::channel();
        let cancellation = self.state.lifetime.child_token();
        {
            let mut pending = self.replies.prompts.lock();
            match pending.entry(session_id.clone()) {
                Entry::Occupied(_) => return Err(failed("busy")),
                Entry::Vacant(entry) => {
                    entry.insert(PendingPrompt {
                        request_id: id.clone(),
                        completion: Some(completion),
                        cancellation: cancellation.clone(),
                        error: None,
                    });
                }
            }
        }
        let adapter = self.clone();
        Ok(async move {
            let outcome = async {
                adapter
                    .native(QueuedCommand {
                        command: ServeCommand::Prompt {
                            id: Some(id.clone()),
                            session_id: Some(session_id.clone()),
                            text,
                            params: Default::default(),
                        },
                        turn_token: Some(cancellation),
                        session_mcp: None,
                    })
                    .await?;
                // accepted/agent_end are not completion. Keep this request pending
                // through cancellation cleanup so another prompt cannot overtake it.
                let response = result.await.map_err(|_| failed("connection_closed"))??;
                if response.stop_reason == acp::StopReason::Cancelled {
                    adapter
                        .state
                        .finish_cancelled_turn(&session_id)
                        .await
                        .map_err(|e| failed(e.to_string()))?;
                }
                Ok(response)
            }
            .await;
            let mut pending = adapter.replies.prompts.lock();
            if pending
                .get(&session_id)
                .is_some_and(|prompt| prompt.request_id == id)
            {
                pending.remove(&session_id);
            }
            outcome
        })
    }

    async fn config_options(&self, session_id: &str) -> acp::Result<Vec<acp::SessionConfigOption>> {
        let state = self
            .call(|id| ServeCommand::GetState {
                id: Some(id),
                session_id: Some(session_id.into()),
            })
            .await?
            .payload
            .unwrap_or_default();
        let current = state["model"]
            .as_str()
            .ok_or_else(|| failed("missing_session_model"))?;
        let catalog = self
            .call(|id| ServeCommand::ListModels { id: Some(id) })
            .await?
            .payload
            .unwrap_or_default();
        let choices = catalog["models"]
            .as_array()
            .ok_or_else(|| failed("missing_model_catalog"))?
            .iter()
            .filter_map(|model| {
                model["id"].as_str().map(|id| {
                    acp::SessionConfigSelectOption::new(
                        id.to_owned(),
                        model["name"].as_str().unwrap_or(id),
                    )
                })
            })
            .collect::<Vec<_>>();
        Ok(vec![acp::SessionConfigOption::select(
            "model",
            "Model",
            current.to_owned(),
            choices,
        )
        .category(acp::SessionConfigOptionCategory::Model)])
    }

    async fn history_page(&self, session_id: &str, cursor: Option<String>) -> acp::Result<Value> {
        Ok(self
            .call(|id| ServeCommand::GetMessages {
                id: Some(id),
                session_id: Some(session_id.into()),
                params: GetMessagesParams {
                    limit: Some(64),
                    cursor,
                    attachment_mode: AttachmentMode::Reference,
                    ..Default::default()
                },
            })
            .await?
            .payload
            .unwrap_or_default())
    }

    async fn replay(&self, session_id: &str) -> acp::Result<()> {
        // Native history pages backwards. Keep only its cursors, then reread oldest
        // first; never hold a second full transcript or bypass its existing filters.
        let mut cursors = vec![None];
        loop {
            let page = self
                .history_page(session_id, cursors.last().cloned().flatten())
                .await?;
            if page["hasMore"] != true {
                break;
            }
            let next = page["nextCursor"]
                .as_str()
                .ok_or_else(|| failed("missing_history_cursor"))?
                .to_owned();
            cursors.push(Some(next));
        }
        for cursor in cursors.into_iter().rev() {
            let page = self.history_page(session_id, cursor).await?;
            for entry in page["messages"]
                .as_array()
                .ok_or_else(|| failed("missing_history_messages"))?
            {
                for update in events::history_updates(entry) {
                    let line = events::notification(session_id, update)
                        .map_err(|e| failed(e.to_string()))?;
                    self.state
                        .writer
                        .send(OutFrame::Acp {
                            session_id: Some(session_id.into()),
                            line,
                        })
                        .map_err(|e| failed(e.to_string()))?;
                }
            }
            // The next native page response uses this same session queue: it waits
            // behind this page's replay bytes, bounding output to one page. The final
            // GetState in config_options supplies that barrier for the last page.
        }
        Ok(())
    }
}

fn validate_setup(
    mcp: &[acp::McpServer],
    directories: &[std::path::PathBuf],
) -> acp::Result<SessionMcpConfig> {
    if !directories.is_empty() {
        return Err(invalid("additional_directories is not supported"));
    }
    let mut servers = std::collections::BTreeMap::new();
    for server in mcp {
        let mut config = McpServerConfig {
            command: String::new(),
            args: Vec::new(),
            url: None,
            auth: None,
            headers: Default::default(),
            oauth: None,
            env: Default::default(),
            cwd: None,
            tool_filter: Default::default(),
        };
        let name = match server {
            acp::McpServer::Stdio(server) => {
                config.command = server.command.to_string_lossy().into_owned();
                config.args = server.args.clone();
                config.env = server
                    .env
                    .iter()
                    .map(|e| (e.name.clone(), e.value.clone()))
                    .collect();
                crate::infra::platform::EnvironmentSnapshot::from_values(config.env.clone())
                    .map_err(|e| invalid(e.to_string()))?;
                &server.name
            }
            acp::McpServer::Http(server) => {
                config.url = Some(server.url.clone());
                for header in &server.headers {
                    reqwest::header::HeaderName::try_from(header.name.as_str())
                        .map_err(|_| invalid("invalid MCP HTTP header name"))?;
                    reqwest::header::HeaderValue::try_from(header.value.as_str())
                        .map_err(|_| invalid("invalid MCP HTTP header value"))?;
                }
                config.headers = server
                    .headers
                    .iter()
                    .map(|h| (h.name.clone(), h.value.clone()))
                    .collect();
                &server.name
            }
            _ => {
                return Err(invalid(
                    "unsupported MCP transport; use stdio or Streamable HTTP",
                ))
            }
        };
        config.validate(name).map_err(|e| invalid(e.to_string()))?;
        if servers.insert(name.clone(), config).is_some() {
            return Err(invalid("duplicate session MCP server name"));
        }
    }
    Ok(SessionMcpConfig(servers))
}

fn prompt_text(blocks: Vec<acp::ContentBlock>) -> acp::Result<String> {
    let mut parts = Vec::new();
    for block in blocks {
        match block {
            acp::ContentBlock::Text(text) => parts.push(text.text),
            acp::ContentBlock::ResourceLink(link) => {
                parts.push(format!("Referenced resource: {} ({})", link.name, link.uri))
            }
            acp::ContentBlock::Resource(resource) => match resource.resource {
                acp::EmbeddedResourceResource::TextResourceContents(text) => {
                    parts.push(format!("Resource {}:\n{}", text.uri, text.text))
                }
                _ => return Err(invalid("binary embedded resources are not supported")),
            },
            _ => return Err(invalid("unsupported prompt content type")),
        }
    }
    let text = parts.join("\n");
    if text.trim().is_empty() {
        return Err(invalid("empty prompt"));
    }
    Ok(text)
}

pub(super) fn failed(message: impl Into<String>) -> acp::Error {
    acp::Error::new(-32603, message)
}
fn invalid(message: impl Into<String>) -> acp::Error {
    acp::Error::new(-32602, message)
}
fn decode<T: DeserializeOwned>(value: Value) -> acp::Result<T> {
    serde_json::from_value(value).map_err(|e| invalid(e.to_string()))
}
fn json(value: impl Serialize) -> acp::Result<Value> {
    serde_json::to_value(value).map_err(|e| failed(e.to_string()))
}

#[cfg(test)]
#[path = "tests/setup_test.rs"]
mod tests;
