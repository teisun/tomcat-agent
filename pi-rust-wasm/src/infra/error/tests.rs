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
