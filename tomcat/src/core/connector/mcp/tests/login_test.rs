use super::*;
fn fixture() -> (tempfile::TempDir, Arc<McpManager>, crate::AppConfig, String) {
    let temp = tempfile::tempdir().unwrap();
    let mut cfg = crate::AppConfig::default();
    cfg.storage.work_dir = Some(temp.path().to_string_lossy().into_owned());
    crate::core::connector::mcp::config::add_global_server(
        &cfg,
        "login".into(),
        serde_json::from_value(serde_json::json!({"url":"http://127.0.0.1:9/mcp", "auth":"none"}))
            .unwrap(),
    )
    .unwrap();
    let manager = McpManager::new(&cfg, None).unwrap();
    let key = manager.configured_server("login").unwrap().config_key;
    (temp, manager, cfg, key)
}
fn token(value: &str) -> StoredOAuthToken {
    serde_json::from_value(serde_json::json!({"accessToken":value,"tokenEndpoint":"http://127.0.0.1/token","clientId":"fixture"})).unwrap()
}

fn cached_pair() -> (
    tempfile::TempDir,
    Arc<crate::core::connector::ConnectorRegistry>,
    Arc<crate::core::connector::ConnectorRegistry>,
    crate::AppConfig,
    String,
) {
    let (temp, _, cfg, key) = fixture();
    let workspace = temp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();
    let first = crate::core::connector::ConnectorRegistry::new(&cfg, None).unwrap();
    let second = crate::core::connector::ConnectorRegistry::new(&cfg, Some(&workspace)).unwrap();
    (temp, first, second, cfg, key)
}

