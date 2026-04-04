use super::*;

#[test]
fn run_script_returns_err_in_stub() {
    let mut inst = WasmInstance::new("p1".to_string());
    let r = inst.run_script("1+1");
    assert!(r.is_err());
    assert!(r.unwrap_err().to_string().contains("stub"));
}

#[test]
fn register_host_binding_ok_in_stub() {
    let mut inst = WasmInstance::new("p1".to_string());
    let r = inst.register_host_binding(|_| Err(AppError::Config("x".to_string())));
    assert!(r.is_ok());
}

#[test]
fn destroy_consumes_instance() {
    let inst = WasmInstance::new("p1".to_string());
    inst.destroy();
}
