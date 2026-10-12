//! Local commands handled by `tomcat chat` before a line is sent to the LLM.

mod cmd_ckpt;
mod cmd_command;
mod cmd_compact;
mod cmd_connector;
mod cmd_context;
mod cmd_effort;
mod cmd_help;
mod cmd_install;
mod cmd_model;
mod cmd_path;
mod cmd_plan;
mod cmd_reload;
mod cmd_restore;
mod cmd_skill;
mod cmd_thinking;
mod cmd_uninstall;
mod parse;
mod shared;

pub(crate) use cmd_ckpt::checkpoint_kind_label;
pub(crate) use cmd_compact::compact_session;
pub use cmd_install::InstallTarget;
pub use cmd_plan::PlanCommand;
pub(crate) use cmd_restore::{restore_core, RestoreCoreReport};
pub use shared::{
    parse_shared_slash, run_shared_slash_command, shared_slash_commands, shared_usage_error,
    SharedSlashCommand, SlashReply,
};

#[cfg(test)]
mod tests;

pub use cmd_path::render_path_menu;
pub(super) use parse::{dispatch_chat_command, ChatCommandOutcome};
pub use parse::{parse_chat_command, ChatCommand, ConnectorCommand, ModelCommand, SkillCommand};

/// Public façade returning the local-command help banner shown by `/help`.
///
/// Crate-internal call sites use [`cmd_help::help_text`] directly; this
/// re-exposed entry point lets integration tests (e.g. `path_command_e2e`)
/// pin the user-visible wording without widening the internal helper's
/// visibility beyond `pub(crate)`.
pub fn help_text() -> String {
    cmd_help::help_text()
}
