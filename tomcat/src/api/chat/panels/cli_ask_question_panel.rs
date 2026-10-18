use crate::infra::i18n::tr;
use crate::infra::platform::stdin;
use async_trait::async_trait;

use crate::core::plan_runtime::panels::{
    Answer, AskQuestionOutcome, AskQuestionPanel, AskQuestionResult, AskQuestionTermination,
    Question, QuestionOption, CUSTOM_OPTION_ID,
};

const ASK_QUESTION_TEST_AUTO_PICK_ENV: &str = "TOMCAT_ASK_QUESTION_TEST_AUTO_PICK";

/// CLI panel with one cancel-safe stdin owner. [`stdin::read_line_cancellable`] rechecks the
/// termination every 50ms, so an interrupt releases ownership before the chat prompt starts
/// reading the next line.
pub struct CliAskQuestionPanel;

#[async_trait]
impl AskQuestionPanel for CliAskQuestionPanel {
    async fn ask(
        &self,
        questions: Vec<Question>,
        termination: AskQuestionTermination,
    ) -> AskQuestionResult {
        let mut answers = Vec::with_capacity(questions.len());
        for question in &questions {
            if let Some(result) = termination.result() {
                return result;
            }
            if let Some(answer) = auto_pick_answer_for_test(question) {
                let picked = answer.option_ids.join(",");
                eprintln!(
                    "[ask_question:auto-pick] qid={} strategy=recommended picks={picked}",
                    question.id
                );
                answers.push(answer);
                continue;
            }
            loop {
                if let Some(result) = termination.result() {
                    return result;
                }
                eprintln!("\n{}", question.prompt);
                for (index, option) in question.options.iter().enumerate() {
                    let suffix = if option.recommended {
                        tr("question.recommended", &[])
                    } else {
                        String::new()
                    };
                    eprintln!("  {}. {}{}", index + 1, option.label, suffix);
                }
                if question.allow_custom {
                    eprintln!("{}", tr("question.custom_menu", &[]));
                }
                eprintln!("{}", tr("question.skip_menu", &[]));
                eprint!(
                    "{}",
                    tr(
                        if question.allow_custom {
                            "question.input_custom"
                        } else {
                            "question.input_fixed"
                        },
                        &[]
                    )
                );

                let line = match read_one_line(&termination).await {
                    Ok(line) => line,
                    Err(result) => return result,
                };
                if let Some(result) = termination.result() {
                    return result;
                }
                let line = line.trim();
                if line.eq_ignore_ascii_case("skip") {
                    answers.push(Answer {
                        question_id: question.id.clone(),
                        option_ids: vec![],
                        custom_text: None,
                        skipped: true,
                        picked_recommended: false,
                    });
                    break;
                }
                match parse_custom_answer(question, line, &termination).await {
                    Ok(Some(answer)) => {
                        answers.push(answer);
                        break;
                    }
                    Ok(None) => {}
                    Err(result) => return result,
                }
                if let Some(answer) = parse_single_choice_answer(question, line) {
                    answers.push(answer);
                    break;
                }
                eprintln!("{}", tr("question.invalid", &[]));
            }
        }
        AskQuestionResult::answered(answers)
    }
}

fn auto_pick_answer_for_test(question: &Question) -> Option<Answer> {
    let strategy = std::env::var(ASK_QUESTION_TEST_AUTO_PICK_ENV).ok()?;
    if !strategy.eq_ignore_ascii_case("recommended") {
        return None;
    }
    let picked = question
        .options
        .iter()
        .find(|option| option.recommended)
        .or_else(|| question.options.first())?;
    Some(Answer {
        question_id: question.id.clone(),
        option_ids: vec![picked.id.clone()],
        custom_text: None,
        skipped: false,
        picked_recommended: picked.recommended,
    })
}

async fn parse_custom_answer(
    question: &Question,
    line: &str,
    termination: &AskQuestionTermination,
) -> Result<Option<Answer>, AskQuestionResult> {
    if !question.allow_custom {
        return Ok(None);
    }
    let mut chars = line.chars();
    let Some(first) = chars.next() else {
        return Ok(None);
    };
    if !first.eq_ignore_ascii_case(&'c') {
        return Ok(None);
    }
    let mut text = chars.as_str().trim().to_string();
    if text.is_empty() {
        eprint!("{}", tr("question.custom_prompt", &[]));
        text = read_one_line(termination).await?.trim().to_string();
    }
    if text.is_empty() || text.chars().count() > 500 {
        eprintln!("{}", tr("question.custom_invalid", &[]));
        return Ok(None);
    }
    Ok(Some(Answer {
        question_id: question.id.clone(),
        option_ids: vec![CUSTOM_OPTION_ID.into()],
        custom_text: Some(text),
        skipped: false,
        picked_recommended: false,
    }))
}

fn parse_single_choice_answer(question: &Question, line: &str) -> Option<Answer> {
    let number = line.parse::<usize>().ok()?;
    if !(1..=question.options.len()).contains(&number) {
        return None;
    }
    let picked: &QuestionOption = &question.options[number - 1];
    Some(Answer {
        question_id: question.id.clone(),
        option_ids: vec![picked.id.clone()],
        custom_text: None,
        skipped: false,
        picked_recommended: picked.recommended,
    })
}

async fn read_one_line(termination: &AskQuestionTermination) -> Result<String, AskQuestionResult> {
    let termination = termination.clone();
    tokio::task::spawn_blocking(move || {
        panel_result(stdin::read_line_cancellable(|| termination.result()))
    })
    .await
    .unwrap_or_else(|_| {
        Err(AskQuestionResult::terminal(
            AskQuestionOutcome::HostDisconnected,
        ))
    })
}

fn panel_result(
    read: Result<String, stdin::ReadLineError<AskQuestionResult>>,
) -> Result<String, AskQuestionResult> {
    read.map_err(|error| match error {
        stdin::ReadLineError::Stopped(result) => result,
        stdin::ReadLineError::Disconnected => {
            AskQuestionResult::terminal(AskQuestionOutcome::HostDisconnected)
        }
    })
}

#[cfg(all(test, unix))]
pub(crate) async fn read_one_line_from_fd_for_test(
    fd: std::os::fd::RawFd,
    termination: AskQuestionTermination,
) -> Result<String, AskQuestionResult> {
    tokio::task::spawn_blocking(move || {
        // SAFETY: the test keeps fd open until this read returns.
        let input = unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) };
        panel_result(stdin::read_line_from(input, || termination.result()))
    })
    .await
    .unwrap_or_else(|_| {
        Err(AskQuestionResult::terminal(
            AskQuestionOutcome::HostDisconnected,
        ))
    })
}

#[cfg(all(test, windows))]
pub(crate) async fn read_one_line_from_pipe_for_test(
    reader: std::sync::Arc<std::io::PipeReader>,
    termination: AskQuestionTermination,
) -> Result<String, AskQuestionResult> {
    tokio::task::spawn_blocking(move || {
        let input = std::os::windows::io::AsHandle::as_handle(&*reader);
        panel_result(stdin::read_line_from(input, || termination.result()))
    })
    .await
    .unwrap_or_else(|_| {
        Err(AskQuestionResult::terminal(
            AskQuestionOutcome::HostDisconnected,
        ))
    })
}

#[cfg(test)]
#[path = "tests/cli_ask_question_panel_test.rs"]
mod tests;
