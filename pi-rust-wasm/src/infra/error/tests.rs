use super::*;

#[test]
fn app_error_display() {
    let e = AppError::Config("test".to_string());
    assert!(e.to_string().contains("配置错误"));
    assert!(e.to_string().contains("test"));
}

#[test]
fn app_error_from_io() {
    let io = std::io::Error::new(std::io::ErrorKind::NotFound, "file not found");
    let e: AppError = io.into();
    assert!(matches!(e, AppError::Io(_)));
}

#[test]
fn app_error_apply_boundary_stale_display() {
    let e = AppError::ApplyBoundaryStale {
        covered_end_id: "e1".to_string(),
    };
    let s = e.to_string();
    assert!(s.contains("e1"));
    assert!(s.contains("apply_boundary"));
}
