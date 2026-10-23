//! UI preferences belong to the existing Tomcat configuration, not a chat session.
use super::types::{OutFrame, ResponseFrame};
use super::ServeState;
use crate::infra::config::{ui, UiLanguage};
use crate::infra::i18n::set_locale;
use crate::AppError;

fn language_environment(state: &ServeState) -> (Option<String>, Option<String>) {
    let environment = state.child_environment();
    let value = |key| match &environment {
        Some(environment) => environment.get(key).map(str::to_owned),
        None => std::env::var(key).ok(),
    };
    (value("TOMCAT_HOST_LOCALE"), value("TOMCAT__UI__LANGUAGE"))
}

pub(super) fn snapshot(state: &ServeState) -> Result<ui::UiPreferences, AppError> {
    let (host, override_language) = language_environment(state);
    ui::preferences(&state.config_path, host.as_deref(), override_language.as_deref())
}

pub(super) async fn set_language(
    state: &ServeState,
    id: Option<String>,
    language: UiLanguage,
) -> Result<(), AppError> {
    let path = state.config_path.clone();
    let (host, override_language) = language_environment(state);
    let result = tokio::task::spawn_blocking(move || {
        ui::write_language(&path, language)?;
        let preferences = ui::preferences(&path, host.as_deref(), override_language.as_deref())?;
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
