use super::{PluginEngine, PluginEngineConfig};

#[test]
fn global_keeps_explicit_runtime_config() {
    let engine = PluginEngine::global(Some(PluginEngineConfig {
        quickjs_heap_mb: 8,
        call_timeout_ms: 321,
        interrupt_budget: 99,
        ..Default::default()
    }))
    .expect("create engine");
    assert_eq!(engine.config.quickjs_heap_mb, 8);
    assert_eq!(engine.config.call_timeout_ms, 321);
    assert_eq!(engine.config.interrupt_budget, 99);
}

#[test]
fn create_instance_binds_plugin_identity() {
    let engine = PluginEngine::global(None).expect("create engine");
    let instance = engine
        .create_instance("engine-plugin")
        .expect("create instance");
    assert_eq!(instance.plugin_id(), "engine-plugin");
    instance.destroy();
}
