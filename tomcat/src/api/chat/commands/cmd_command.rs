use super::parse::ChatCommandOutcome;
use crate::api::chat::ChatContext;
use crate::core::llm::{ChatMessage, ChatMessageContentPart};
use crate::core::project_instructions::{InstructionKind, PromptFile};

pub(super) fn message(
    ctx: &ChatContext,
    id: &str,
    kind: InstructionKind,
    intent: &str,
) -> Result<ChatMessage, String> {
    let reference = ctx.resolve_instruction(id, kind)?;
    let mut parts = vec![ChatMessageContentPart::reference(reference)];
    if !intent.is_empty() {
        parts.push(ChatMessageContentPart::text(format!("\n\n{intent}")));
    }
    Ok(ChatMessage::user_with_parts(parts))
}

fn invoke(ctx: &ChatContext, id: &str, intent: &str, history_line: String) -> ChatCommandOutcome {
    match message(ctx, id, InstructionKind::Command, intent) {
        Ok(message) => ChatCommandOutcome::UserMessage {
            message,
            history_line,
        },
        Err(error) => {
            println!("[command] {error}");
            ChatCommandOutcome::Handled
        }
    }
}

pub(super) fn list_text(files: &[PromptFile]) -> String {
    let mut text = format!("Commands（{}）", files.len());
    for file in files {
        text.push_str(&format!(
            "\n  /{}  {}  {}",
            file.card.name,
            file.card.source,
            shell_words::quote(&file.card.id)
        ));
    }
    text
}

pub(super) fn duplicate_text(name: &str, files: &[&PromptFile]) -> String {
    let mut text = format!(
        "[command] /{name} 有 {} 个同名命令，复制其中一条：",
        files.len()
    );
    for file in files {
        text.push_str(&format!(
            "\n  /command use {}",
            shell_words::quote(&file.card.id)
        ));
    }
    text
}

pub(super) fn run(ctx: &ChatContext, line: String) -> ChatCommandOutcome {
    let words = match shell_words::split(&line) {
        Ok(w) => w,
        Err(e) => {
            println!("[command] {e}");
            return ChatCommandOutcome::Handled;
        }
    };
    match words.as_slice() {
        [_, sub] if sub == "list" => {
            let commands = ctx.project_commands();
            println!("{}", list_text(&commands.files));
            ChatCommandOutcome::Handled
        }
        [_, sub, id, intent @ ..] if sub == "use" => invoke(ctx, id, &intent.join(" "), line),
        _ => {
            println!("[command] 用法：/command list | /command use <resourceId> [补充说明]");
            ChatCommandOutcome::Handled
        }
    }
}

pub(super) fn shortcut(ctx: &ChatContext, line: String) -> ChatCommandOutcome {
    let trimmed = line.trim();
    if let Some(name) = trimmed
        .split_whitespace()
        .next()
        .and_then(|s| s.strip_prefix('/'))
    {
        let commands = ctx.project_commands();
        let matches: Vec<_> = commands
            .files
            .iter()
            .filter(|f| f.card.name == name)
            .collect();
        match matches.as_slice() {
            [file] => {
                return invoke(
                    ctx,
                    &file.card.id,
                    trimmed.get(name.len() + 1..).unwrap_or("").trim_start(),
                    line.clone(),
                )
            }
            [] => {}
            many => {
                println!("{}", duplicate_text(name, many));
                return ChatCommandOutcome::Handled;
            }
        }
    }
    ChatCommandOutcome::Continue {
        line,
        echo_user: false,
        history_line: None,
    }
}
