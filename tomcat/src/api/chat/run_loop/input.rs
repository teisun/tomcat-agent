use crate::infra::error::AppError;

pub(super) fn make_readline_editor() -> Result<rustyline::DefaultEditor, AppError> {
    rustyline::DefaultEditor::with_config(build_readline_config())
        .map_err(|e| AppError::Config(format!("初始化行编辑器失败: {}", e)))
}

pub(crate) fn build_readline_config() -> rustyline::Config {
    rustyline::Config::builder().bracketed_paste(true).build()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_readline_config_enables_bracketed_paste() {
        let cfg = build_readline_config();
        assert!(cfg.enable_bracketed_paste());
    }
}
