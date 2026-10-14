//! UI preferences belong to the existing Tomcat configuration, not a chat session.
use super::types::{OutFrame, ResponseFrame};
use super::ServeState;
use crate::infra::config::{ui, UiLanguage};
use crate::infra::i18n::set_locale;
use crate::AppError;

pub(super) fn snapshot(path: &std::path::Path) -> Result<ui::UiPreferences, AppError> {
    ui::preferences(path, std::env::var("TOMCAT_HOST_LOCALE").ok().as_deref())
}

pub(super) async fn set_language(
    state: &ServeState,
    id: Option<String>,
    language: UiLanguage,
) -> Result<(), AppError> {
    let path = state.config_path.clone();
    let result = tokio::task::spawn_blocking(move || {
        ui::write_language(&path, language)?;
        let preferences = snapshot(&path)?;
        set_locale(preferences.effective);
        Ok::<_, AppError>(preferences)
    })
    .await
    .map_err(|error| AppError::Internal(error.to_string()))?;
    let response = match result {
        Ok(preferences) => ResponseFrame::ok(id, None, Some(serde_json::to_value(preferences)?)),
        Err(error) => ResponseFrame::error(id, None, error.to_string()),
    };
    state.writer.send(OutFrame::Response(response))
}
