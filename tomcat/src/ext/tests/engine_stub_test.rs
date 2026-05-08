use super::*;

#[test]
fn engine_global_returns_err_in_stub() {
    let r = WasmEngine::global(None);
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("stub"));
}

#[test]
fn engine_create_instance_returns_err_in_stub() {
    let engine = WasmEngine {
        _config: WasmEngineConfig::default(),
    };
    let r = engine.create_instance("plugin-1");
    assert!(r.is_err());
}

#[test]
fn config_default_standard_mode() {
    let c = WasmEngineConfig::default();
    assert_eq!(c.wasm_max_pages, DEFAULT_WASM_MAX_PAGES);
    assert_eq!(c.quickjs_heap_mb, DEFAULT_QUICKJS_HEAP_MB);
    assert!(c.quickjs_path.is_none());
}
