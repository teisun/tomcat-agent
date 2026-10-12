//! Real authorization refresh plus counted MCP startups, using the existing
//! OAuth fixture (no browser and no extra HTTP server implementation).
use super::{
    start_fixture_with_oauth_failure, AppConfig, Duration, OAuthTokenStore, StoredOAuthToken,
};
use serde_json::json;
use tomcat::core::connector::mcp::{
    config::add_global_server,
    manager::{McpManager, ServerState},
};

#[tokio::test]
async fn oauth_refresh_rehandshake_consumes_the_same_three_attempt_budget() {
    for fail_after_refresh in [false, true] {
        let temp = tempfile::tempdir().unwrap();
        let refreshes = temp.path().join("refresh-count");
        let initializations = refreshes.with_extension("initializations");
        let (mut child, address) = start_fixture_with_oauth_failure(
            temp.path(),
            Some(&refreshes),
            false,
            false,
            fail_after_refresh,
        )
        .await;
        let mut cfg = AppConfig::default();
        cfg.storage.work_dir = Some(temp.path().join("work").to_string_lossy().into_owned());
        let url = format!("http://{address}/mcp");
        add_global_server(
            &cfg,
            "oauth".into(),
            serde_json::from_value(json!({"url":url, "auth":"oauth"})).unwrap(),
        )
        .unwrap();
        let manager = McpManager::new(&cfg, None).unwrap();
        let key = manager.configured_server("oauth").unwrap().config_key;
        let store = OAuthTokenStore::open(&cfg).unwrap();
        store
            .save(
                &key,
                StoredOAuthToken {
                    access_token: "stale-access-token".into(),
                    refresh_token: Some("fake-refresh-token".into()),
                    expires_at: Some(u64::MAX),
                    token_endpoint: format!("http://{address}/token"),
                    issuer: Some(format!("http://{address}")),
                    resource: Some(url.clone()),
                    mcp_url: Some(url),
                    client_metadata_url: None,
                    scopes: Vec::new(),
                    client_id: "fake-client".into(),
                    client_secret: None,
                },
            )
            .unwrap();
        let result = tokio::time::timeout(Duration::from_secs(8), manager.connect_server("oauth"))
            .await
            .unwrap();
        assert_eq!(result.is_ok(), !fail_after_refresh, "{result:?}");
        let count = tokio::fs::read_to_string(&initializations)
            .await
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert_eq!(
            count,
            if fail_after_refresh { 3 } else { 2 },
            "metadata preparation must not send hidden initialize probes"
        );
        assert_eq!(tokio::fs::read_to_string(&refreshes).await.unwrap(), "1");
        if fail_after_refresh {
            assert!(matches!(
                manager.statuses()[0].state,
                ServerState::Failed(_)
            ));
            for _ in 0..3 {
                assert!(manager.connect_server("oauth").await.is_err());
            }
            assert_eq!(
                tokio::fs::read_to_string(&initializations).await.unwrap(),
                "3"
            );
        } else {
            assert_eq!(manager.statuses()[0].state, ServerState::Ready);
            let result = manager
                .call_model_tool("mcp__oauth__echo", json!({"message":"after refresh"}))
                .await
                .unwrap();
            assert!(result.to_string().contains("fake echo: after refresh"));
        }
        manager.remove_configured_server(&key, &cfg).unwrap();
        child.kill().await.unwrap();
        child.wait().await.unwrap();
    }
}
