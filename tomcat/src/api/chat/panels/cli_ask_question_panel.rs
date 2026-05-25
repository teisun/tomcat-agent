use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use crate::core::plan_runtime::panels::{
    Answer, AskQuestionPanel, AskQuestionResult, Question, QuestionOption, CUSTOM_OPTION_ID,
};

const ASK_QUESTION_TEST_AUTO_PICK_ENV: &str = "TOMCAT_ASK_QUESTION_TEST_AUTO_PICK";

/// CLI MVP：通过 stdin 编号选择。
pub struct CliAskQuestionPanel;

#[async_trait]
impl AskQuestionPanel for CliAskQuestionPanel {
    async fn ask(
        &self,
        questions: Vec<Question>,
        cancel_signal: Arc<AtomicBool>,
    ) -> AskQuestionResult {
        let mut answers = Vec::with_capacity(questions.len());
        for q in &questions {
            if cancel_signal.load(Ordering::Relaxed) {
                return AskQuestionResult {
                    answers: vec![],
                    cancelled: true,
                };
            }
            if let Some(answer) = auto_pick_answer_for_test(q) {
                let picked = answer.option_ids.join(",");
                eprintln!(
                    "[ask_question:auto-pick] qid={} strategy=recommended picks={picked}",
                    q.id
                );
                answers.push(answer);
                continue;
            }
            loop {
                if cancel_signal.load(Ordering::Relaxed) {
                    return AskQuestionResult {
                        answers: vec![],
                        cancelled: true,
                    };
                }
                eprintln!("\n{}", q.prompt);
                for (i, opt) in q.options.iter().enumerate() {
                    let suffix = if opt.recommended { " — 推荐" } else { "" };
                    eprintln!("  {}. {}{}", i + 1, opt.label, suffix);
                }
                eprintln!("  c. 自定义…");
                eprintln!("  skip. 跳过本题");
                eprint!("单选/c/skip > ");

                let line = match read_one_line_blocking().await {
                    Ok(line) => line,
                    Err(_) => {
                        return AskQuestionResult {
                            answers: vec![],
                            cancelled: true,
                        };
                    }
                };
                if cancel_signal.load(Ordering::Relaxed) {
                    return AskQuestionResult {
                        answers: vec![],
                        cancelled: true,
                    };
                }
                let line = line.trim();
                if line.eq_ignore_ascii_case("skip") {
                    answers.push(Answer {
                        question_id: q.id.clone(),
                        option_ids: vec![],
                        custom_text: None,
                        skipped: true,
                        picked_recommended: false,
                    });
                    break;
                }
                if let Some(answer) = parse_custom_answer(q, line).await {
                    answers.push(answer);
                    break;
                }
                if let Some(answer) = parse_single_choice_answer(q, line) {
                    answers.push(answer);
                    break;
                }
                eprintln!("(无法识别，请重试当前题)");
            }
        }
        AskQuestionResult {
            answers,
            cancelled: false,
        }
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
        .find(|opt| opt.recommended)
        .or_else(|| question.options.first())?;
    Some(Answer {
        question_id: question.id.clone(),
        option_ids: vec![picked.id.clone()],
        custom_text: None,
        skipped: false,
        picked_recommended: picked.recommended,
    })
}

async fn parse_custom_answer(question: &Question, line: &str) -> Option<Answer> {
    let mut chars = line.chars();
    let Some(first) = chars.next() else {
        return None;
    };
    if !first.eq_ignore_ascii_case(&'c') {
        return None;
    }
    let mut text = chars.as_str().trim().to_string();
    if text.is_empty() {
        eprint!("自定义内容（1-500 字符）> ");
        text = match read_one_line_blocking().await {
            Ok(t) => t.trim().to_string(),
            Err(_) => return None,
        };
    }
    if text.is_empty() || text.len() > 500 {
        eprintln!("(无效自定义文本，请重试当前题)");
        return None;
    }
    Some(Answer {
        question_id: question.id.clone(),
        option_ids: vec![CUSTOM_OPTION_ID.into()],
        custom_text: Some(text),
        skipped: false,
        picked_recommended: false,
    })
}

fn parse_single_choice_answer(question: &Question, line: &str) -> Option<Answer> {
    let n = line.parse::<usize>().ok()?;
    if !(1..=question.options.len()).contains(&n) {
        return None;
    }
    let picked: &QuestionOption = &question.options[n - 1];
    Some(Answer {
        question_id: question.id.clone(),
        option_ids: vec![picked.id.clone()],
        custom_text: None,
        skipped: false,
        picked_recommended: picked.recommended,
    })
}

async fn read_one_line_blocking() -> std::io::Result<String> {
    tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        let n =
            std::io::BufRead::read_line(&mut std::io::BufReader::new(std::io::stdin()), &mut line)?;
        if n == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::UnexpectedEof,
                "stdin closed",
            ));
        }
        Ok(line)
    })
    .await
    .unwrap_or_else(|_| Ok(String::new()))
}
