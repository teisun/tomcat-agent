use super::super::HostApiDispatcher;
use super::dispatch_with_extension_test::make_real_bash_dispatcher;
use crate::core::tools::primitive::BashTaskStatus;
use crate::core::SessionManager;
use crate::ext::host_binding::{HostRequest, HostResponse};
use serde_json::json;
use std::sync::Arc;

async fn call(
    d: &HostApiDispatcher,
    instance: &str,
    method: &str,
    params: serde_json::Value,
) -> HostResponse {
    d.dispatch_async(
        instance,
        HostRequest {
            module: "fs".into(),
            method: method.into(),
            params,
            call_id: None,
        },
    )
    .await
    .unwrap()
}

#[tokio::test]
async fn shared_dispatcher_routes_files_shell_and_task_control_to_the_calling_session() {
    let temp = tempfile::tempdir().unwrap();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    std::fs::create_dir_all(&a).unwrap();
    std::fs::create_dir_all(&b).unwrap();
    std::fs::write(a.join("value.txt"), "alpha").unwrap();
    std::fs::write(b.join("value.txt"), "beta").unwrap();
    let (d, a_registry) = make_real_bash_dispatcher(&a, true);
    let (other, b_registry) = make_real_bash_dispatcher(&b, true);
    let a_registry = a_registry.unwrap();
    let b_registry = b_registry.unwrap();
    let manager = Arc::new(SessionManager::new(temp.path().join("sessions")));
    d.bind_session(
        "a",
        Arc::downgrade(&manager),
        Some(Arc::downgrade(d.primitive.as_ref().unwrap())),
        Some(Arc::downgrade(&a_registry)),
    );
    d.bind_session(
        "b",
        Arc::downgrade(&manager),
        Some(Arc::downgrade(other.primitive.as_ref().unwrap())),
        Some(Arc::downgrade(&b_registry)),
    );

    let read = call(&d, "b/plugin", "readFile", json!({"path":"value.txt"})).await;
    assert!(read.ok, "{read:?}");
    assert!(read.data.unwrap()["content"]
        .as_str()
        .unwrap()
        .contains("beta"));
    let written = call(
        &d,
        "b/plugin",
        "writeFile",
        json!({"path":"created.txt", "content":"from B"}),
    )
    .await;
    assert!(written.ok, "{written:?}");
    let shell = call(
        &d,
        "b/plugin",
        "executeBash",
        json!({"command":"printf plugin > plugin-marker"}),
    )
    .await;
    assert!(shell.ok, "{shell:?}");
    assert_eq!(
        std::fs::read_to_string(b.join("created.txt")).unwrap(),
        "from B"
    );
    assert_eq!(
        std::fs::read_to_string(b.join("plugin-marker")).unwrap(),
        "plugin"
    );
    assert!(!a.join("created.txt").exists());
    assert!(!a.join("plugin-marker").exists());
    assert_eq!(
        std::fs::read_to_string(a.join("value.txt")).unwrap(),
        "alpha"
    );

    let ta = a_registry
        .spawn("exec sleep 30".into(), None)
        .await
        .unwrap();
    let tb = b_registry
        .spawn("printf ready; exec sleep 30".into(), None)
        .await
        .unwrap();
    let own_output = call(&d, "b/plugin", "taskOutput", json!({"taskId":tb.task_id})).await;
    let foreign_stop = call(&d, "b/plugin", "taskStop", json!({"taskId":ta.task_id})).await;
    let own_stop = call(&d, "b/plugin", "taskStop", json!({"taskId":tb.task_id})).await;
    let a_was_running = a_registry
        .list()
        .iter()
        .any(|t| t.task_id == ta.task_id && matches!(t.status, BashTaskStatus::Running));
    // Clean both real processes before assertions, including when routing is broken.
    a_registry.stop(&ta.task_id).await.unwrap();
    b_registry.stop(&tb.task_id).await.unwrap();
    a_registry.wait_for_finish(&ta.task_id).await.unwrap();
    b_registry.wait_for_finish(&tb.task_id).await.unwrap();
    assert!(own_output.ok, "{own_output:?}");
    assert!(!foreign_stop.ok, "a plugin in B cannot stop A's task");
    assert!(own_stop.ok, "{own_stop:?}");
    assert!(a_was_running);

    drop(other);
    drop(b_registry);
    for instance in ["b/plugin", "unknown/plugin", "unbound-plugin"] {
        let read = call(&d, instance, "readFile", json!({"path":"value.txt"})).await;
        assert!(
            !read.ok,
            "expired/unknown session must never fall back to A: {instance}"
        );
    }
    assert!(
        call(&d, "a/plugin", "readFile", json!({"path":"value.txt"}))
            .await
            .ok
    );
}
