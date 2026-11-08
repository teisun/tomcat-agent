//! Local shared-service discovery, startup and explicit shutdown. No discovery file or token.
use std::fs::{File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::time::Duration;

use fs2::FileExt;
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};
use tokio::sync::watch;
use tokio::task::JoinSet;

use super::{
    runtime::ServeRuntime,
    writer::{spawn_writer, WriterConfig},
    ServeState,
};
use crate::api::cli::ServeSub;
use crate::infra::platform::{
    self,
    local_socket::{self, LocalListener, LocalStream},
};
use crate::{AppConfig, AppError};

// Incompatible changes to ConnectionHello, ServiceInfo or ACP's private close records
// require a version bump. Optional fields with compatible defaults may keep this version.
// The unpublished Native environment exchange is identified by message shape, not this
// version, so a new CLI can still status/stop an old same-version service build.
pub(crate) const LOCAL_PROTOCOL_VERSION: u32 = 1;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const START_TIMEOUT: Duration = Duration::from_secs(20);
// Includes the existing 30s checkpoint flush plus turn/child/MCP shutdown.
const STOP_TIMEOUT: Duration = Duration::from_secs(60);
const HELLO_MAX_BYTES: u64 = 1024 * 1024;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum ConnectionProtocol {
    Native,
    Acp,
    Status,
    Stop,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionHello {
    pub service: String,
    pub protocol_version: u32,
    pub client_version: String,
    pub protocol: ConnectionProtocol,
    #[serde(default)]
    pub environment: platform::EnvironmentSnapshot,
    #[serde(default)]
    pub skills_dir: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub acp_options: Option<crate::api::acp::AcpLaunchOptions>,
}

#[derive(Deserialize)]
struct NativeEnvironment {
    environment: platform::EnvironmentSnapshot,
}

impl ConnectionHello {
    pub(super) fn new(protocol: ConnectionProtocol) -> Self {
        Self {
            service: "tomcat".into(),
            protocol_version: LOCAL_PROTOCOL_VERSION,
            client_version: env!("CARGO_PKG_VERSION").into(),
            protocol,
            environment: Default::default(),
            skills_dir: None,
            acp_options: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ServiceInfo {
    pub service: String,
    pub protocol_version: u32,
    pub server_version: String,
    pub instance_id: String,
    pub pid: u32,
    pub agent_id: String,
    pub endpoint: PathBuf,
    pub running: bool,
}

pub(crate) fn service_directory(cfg: &AppConfig) -> Result<PathBuf, AppError> {
    Ok(crate::infra::config::resolve_agent_trail_dir(cfg)?.join("serve"))
}

fn print_json(value: &impl Serialize) -> Result<(), AppError> {
    println!(
        "{}",
        serde_json::to_string(value).map_err(AppError::Serialize)?
    );
    Ok(())
}

pub(crate) fn run(action: ServeSub, cfg: &AppConfig) -> Result<(), AppError> {
    tokio::runtime::Runtime::new()?.block_on(async {
        match action {
            ServeSub::Start { foreground: false } => print_json(&ensure_running(cfg).await?),
            ServeSub::Start { foreground: true } => {
                if let Some(info) = probe_service(cfg).await? {
                    return print_json(&info);
                }
                let dir = service_directory(cfg)?;
                local_socket::prepare_directory(&dir)?;
                let mut command = service_command(cfg)?;
                command.current_dir(dir).stdin(Stdio::null());
                let status = tokio::process::Command::from(command).status().await?;
                if !status.success() { return Err(AppError::Config(format!("foreground service exited: {status}"))); }
                Ok(())
            }
            ServeSub::Status => {
                match probe_service(cfg).await? {
                    Some(info) => print_json(&info),
                    None => print_json(&serde_json::json!({"running": false, "endpoint": local_socket::endpoint(&service_directory(cfg)?)?})),
                }
            }
            ServeSub::Stop => {
                let stopped = stop(cfg).await?;
                print_json(&serde_json::json!({"running":false, "stopped":stopped.is_some(), "instanceId":stopped}))
            }
            ServeSub::Restart => {
                stop(cfg).await.inspect_err(|error| tracing::warn!(%error, "service restart: stop phase failed"))?;
                print_json(&ensure_running(cfg).await.inspect_err(|error| tracing::warn!(%error, "service restart: start phase failed"))?)
            }
            ServeSub::Models => unreachable!("models handled before runtime bootstrap"),
            ServeSub::Run { .. } => run_service(cfg).await,
        }
    })
}

/// The connecting client's identity, resolved before a service changes its working directory.
pub(crate) fn client_identity(cfg: &AppConfig) -> Result<(PathBuf, String), AppError> {
    Ok((crate::get_work_dir(cfg)?, cfg.agent.id.clone()))
}

/// Service startup and offline catalog discovery deliberately ignore ambient model/key overrides.
pub(crate) fn load_saved_configuration(
    config_path: Option<&Path>,
    work_dir: &Path,
    agent_id: &str,
) -> Result<(AppConfig, Vec<String>), AppError> {
    if !work_dir.is_absolute() {
        return Err(AppError::Config(
            "internal service data directory must be absolute".into(),
        ));
    }
    crate::infra::config::use_saved_configuration_only();
    let (mut cfg, ignored) = crate::infra::config::load_config_with_ignored_keys(config_path)?;
    cfg.storage.work_dir = Some(work_dir.to_string_lossy().into_owned());
    cfg.agent.id = agent_id.to_owned();
    Ok((cfg, ignored))
}

pub(super) fn service_command(cfg: &AppConfig) -> Result<Command, AppError> {
    let mut command = Command::new(std::env::current_exe()?);
    // Shared HTTP clients use saved configuration or system proxy discovery.
    // Never let the first starter's shell proxy choose for every connection.
    for key in [
        "HTTP_PROXY",
        "HTTPS_PROXY",
        "ALL_PROXY",
        "NO_PROXY",
        "http_proxy",
        "https_proxy",
        "all_proxy",
        "no_proxy",
    ] {
        command.env_remove(key);
    }
    // Carry the resolved data directory across the cwd change, including relative configurations.
    let (work_dir, agent_id) = client_identity(cfg)?;
    command
        .args(["serve", "run", "--work-dir"])
        .arg(work_dir)
        .arg("--agent-id")
        .arg(agent_id);
    Ok(command)
}

/// Every starter and every child rechecks under this lock. A killed starter cannot leave
/// its child binding without a lock; lock files are NEVER deleted or replaced.
async fn startup_lock(directory: &Path) -> Result<File, AppError> {
    let path = directory.join("serve.lock");
    let mut options = OpenOptions::new();
    options.create_new(true).read(true).write(true);
    platform::set_create_mode(&mut options, 0o600);
    let file = match options.open(&path) {
        Ok(file) => {
            platform::set_private_file_permissions(&path)?;
            file
        }
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => platform::open_no_follow(&path)?,
        Err(e) => return Err(e.into()),
    };
    if !file.metadata()?.is_file() {
        return Err(AppError::Config(
            "service lock must be a regular file".into(),
        ));
    }
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    loop {
        match file.try_lock_exclusive() {
            Ok(()) => return Ok(file),
            Err(e) if platform::is_lock_contended(&e) => {
                if tokio::time::Instant::now() >= deadline {
                    return Err(AppError::Config(
                        "shared service startup lock timed out".into(),
                    ));
                }
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// A retiring endpoint can accept a connection then close before replying. Retry only
/// read-only discovery, within the original connect + hello budget. Never treat a
/// broken handshake as absence, or replay Stop/ACP/Native requests.
async fn probe_service(cfg: &AppConfig) -> Result<Option<ServiceInfo>, AppError> {
    let probe = async {
        loop {
            match open_management(cfg, ConnectionHello::new(ConnectionProtocol::Status)).await {
                Ok(reply) => return Ok(reply.map(|(_, info)| info)),
                Err(AppError::Io(error))
                    if matches!(
                        error.kind(),
                        io::ErrorKind::UnexpectedEof
                            | io::ErrorKind::ConnectionReset
                            | io::ErrorKind::NotConnected
                            | io::ErrorKind::BrokenPipe
                    ) => {}
                Err(error) => return Err(error),
            }
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
    };
    tokio::time::timeout(CONNECT_TIMEOUT * 2, probe)
        .await
        .map_err(|_| {
            AppError::Config(
                "local service discovery timed out; refusing to start another instance".into(),
            )
        })?
}

pub(crate) async fn ensure_running(cfg: &AppConfig) -> Result<ServiceInfo, AppError> {
    if let Some(info) = probe_service(cfg).await? {
        return Ok(info);
    }
    let directory = service_directory(cfg)?;
    local_socket::prepare_directory(&directory)?;
    let lock = startup_lock(&directory).await?;
    if let Some(info) = probe_service(cfg).await? {
        return Ok(info);
    }
    // Only the eventual listener, under its OWN lock, removes a stale socket.
    let log_path = directory.join("serve.log");
    let mut options = OpenOptions::new();
    options.create(true).append(true);
    platform::set_create_mode(&mut options, 0o600);
    let log = options.open(&log_path)?;
    platform::set_private_file_permissions(&log_path)?;
    let mut command = service_command(cfg)?;
    command
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(log);
    let mut child = platform::spawn_service_process(&mut command, &directory).map_err(|e| {
        AppError::Config(format!(
            "could not detach shared service: {e}; if the host prohibits Windows Job breakaway, \
             run \"tomcat serve start\" from an independent terminal (or open VS Code), then retry"
        ))
    })?;
    drop(lock); // The child reacquires it before probing/cleaning/binding.
    wait_for_started_service(cfg, &mut child, &log_path).await
}

async fn wait_for_started_service(
    cfg: &AppConfig,
    child: &mut std::process::Child,
    log_path: &Path,
) -> Result<ServiceInfo, AppError> {
    let deadline = tokio::time::Instant::now() + START_TIMEOUT;
    loop {
        let observed = probe_service(cfg).await?;
        let exited = child.try_wait()?;
        if let Some(info) = observed {
            // A concurrent starter may have won. Do not report success while our
            // losing child can still acquire the lock and resurrect a later stop.
            if info.pid == child.id() || exited.is_some() {
                return Ok(info);
            }
        } else if let Some(status) = exited {
            return Err(AppError::Config(format!(
                "shared service exited during startup ({status}); see {}",
                log_path.display()
            )));
        }
        if tokio::time::Instant::now() >= deadline {
            return Err(AppError::Config(format!(
                "shared service startup timed out; see {}",
                log_path.display()
            )));
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

pub(crate) async fn connect_acp(
    cfg: &AppConfig,
    environment: platform::EnvironmentSnapshot,
    skills_dir: Option<PathBuf>,
    acp_options: Option<crate::api::acp::AcpLaunchOptions>,
) -> Result<BufReader<LocalStream>, AppError> {
    if let Some(directory) = skills_dir.as_deref() {
        crate::core::skill::discovery::validate_connection_root(directory)?;
    }
    ensure_running(cfg).await?;
    let (stream, _) = open_management(
        cfg,
        ConnectionHello {
            environment,
            skills_dir,
            acp_options,
            ..ConnectionHello::new(ConnectionProtocol::Acp)
        },
    )
    .await?
    .ok_or_else(|| AppError::Config("shared service disappeared before ACP connection".into()))?;
    Ok(stream)
}

async fn open_management(
    cfg: &AppConfig,
    hello: ConnectionHello,
) -> Result<Option<(BufReader<LocalStream>, ServiceInfo)>, AppError> {
    let address = local_socket::endpoint(&service_directory(cfg)?)?;
    let stream = match tokio::time::timeout(CONNECT_TIMEOUT, local_socket::connect(&address)).await
    {
        Ok(Ok(stream)) => stream,
        Ok(Err(e))
            if matches!(
                e.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
            ) =>
        {
            return Ok(None)
        }
        Ok(Err(e)) => return Err(e.into()),
        Err(_) => {
            return Err(AppError::Config(
                "local service connection timed out; refusing to start another instance".into(),
            ))
        }
    };
    let mut stream = BufReader::new(stream);
    let info: ServiceInfo = tokio::time::timeout(CONNECT_TIMEOUT, async {
        write_json(&mut stream, &hello).await?;
        let reply: serde_json::Value = read_json(&mut stream).await?;
        if let Some(error) = reply.get("error").and_then(|v| v.as_str()) {
            return Err(AppError::Config(error.to_owned()));
        }
        if let Some(expected) = hello.acp_options {
            let accepted = reply.get("acpOptions").cloned().and_then(|value| {
                serde_json::from_value::<crate::api::acp::AcpLaunchOptions>(value).ok()
            });
            if accepted != Some(expected) {
                return Err(AppError::Config(
                    concat!(
                        "shared service cannot accept ACP settings: ",
                        "use `tomcat serve restart` to upgrade; all tasks will stop"
                    )
                    .into(),
                ));
            }
        }
        serde_json::from_value(reply).map_err(AppError::Serialize)
    })
    .await
    .map_err(|_| {
        AppError::Config(
            "local service handshake timed out; refusing to start another instance".into(),
        )
    })??;
    if info.service != "tomcat"
        || info.endpoint != address
        || info.agent_id != cfg.agent.id
        || info.protocol_version != LOCAL_PROTOCOL_VERSION
    {
        return Err(AppError::Config(
            "local service identity/protocol mismatch; refusing to start another instance".into(),
        ));
    }
    Ok(Some((stream, info)))
}

async fn stop(cfg: &AppConfig) -> Result<Option<String>, AppError> {
    let Some((mut stream, info)) = open_management(
        cfg,
        ConnectionHello::new(ConnectionProtocol::Stop),
    )
    .await
    .inspect_err(
        |error| tracing::warn!(%error, "service stop: connection or identity handshake failed"),
    )?
    else {
        return Ok(None);
    };
    let result: serde_json::Value = tokio::time::timeout(STOP_TIMEOUT, read_json(&mut stream))
        .await
        .map_err(|_| {
            AppError::Config(
                "shared service shutdown timed out; task cleanup is not confirmed".into(),
            )
        })?
        .inspect_err(
            |error| tracing::warn!(%error, "service stop: cleanup acknowledgement failed"),
        )?;
    if result.get("stopped").and_then(|v| v.as_bool()) != Some(true) {
        return Err(AppError::Config(format!(
            "shared service cleanup failed: {result}"
        )));
    }
    Ok(Some(info.instance_id))
}

async fn run_service(cfg: &AppConfig) -> Result<(), AppError> {
    let directory = service_directory(cfg)?;
    local_socket::prepare_directory(&directory)?;
    if platform::canonicalize(std::env::current_dir()?)? != platform::canonicalize(&directory)? {
        return Err(AppError::Config(
            "internal service process must start in its data directory; use `tomcat serve start`"
                .into(),
        ));
    }
    let lock = startup_lock(&directory).await?;
    if let Some(info) = probe_service(cfg).await? {
        return print_json(&info);
    }
    let address = local_socket::endpoint(&directory)?;
    local_socket::remove_stale_endpoint(&address)?;
    let runtime = ServeRuntime::new(
        cfg.clone(),
        crate::api::cli::config_file_path()?,
        super::build_shared_model_prefs(cfg)?,
        true,
    )?;
    let listener = LocalListener::bind(&address)?;
    let info = ServiceInfo {
        service: "tomcat".into(),
        protocol_version: LOCAL_PROTOCOL_VERSION,
        server_version: env!("CARGO_PKG_VERSION").into(),
        instance_id: uuid::Uuid::new_v4().to_string(),
        pid: std::process::id(),
        agent_id: cfg.agent.id.clone(),
        endpoint: address,
        running: true,
    };
    drop(lock); // Binding/listen completed; a live probe now protects this endpoint.
    print_json(&info)?;
    run_listener(runtime, listener, info).await
}

async fn run_listener(
    runtime: Arc<ServeRuntime>,
    mut listener: LocalListener,
    info: ServiceInfo,
) -> Result<(), AppError> {
    let (finished_tx, finished_rx) = watch::channel::<Option<Result<(), String>>>(None);
    let mut connections = JoinSet::new();
    let signal = async {
        if let Err(error) = local_socket::termination_signal().await {
            tracing::warn!(%error, "service console signal unavailable; local stop remains available");
            std::future::pending::<()>().await;
        }
    };
    tokio::pin!(signal);
    let accept_result = loop {
        tokio::select! {
            _ = runtime.shutdown.cancelled() => break Ok(()),
            _ = &mut signal => break Ok(()),
            joined = connections.join_next(), if !connections.is_empty() => {
                if let Some(Err(error)) = joined { tracing::error!(%error, "service connection panicked"); }
            }
            accepted = listener.accept() => {
                let stream = match accepted {
                    Ok(stream) => stream,
                    Err(e) if e.kind() == io::ErrorKind::PermissionDenied => { tracing::warn!(%e, "local peer rejected"); continue; }
                    Err(e) => { tracing::warn!(%e, "local service accept failed"); break Err(AppError::Io(e)); }
                };
                let runtime = runtime.clone();
                let info = info.clone();
                let finished = finished_rx.clone();
                connections.spawn(async move {
                    if let Err(error) = handle_connection(runtime, stream, info, finished).await { tracing::debug!(%error, "service connection ended"); }
                });
            }
        }
    };
    // Keep the endpoint bound until cleanup completes: a concurrent start must not
    // mistake a stopping service for a stale socket and begin work alongside it.
    for connection in runtime
        .connections
        .read()
        .values()
        .filter_map(std::sync::Weak::upgrade)
    {
        let _ = connection.writer.send(super::types::OutFrame::Event(
            serde_json::json!({"type":"service_stopping"}),
        ));
    }
    let cleanup = runtime.stop_all().await;
    drop(listener); // A late listener drop never unlinks a replacement socket.
    finished_tx.send_replace(Some(
        cleanup.as_ref().map(|_| ()).map_err(ToString::to_string),
    ));
    // Task cleanup is complete; broken/idle handshakes must not hold process shutdown open.
    if tokio::time::timeout(Duration::from_secs(2), async {
        while connections.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        connections.abort_all();
        while connections.join_next().await.is_some() {}
    }
    cleanup.and(accept_result)
}

async fn handle_connection(
    runtime: Arc<ServeRuntime>,
    stream: LocalStream,
    info: ServiceInfo,
    mut finished: watch::Receiver<Option<Result<(), String>>>,
) -> Result<(), AppError> {
    let mut stream = BufReader::new(stream);
    let deadline = tokio::time::Instant::now() + CONNECT_TIMEOUT;
    let record: serde_json::Value = tokio::time::timeout_at(deadline, read_json(&mut stream))
        .await
        .map_err(|_| AppError::Config("local handshake timed out".into()))??;
    let carries_environment = record.get("environment").is_some();
    let hello: ConnectionHello = serde_json::from_value(record).map_err(AppError::Serialize)?;
    let error = if hello.service != "tomcat" || hello.protocol_version != LOCAL_PROTOCOL_VERSION {
        Some("local_protocol_mismatch")
    } else if hello.protocol == ConnectionProtocol::Native && carries_environment {
        Some("native_handshake_outdated")
    } else if hello.environment.validate().is_err() {
        Some("invalid_connection_environment")
    } else if hello.skills_dir.as_deref().is_some_and(|directory| {
        hello.protocol != ConnectionProtocol::Acp
            || crate::core::skill::discovery::validate_connection_root(directory).is_err()
    }) {
        Some("invalid_connection_skills_directory")
    } else if hello.protocol == ConnectionProtocol::Native
        && hello.client_version != info.server_version
    {
        Some("service_version_mismatch: use `tomcat serve restart` to upgrade; all tasks will stop")
    } else {
        None
    };
    let mut reply = serde_json::to_value(&info).map_err(AppError::Serialize)?;
    if let Some(error) = error {
        reply["error"] = error.into();
    }
    if let Some(options) = hello.acp_options {
        reply["acpOptions"] = serde_json::to_value(options)?;
    }
    write_json(&mut stream, &reply).await?;
    if error.is_some() {
        return Ok(());
    }
    match hello.protocol {
        ConnectionProtocol::Status => Ok(()),
        ConnectionProtocol::Stop => {
            runtime.shutdown.cancel();
            let result = finished
                .wait_for(|result| result.is_some())
                .await
                .map_err(|_| AppError::Config("service cleanup result unavailable".into()))?
                .clone()
                .expect("wait predicate checked completion");
            let reply = match result {
                Ok(()) => serde_json::json!({"stopped":true}),
                Err(error) => serde_json::json!({"stopped":false,"error":error}),
            };
            write_json(&mut stream, &reply).await
        }
        ConnectionProtocol::Acp => {
            tracing::debug!(client_version = ?hello.client_version, "ACP transport connected");
            let (read, write) = tokio::io::split(stream);
            crate::api::acp::run_connection(
                runtime,
                BufReader::new(read),
                write,
                hello.environment,
                hello.skills_dir,
                hello.acp_options,
            )
            .await
        }
        ConnectionProtocol::Native => {
            let environment =
                tokio::time::timeout_at(deadline, read_json::<NativeEnvironment>(&mut stream))
                    .await
                    .map_err(|_| AppError::Config("local handshake timed out".into()))?;
            let environment = match environment {
                Ok(record) if record.environment.validate().is_ok() => record.environment,
                _ => {
                    // Do not echo parsing errors or values from a credential-bearing record.
                    write_json(
                        &mut stream,
                        &serde_json::json!({"error":"invalid_connection_environment"}),
                    )
                    .await?;
                    return Ok(());
                }
            };
            write_json(&mut stream, &serde_json::json!({"ready":true})).await?;
            let (read, write) = tokio::io::split(stream);
            let writer = spawn_writer(Box::pin(write), WriterConfig::from(&runtime.cfg.serve));
            let connection = ServeState::connect_with_source(
                runtime,
                writer.clone(),
                None,
                environment,
                None,
                None,
            );
            let result = super::stdin::run_connection_loop(connection, BufReader::new(read)).await;
            writer.close();
            result
        }
    }
}

async fn read_json<T: DeserializeOwned>(
    reader: &mut (impl AsyncBufRead + Unpin),
) -> Result<T, AppError> {
    let mut line = Vec::new();
    (&mut *reader)
        .take(HELLO_MAX_BYTES + 1)
        .read_until(b'\n', &mut line)
        .await?;
    if line.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::UnexpectedEof,
            "local service closed before reply",
        )
        .into());
    }
    if line.len() as u64 > HELLO_MAX_BYTES || !line.ends_with(b"\n") {
        return Err(AppError::Config(
            "invalid/oversized local service handshake".into(),
        ));
    }
    serde_json::from_slice(&line).map_err(AppError::Serialize)
}

#[cfg(test)]
#[path = "tests/service_test.rs"]
mod tests;

async fn write_json(
    writer: &mut (impl AsyncWrite + Unpin),
    value: &impl Serialize,
) -> Result<(), AppError> {
    let mut bytes = serde_json::to_vec(value).map_err(AppError::Serialize)?;
    bytes.push(b'\n');
    writer.write_all(&bytes).await?;
    writer.flush().await?;
    Ok(())
}
