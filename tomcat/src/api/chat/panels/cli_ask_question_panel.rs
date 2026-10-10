use crate::infra::i18n::tr;
use async_trait::async_trait;

use crate::core::plan_runtime::panels::{
    Answer, AskQuestionOutcome, AskQuestionPanel, AskQuestionResult, AskQuestionTermination,
    Question, QuestionOption, CUSTOM_OPTION_ID,
};

const ASK_QUESTION_TEST_AUTO_PICK_ENV: &str = "TOMCAT_ASK_QUESTION_TEST_AUTO_PICK";

/// CLI panel with one cancel-safe stdin owner. On Unix `poll` bounds every read wait, so an
/// interrupt releases ownership before the chat prompt starts reading the next line.
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
    tokio::task::spawn_blocking(move || read_one_line_owned(&termination))
        .await
        .unwrap_or_else(|_| {
            Err(AskQuestionResult::terminal(
                AskQuestionOutcome::HostDisconnected,
            ))
        })
}

#[cfg(unix)]
fn read_one_line_owned(termination: &AskQuestionTermination) -> Result<String, AskQuestionResult> {
    read_one_line_from_fd_owned(libc::STDIN_FILENO, termination)
}

#[cfg(unix)]
fn read_one_line_from_fd_owned(
    fd: std::os::fd::RawFd,
    termination: &AskQuestionTermination,
) -> Result<String, AskQuestionResult> {
    static STDIN_OWNER: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    let _owner = STDIN_OWNER
        .get_or_init(|| std::sync::Mutex::new(()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    #[cfg(unix)]
    {
        let mut bytes = Vec::new();
        loop {
            if let Some(result) = termination.result() {
                return Err(result);
            }
            let mut descriptor = libc::pollfd {
                fd,
                events: libc::POLLIN | libc::POLLHUP,
                revents: 0,
            };
            // SAFETY: descriptor points to one initialized pollfd for the whole call.
            let ready = unsafe { libc::poll(&mut descriptor, 1, 50) };
            if ready < 0 {
                let error = std::io::Error::last_os_error();
                if error.kind() == std::io::ErrorKind::Interrupted {
                    continue;
                }
                return Err(AskQuestionResult::terminal(
                    AskQuestionOutcome::HostDisconnected,
                ));
            }
            if ready == 0 {
                continue;
            }
            let mut byte = 0u8;
            // SAFETY: byte is writable and stdin is a process-owned file descriptor.
            let count = unsafe { libc::read(fd, (&mut byte as *mut u8).cast::<libc::c_void>(), 1) };
            if count <= 0 {
                return Err(AskQuestionResult::terminal(
                    AskQuestionOutcome::HostDisconnected,
                ));
            }
            bytes.push(byte);
            if byte == b'\n' {
                return String::from_utf8(bytes).map_err(|_| {
                    AskQuestionResult::terminal(AskQuestionOutcome::HostDisconnected)
                });
            }
        }
    }

    #[cfg(not(unix))]
    {
        let mut line = String::new();
        let read =
            std::io::BufRead::read_line(&mut std::io::BufReader::new(std::io::stdin()), &mut line)
                .map_err(|_| AskQuestionResult::terminal(AskQuestionOutcome::HostDisconnected))?;
        if read == 0 {
            Err(AskQuestionResult::terminal(
                AskQuestionOutcome::HostDisconnected,
            ))
        } else {
            Ok(line)
        }
    }
}

#[cfg(not(unix))]
fn read_one_line_owned(termination: &AskQuestionTermination) -> Result<String, AskQuestionResult> {
    if let Some(result) = termination.result() {
        return Err(result);
    }
    let mut line = String::new();
    let read =
        std::io::BufRead::read_line(&mut std::io::BufReader::new(std::io::stdin()), &mut line)
            .map_err(|_| AskQuestionResult::terminal(AskQuestionOutcome::HostDisconnected))?;
    if read == 0 {
        Err(AskQuestionResult::terminal(
            AskQuestionOutcome::HostDisconnected,
        ))
    } else {
        Ok(line)
    }
}

#[cfg(all(test, unix))]
pub(crate) async fn read_one_line_from_fd_for_test(
    fd: std::os::fd::RawFd,
    termination: AskQuestionTermination,
) -> Result<String, AskQuestionResult> {
    tokio::task::spawn_blocking(move || read_one_line_from_fd_owned(fd, &termination))
        .await
        .unwrap_or_else(|_| {
            Err(AskQuestionResult::terminal(
                AskQuestionOutcome::HostDisconnected,
            ))
        })
}

#[cfg(test)]
mod custom_answer_tests {
    use super::*;

    #[tokio::test]
    async fn cli_custom_answer_counts_characters_not_utf8_bytes() {
        let question = Question {
            allow_custom: true,
            id: "q1".into(),
            prompt: "Choose".into(),
            options: vec![
                QuestionOption {
                    id: "a".into(),
                    label: "A".into(),
                    recommended: true,
                },
                QuestionOption {
                    id: "b".into(),
                    label: "B".into(),
                    recommended: false,
                },
            ],
        };
        for glyph in ["a", "中", "🙂"] {
            let text = glyph.repeat(500);
            let answer = parse_custom_answer(
                &question,
                &format!("c {text}"),
                &AskQuestionTermination::default(),
            )
            .await
            .unwrap()
            .unwrap();
            assert_eq!(answer.custom_text.as_deref(), Some(text.as_str()));
            assert!(parse_custom_answer(
                &question,
                &format!("c {}", glyph.repeat(501)),
                &AskQuestionTermination::default()
            )
            .await
            .unwrap()
            .is_none());
        }
    }
}