#[test]
fn shared_logout_fences_cache_membership_and_late_login_until_revocation_finishes() {
    let (_temp, first, second, _, key) = cached_pair();
    let first_manager = first.mcp_manager();
    let second_manager = second.mcp_manager();
    let (_, generation, expected, old) = second_manager.begin_login(&key).unwrap();
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(first_manager.oauth_store.path().with_extension("lock"))
        .unwrap();
    fs2::FileExt::lock_exclusive(&file).unwrap();
    let (sender, receiver) = std::sync::mpsc::channel();
    let logout_key = key.clone();
    let worker = std::thread::spawn(move || {
        sender.send(first.logout_server(&logout_key)).unwrap();
    });
    // The store lock holds the real operation at its last synchronous boundary.
    // Both entry locks and the cache read guard must still be held there.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
    let mut entries_locked = false;
    while std::time::Instant::now() < deadline {
        if first_manager.entries.try_read().is_none() && second_manager.entries.try_read().is_none()
        {
            entries_locked = true;
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let membership_locked = crate::core::connector::connector_registry_cache()
        .try_write()
        .is_none();
    drop(file);
    let removed = receiver
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap()
        .unwrap();
    worker.join().unwrap();
    assert!(entries_locked && membership_locked);
    assert!(!removed);
    assert!(old.is_cancelled());
    let (_, new_generation, new_snapshot, current) = second_manager.begin_login(&key).unwrap();
    second_manager
        .commit_login(
            &key,
            new_generation,
            &current,
            &new_snapshot,
            token("new-owner"),
        )
        .unwrap();
    assert!(second_manager
        .commit_login(&key, generation, &old, &expected, token("late-old"))
        .is_err());
    drop(LoginOwner {
        manager: &second_manager,
        key: key.clone(),
        cancel: old,
    });
    assert!(!current.is_cancelled());
    assert_eq!(
        second_manager
            .oauth_store
            .load(&key)
            .unwrap()
            .unwrap()
            .access_token,
        "new-owner"
    );
}

#[test]
fn concurrent_shared_logouts_use_one_lock_order() {
    let (_temp, first, second, _, key) = cached_pair();
    let managers = [first.mcp_manager(), second.mcp_manager()];
    let barrier = Arc::new(std::sync::Barrier::new(2));
    let (sender, receiver) = std::sync::mpsc::channel();
    let mut workers = Vec::new();
    for registry in [first, second] {
        let barrier = barrier.clone();
        let sender = sender.clone();
        let key = key.clone();
        workers.push(std::thread::spawn(move || {
            barrier.wait();
            sender.send(registry.logout_server(&key)).unwrap();
        }));
    }
    for _ in 0..2 {
        assert!(!receiver
            .recv_timeout(std::time::Duration::from_secs(3))
            .expect("parallel logouts must not deadlock")
            .unwrap());
    }
    for worker in workers {
        worker.join().unwrap();
    }
    assert!(managers
        .iter()
        .all(|manager| manager.statuses()[0].state == ServerState::Disconnected));
}

#[test]
fn shared_logout_preserves_local_retirement_when_credential_persistence_fails() {
    let (_temp, first, second, _, key) = cached_pair();
    let manager = second.mcp_manager();
    let (_, _, _, cancel) = manager.begin_login(&key).unwrap();
    std::fs::write(manager.oauth_store.path(), "invalid credential document").unwrap();
    assert!(first.logout_server(&key).is_err());
    assert!(cancel.is_cancelled());
    assert_eq!(
        first.mcp_manager().statuses()[0].state,
        ServerState::Disconnected
    );
    assert_eq!(manager.statuses()[0].state, ServerState::Disconnected);
    assert_eq!(
        std::fs::read_to_string(manager.oauth_store.path()).unwrap(),
        "invalid credential document"
    );
}

#[test]
fn global_logout_does_not_revoke_a_same_named_project_login() {
    use crate::core::connector::mcp::config::{add_project_server, McpConfigSource};
    use crate::core::security::project_trust::ProjectTrustStore;
    let (temp, first, _second, cfg, key) = cached_pair();
    let workspace = temp.path().join("project-override");
    std::fs::create_dir_all(&workspace).unwrap();
    add_project_server(
        &cfg,
        &workspace,
        "login".into(),
        serde_json::from_value(serde_json::json!({"url":"http://127.0.0.1:9/mcp", "auth":"none"}))
            .unwrap(),
    )
    .unwrap();
    let root = ProjectTrustStore::root_for(&workspace).unwrap();
    ProjectTrustStore::open(&cfg).unwrap().trust(&root).unwrap();
    let registry = crate::core::connector::ConnectorRegistry::new(&cfg, Some(&workspace)).unwrap();
    let manager = registry.mcp_manager();
    let project_key = manager
        .statuses()
        .into_iter()
        .find(|status| status.source == McpConfigSource::Project)
        .unwrap()
        .config_key;
    assert_ne!(project_key, key);
    let (_, generation, expected, current) = manager.begin_login(&project_key).unwrap();
    manager
        .commit_login(
            &project_key,
            generation,
            &current,
            &expected,
            token("project-owner"),
        )
        .unwrap();
    assert!(!first.logout_server(&key).unwrap());
    assert!(!current.is_cancelled());
    assert_eq!(
        manager.entries.read().get(&project_key).unwrap().generation,
        generation
    );
    assert_eq!(
        manager
            .oauth_store
            .load(&project_key)
            .unwrap()
            .unwrap()
            .access_token,
        "project-owner"
    );
}

#[tokio::test]
async fn older_login_cannot_save_erase_cancel_or_reconnect_a_newer_login() {
    let (_temp, manager, _, key) = fixture();
    let (_, generation, old_snapshot, old) = manager.begin_login(&key).unwrap();
    let (_, current_generation, current_snapshot, current) = manager.begin_login(&key).unwrap();
    assert!(old.is_cancelled());
    manager
        .commit_login(
            &key,
            current_generation,
            &current,
            &current_snapshot,
            token("new"),
        )
        .unwrap();
    assert!(manager
        .commit_login(&key, generation, &old, &old_snapshot, token("late-old"))
        .is_err());
    assert!(manager
        .reconnect_after_login(&key, generation, old.clone())
        .await
        .is_err());
    drop(LoginOwner {
        manager: &manager,
        key: key.clone(),
        cancel: old,
    });
    assert!(!current.is_cancelled());
    assert!(manager
        .oauth_cancellations
        .get(&key)
        .is_some_and(|value| Arc::ptr_eq(value.value(), &current)));
    assert_eq!(
        manager
            .oauth_store
            .load(&key)
            .unwrap()
            .unwrap()
            .access_token,
        "new"
    );
}

#[test]
fn revoke_and_replacement_do_not_resurrect_or_erase_credentials() {
    for action in ["logout", "remove-readd"] {
        let (_temp, manager, cfg, key) = fixture();
        let (definition, generation, expected, cancel) = manager.begin_login(&key).unwrap();
        match action {
            "logout" => {
                assert!(!manager.logout_server(&key).unwrap());
            }
            _ => {
                manager.remove_configured_server(&key, &cfg).unwrap();
                crate::core::connector::mcp::config::add_global_server(
                    &cfg,
                    "login".into(),
                    definition.config,
                )
                .unwrap();
                manager.reload_configuration(&cfg).unwrap();
            }
        }
        manager.oauth_store.save(&key, token("new-owner")).unwrap();
        assert!(manager
            .commit_login(&key, generation, &cancel, &expected, token("late-old"))
            .is_err());
        assert_eq!(
            manager
                .oauth_store
                .load(&key)
                .unwrap()
                .unwrap()
                .access_token,
            "new-owner"
        );
    }
}
