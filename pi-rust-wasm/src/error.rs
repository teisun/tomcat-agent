//! 项目统一错误枚举。MVP 会话与审计均不使用 SQLite，故不包含 Db 变体。

use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("IO错误: {0}")]
    Io(#[from] std::io::Error),
    #[error("LLM调用错误: {0}")]
    Llm(String),
    #[error("插件错误: {0}")]
    Plugin(String),
    #[error("4原语执行错误: {0}")]
    Primitive(String),
    #[error("事件执行错误: {0}")]
    Event(String),
    #[error("配置错误: {0}")]
    Config(String),
    #[error("权限错误: {0}")]
    Permission(String),
    #[error("工具调用错误: {0}")]
    Tool(String),
    #[error("序列化错误: {0}")]
    Serialize(#[from] serde_json::Error),
    #[error("Wasm运行时错误: {0}")]
    WasmEdge(String),
    #[error("JS执行错误: {0}")]
    QuickJS(String),
    #[error("审计日志错误: {0}")]
    Audit(String),
}

#[cfg(test)]
mod tests {
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
}
