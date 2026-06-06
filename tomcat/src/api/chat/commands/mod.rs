//! Local commands handled by `tomcat chat` before a line is sent to the LLM.

mod cmd_ckpt;
mod cmd_help;
mod cmd_model;
mod cmd_path;
mod cmd_plan;
mod cmd_restore;
mod cmd_skill;
mod cmd_thinking;
mod parse;

pub use cmd_plan::PlanCommand;

#[cfg(test)]
mod tests;

pub use cmd_path::render_path_menu;
pub(super) use parse::{dispatch_chat_command, ChatCommandOutcome};
pub use parse::{parse_chat_command, ChatCommand, ModelCommand, SkillCommand};

/// Public façade returning the local-command help banner shown by `/help`.
///
/// Crate-internal call sites use [`cmd_help::help_text`] directly; this
/// re-exposed entry point lets integration tests (e.g. `path_command_e2e`)
/// pin the user-visible wording without widening the internal helper's
/// visibility beyond `pub(crate)`.
pub fn help_text() -> &'static str {
    cmd_help::help_text()
}
