use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;
use tokio::time::sleep;
use tomcat::core::connector::mcp::config::{
    ConfiguredMcpServer, McpConfigSource, McpOAuthConfig, McpServerConfig, ToolFilter,
};
use tomcat::core::connector::mcp::oauth::{authorize, OAuthTokenStore, StoredOAuthToken};
use tomcat::core::connector::mcp::transport::{HttpTransport, McpTransport};
use tomcat::AppConfig;

async fn start_fake_server(
    temp_dir: &Path,
    refresh_count_file: Option<&Path>,
    discovery_unavailable: bool,
) -> (tokio::process::Child, String) {
    let address_file = temp_dir.join("bound-address");
    let mut command = Command::new(env!("CARGO_BIN_EXE_test_streamable_http_server"));
    command
        .args(["--port", "0"])
        .env("MCP_STREAMABLE_HTTP_BOUND_ADDR_FILE", &address_file)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    if let Some(refresh_count_file) = refresh_count_file {
        command.env("MCP_STREAMABLE_HTTP_REFRESH_COUNT_FILE", refresh_count_file);
    }
    if discovery_unavailable {
        command.env("MCP_STREAMABLE_HTTP_DISCOVERY_UNAVAILABLE", "1");
    }
    let mut child = command.spawn().expect("spawn fake HTTP MCP server");
    let address = match tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Ok(address) = tokio::fs::read_to_string(&address_file).await {
                break address;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    {
        Ok(address) => address,
        Err(_) => {
            let _ = child.kill().await;
            panic!("fake server bound");
        }
    };
    (child, address)
}

#[tokio::test]
async fn fake_streamable_http_oauth_round_trips_without_a_human() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("tomcat").to_string_lossy().into_owned());
    std::fs::create_dir_all(temp.path().join("tomcat")).expect("work dir");
    let refresh_count_file = temp.path().join("refresh-count");
    let (mut child, address) =
        start_fake_server(temp.path(), Some(&refresh_count_file), false).await;
    let mcp_url = format!("http://{address}/mcp");
    let store = OAuthTokenStore::open(&cfg).expect("token store");
    let oauth = McpOAuthConfig::default();
    let client = reqwest::Client::builder()
        .no_proxy()
        .build()
        .expect("local HTTP test client");
    let token = authorize(&client, &store, "fake", &mcp_url, &oauth, false)
        .await
        .expect("unattended OAuth login");
    assert_eq!(token.access_token, "fake-access-token");
    assert!(store.load("fake").expect("load token").is_some());
    let mut expired = token.clone();
    expired.expires_at = Some(0);
    store
        .save("fake", expired)
        .expect("expire stored token for refresh");

    let server = ConfiguredMcpServer {
        config_key: "fake".to_string(),
        name: "fake".to_string(),
        source: McpConfigSource::Global,
        config: McpServerConfig {
            command: String::new(),
            args: Vec::new(),
            url: Some(mcp_url),
            auth: Some("oauth".to_string()),
            env: Default::default(),
            headers: Default::default(),
            oauth: Some(oauth),
            cwd: None,
            trusted: false,
            integrity: None,
            startup_timeout_ms: 30_000,
            call_timeout_ms: 120_000,
            tool_filter: ToolFilter::default(),
        },
    };
    let client = HttpTransport::new(store)
        .connect(&server)
        .await
        .expect("HTTP MCP connect");
    let tools = client.peer().list_tools(None).await.expect("tools/list");
    assert_eq!(tools.tools.len(), 1);
    let result = client
        .peer()
        .call_tool(
            rmcp::model::CallToolRequestParams::new("echo").with_arguments(
                serde_json::json!({"message": "hello"})
                    .as_object()
                    .expect("arguments")
                    .clone(),
            ),
        )
        .await
        .expect("tools/call");
    let result = serde_json::to_value(result).expect("serialize result");
    assert!(result.to_string().contains("fake echo: hello"));

    let refresh_count = tokio::fs::read_to_string(&refresh_count_file)
        .await
        .expect("fake OAuth refresh count");
    assert!(refresh_count.parse::<usize>().expect("refresh count") >= 1);
    child.kill().await.expect("stop fake server");
}

#[tokio::test]
async fn valid_oauth_token_survives_transient_discovery_failure() {
    let temp = tempfile::tempdir().expect("temp dir");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().join("tomcat").to_string_lossy().into_owned());
    std::fs::create_dir_all(temp.path().join("tomcat")).expect("work dir");
    let (mut child, address) = start_fake_server(temp.path(), None, true).await;
    let mcp_url = format!("http://{address}/mcp");
    let store = OAuthTokenStore::open(&cfg).expect("token store");
    store
        .save(
            "fake",
            StoredOAuthToken {
                access_token: "fake-access-token".to_string(),
                refresh_token: Some("fake-refresh-token".to_string()),
                expires_at: Some(u64::MAX),
                token_endpoint: format!("http://{address}/token"),
                issuer: Some(format!("http://{address}")),
                resource: Some(mcp_url.clone()),
                mcp_url: Some(mcp_url.clone()),
                client_metadata_url: None,
                scopes: Vec::new(),
                client_id: "fake-client".to_string(),
                client_secret: None,
            },
        )
        .expect("save valid token");
    let server = ConfiguredMcpServer {
        config_key: "fake".to_string(),
        name: "fake".to_string(),
        source: McpConfigSource::Global,
        config: McpServerConfig {
            command: String::new(),
            args: Vec::new(),
            url: Some(mcp_url),
            auth: Some("oauth".to_string()),
            env: Default::default(),
            headers: Default::default(),
            oauth: Some(McpOAuthConfig::default()),
            cwd: None,
            trusted: false,
            integrity: None,
            startup_timeout_ms: 30_000,
            call_timeout_ms: 120_000,
            tool_filter: ToolFilter::default(),
        },
    };

    let client = HttpTransport::new(store)
        .connect(&server)
        .await
        .expect("valid token remains usable when discovery is unavailable");
    let tools = client.peer().list_tools(None).await.expect("tools/list");
    assert_eq!(tools.tools.len(), 1);
    child.kill().await.expect("stop fake server");
}
