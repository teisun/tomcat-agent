use crate::infra::error::AppError;

pub(super) fn make_readline_editor() -> Result<rustyline::DefaultEditor, AppError> {
    rustyline::DefaultEditor::with_config(build_readline_config()).map_err(|e| {
        AppError::Config(crate::infra::i18n::tr(
            "terminal.editorFailed",
            &[("detail", &e.to_string())],
        ))
    })
}

pub(crate) fn build_readline_config() -> rustyline::Config {
    rustyline::Config::builder().bracketed_paste(true).build()
}
