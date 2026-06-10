use crate::{AppConfig, AppError, SessionMode};

pub(crate) fn run_claw(resume: bool, cfg: &AppConfig) -> Result<(), AppError> {
    super::chat_cmd::run_chat_mode(resume, cfg, SessionMode::Claw)
}
