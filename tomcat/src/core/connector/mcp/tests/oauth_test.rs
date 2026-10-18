use super::{pkce_pair, AuthorizationServerMetadata, OAuthTokenStore, ProtectedResourceMetadata};
use crate::core::connector::mcp::config::McpOAuthConfig;
use crate::AppConfig;

#[test]
fn pkce_pair_has_distinct_verifier_and_challenge() {
    let (verifier, challenge) = pkce_pair().expect("pkce");
    assert!(!verifier.is_empty());
    assert!(!challenge.is_empty());
    assert_ne!(verifier, challenge);
}

#[test]
fn metadata_shapes_are_deserializable() {
    let protected: ProtectedResourceMetadata = serde_json::from_value(serde_json::json!({
        "resource": "http://127.0.0.1/mcp",
        "authorization_servers": ["http://127.0.0.1:9000"],
        "scopes_supported": ["read"]
    }))
    .expect("protected metadata");
    assert_eq!(protected.authorization_servers.len(), 1);
    let metadata: AuthorizationServerMetadata = serde_json::from_value(serde_json::json!({
        "authorization_endpoint": "http://127.0.0.1/authorize",
        "token_endpoint": "http://127.0.0.1/token",
        "code_challenge_methods_supported": ["S256"]
    }))
    .expect("authorization metadata");
    assert_eq!(metadata.token_endpoint, "http://127.0.0.1/token");
}

#[test]
fn credential_revision_rejects_identical_token_aba_and_empty_logout_across_stores() {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().to_string_lossy().into_owned());
    let store = OAuthTokenStore::open(&cfg).unwrap();
    let other = OAuthTokenStore::open(&cfg).unwrap();
    let token: super::StoredOAuthToken = serde_json::from_value(serde_json::json!({"accessToken":"same-token", "tokenEndpoint":"http://127.0.0.1/token", "clientId":"fixture"})).unwrap();
    let empty = store.snapshot("empty").unwrap();
    assert!(!other.remove("empty").unwrap());
    assert!(!store
        .save_if_snapshot_current("empty", &empty, token.clone())
        .unwrap());
    store.save("server", token.clone()).unwrap();
    let before = store.snapshot("server").unwrap();
    other.remove("server").unwrap();
    other.save("server", token.clone()).unwrap();
    assert!(!store
        .save_if_snapshot_current("server", &before, token.clone())
        .unwrap());
    let current = store.snapshot("server").unwrap();
    assert!(store
        .save_if_snapshot_current("server", &current, token)
        .unwrap());
}

#[test]
fn stale_refresh_cannot_restore_a_removed_or_replaced_token() {
    let temp = tempfile::tempdir().expect("temp");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().to_string_lossy().into_owned());
    let store = OAuthTokenStore::open(&cfg).expect("store");
    let original = super::StoredOAuthToken {
        access_token: "old-access".to_string(),
        refresh_token: Some("old-refresh".to_string()),
        expires_at: Some(0),
        token_endpoint: "https://example.test/token".to_string(),
        issuer: None,
        resource: None,
        mcp_url: None,
        client_metadata_url: None,
        scopes: Vec::new(),
        client_id: "test-client".to_string(),
        client_secret: None,
    };
    let updated = super::StoredOAuthToken {
        access_token: "late-access".to_string(),
        ..original.clone()
    };
    store
        .save("server", original.clone())
        .expect("save original");
    assert!(store.remove("server").expect("logout"));
    assert!(!store
        .save_if_current("server", &original, updated.clone())
        .expect("reject late refresh after removal"));
    assert!(store.load("server").expect("load after removal").is_none());

    store.save("server", updated.clone()).expect("new login");
    assert!(!store
        .save_if_current("server", &original, original.clone())
        .expect("reject late refresh after replacement"));
    assert_eq!(
        store
            .load("server")
            .expect("load replacement")
            .expect("replacement")
            .access_token,
        "late-access"
    );
}

#[test]
fn malformed_token_store_does_not_echo_credential_values() {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().to_string_lossy().into_owned());
    let store = OAuthTokenStore::open(&cfg).unwrap();
    let raw = r#"{"servers":{"private":{"accessToken":"credential-sentinel","expiresAt":"credential-sentinel","tokenEndpoint":"https://example.com/token","clientId":"fixture"}}}"#;
    std::fs::write(store.path(), raw).unwrap();
    let error = store.load("private").unwrap_err();
    assert!(!error.to_string().contains("credential-sentinel"));
    assert!(!format!("{error:?}").contains("credential-sentinel"));
    assert!(error.to_string().contains("OAuth"));
    assert!(error
        .to_string()
        .contains(&store.path().display().to_string()));
    assert_eq!(std::fs::read_to_string(store.path()).unwrap(), raw);
}

#[tokio::test]
async fn refresh_transport_error_does_not_echo_endpoint_query_credentials() {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().to_string_lossy().into_owned());
    let store = OAuthTokenStore::open(&cfg).unwrap();
    let token: super::StoredOAuthToken = serde_json::from_value(serde_json::json!({
        "accessToken":"access-sentinel", "refreshToken":"refresh-sentinel", "expiresAt":0,
        "tokenEndpoint":"http://127.0.0.1:0/token?credential=query-sentinel", "clientId":"fixture"
    }))
    .unwrap();
    store.save("private", token.clone()).unwrap();
    let client = reqwest::Client::builder()
        .no_proxy()
        .timeout(std::time::Duration::from_secs(1))
        .build()
        .unwrap();
    let error = store.force_refresh(&client, "private").await.unwrap_err();
    for sentinel in ["access-sentinel", "refresh-sentinel", "query-sentinel"] {
        assert!(!error.to_string().contains(sentinel));
        assert!(!format!("{error:?}").contains(sentinel));
    }
    assert_eq!(store.load("private").unwrap(), Some(token));
}

#[test]
fn token_store_round_trips_without_logging_secret() {
    let temp = tempfile::tempdir().expect("temp");
    let mut cfg = AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().to_string_lossy().into_owned());
    let store = OAuthTokenStore::open(&cfg).expect("store");
    store
        .save(
            "fake",
            super::StoredOAuthToken {
                access_token: "access-secret".to_string(),
                refresh_token: Some("refresh-secret".to_string()),
                expires_at: Some(u64::MAX),
                token_endpoint: "http://127.0.0.1/token".to_string(),
                issuer: None,
                resource: None,
                mcp_url: None,
                client_metadata_url: None,
                scopes: Vec::new(),
                client_id: "test-client".to_string(),
                client_secret: None,
            },
        )
        .expect("save");
    #[cfg(windows)]
    crate::test_support::assert_private_to_current_user(&store.path);
    assert_eq!(
        store
            .load("fake")
            .expect("load")
            .expect("token")
            .access_token,
        "access-secret"
    );
    assert!(store.remove("fake").expect("remove"));
    assert!(store.load("fake").expect("load after remove").is_none());
    let _ = McpOAuthConfig::default();
}
