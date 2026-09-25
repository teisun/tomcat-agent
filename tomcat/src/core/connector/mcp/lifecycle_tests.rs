use super::*;
use crate::core::connector::mcp::manager::tests::{fake_server_args, manager_with_fake_server};

#[tokio::test]
async fn same_generation_old_attempt_cannot_publish_fail_or_close_the_next_connection() {
    let (_temp, manager) = manager_with_fake_server(fake_server_args(&[]), 1000);
    manager.connect_server("fake").await.unwrap();
    let key = manager.configured_server("fake").unwrap().config_key;
    let old = manager.entries.read()[&key].clone();
    let old_id = old.connection.as_ref().unwrap().id;
    // Inject the same signal delivered by the real SDK service observer.
    manager.connection_event(&key, old_id, ConnectionEvent::ServiceEnded);
    manager.connect_server(&key).await.unwrap();
    let current = manager.entries.read()[&key].clone();
    let connection = current.connection.clone().unwrap();
    assert_eq!(connection.id.generation, old_id.generation);
    assert_eq!(connection.id.attempt, old_id.attempt + 1);
    let run = current.run.as_ref().unwrap();
    let stale_attempt = ConnectAttempt {
        server: current.server.clone(),
        runtime: manager.runtime,
        id: old_id,
        manager: manager.self_weak.clone(),
        workspace_root: manager.workspace_root.clone(),
        oauth_store: manager.oauth_store.clone(),
        cancel: run.cancel.child_token(),
        cleanup: run.cleanup.clone(),
        refresh: None,
        refresh_used: current.budget.as_ref().unwrap().refresh_used.clone(),
    };
    // Use a live peer so rejection proves the attempt guard, not a closed peer.
    assert_eq!(
        manager
            .publish_connection(&stale_attempt, run, connection.clone())
            .unwrap_err()
            .kind,
        FailureKind::Cancelled
    );
    let failure = McpFailure::new(FailureKind::Authorization, "late old attempt");
    manager.finish_recovery(
        &key,
        old_id.generation,
        old.run.as_ref().unwrap(),
        &Err(failure.clone()),
    );
    manager.authorization_failure(&key, old_id, failure);
    for event in [
        ConnectionEvent::ServiceEnded,
        ConnectionEvent::SubmissionClosed,
        ConnectionEvent::SessionExpired,
    ] {
        manager.connection_event(&key, old_id, event);
    }
    let observed = manager.entries.read()[&key].clone();
    assert_eq!(observed.status.state, ServerState::Ready);
    assert_eq!(observed.budget.as_ref().unwrap().used, 2);
    assert!(Arc::ptr_eq(
        observed.connection.as_ref().unwrap(),
        &connection
    ));
    assert_eq!(manager.tool_catalog_snapshot(&key).unwrap().attempt, 2);
    manager.logout_server(&key).unwrap();
    tokio::time::timeout(CLEANUP_GRACE, async {
        while manager.retirement_debug_count(&key) != Some(0) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the final child/service must actually exit");
}
