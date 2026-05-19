//! # `AskQuestionPanel` — `ask_question` 的 UI 后端 trait + 三个实现
//!
//! - `CliAskQuestionPanel`：CLI MVP（plan §决策：T2-P0-008 TUI 未交付，采用 readline + spawn_blocking）。
//! - `IdeAskQuestionPanel`：占位 stub；与 T2-P0-008 联动后由 TUI 实现。
//! - `MockAskQuestionPanel`：测试专用，队列模式回填预设答案；监听 `cancel_signal`。
//!
//! UI 兜底：渲染时**自动**在每题末尾追加一项 `{ id="__custom__", label="自定义…" }`；
//! 推荐项 label 后缀「— 推荐」；选中 `__custom__` 时必须捕获 `custom_text`（非空、≤500）。

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

/// 保留 option id；LLM 不得显式声明此 id；UI 端 panel 自动追加同 id 的兜底槽。
pub const CUSTOM_OPTION_ID: &str = "__custom__";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Question {
    pub id: String,
    pub prompt: String,
    #[serde(default)]
    pub allow_multiple: bool,
    pub options: Vec<QuestionOption>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuestionOption {
    pub id: String,
    pub label: String,
    #[serde(default)]
    pub recommended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Answer {
    pub question_id: String,
    pub option_ids: Vec<String>,
    /// 选中 `__custom__` 时必带（非空，≤ 500）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_text: Option<String>,
    /// 用户最终勾选中是否包含 `recommended: true` 的那一项。
    pub picked_recommended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AskQuestionResult {
    pub answers: Vec<Answer>,
    /// 用户中断 / panel 自动取消时为 true；此时 answers 为空。
    #[serde(default)]
    pub cancelled: bool,
}

#[async_trait]
pub trait AskQuestionPanel: Send + Sync {
    /// 接受 LLM 提交的题目，阻塞 UI 直到用户答完或取消；
    /// 实现方应监听 `cancel_signal`，命中时立即返回 `cancelled: true`。
    async fn ask(
        &self,
        questions: Vec<Question>,
        cancel_signal: Arc<AtomicBool>,
    ) -> AskQuestionResult;
}

// ─── CliAskQuestionPanel ────────────────────────────────────────────────────

/// CLI MVP：通过 stdin 编号选择。
///
/// **限制**：
/// - 不在 TUI 侧边栏渲染（T2-P0-008 未交付）；走 stderr + readline。
/// - 多选输入逗号分隔编号（`1,3` / `1`）；输入 `c` 走自定义槽（要求随后 1 行 free text）。
/// - 输入 `q` → cancelled。
///
/// 实际 stdin 读取走 `tokio::task::spawn_blocking`，避免在 runtime 内阻塞调度。
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
            // 渲染到 stderr，避免污染 chat 主流
            eprintln!("\n{}", q.prompt);
            for (i, opt) in q.options.iter().enumerate() {
                let suffix = if opt.recommended { " — 推荐" } else { "" };
                eprintln!("  {}. {}{}", i + 1, opt.label, suffix);
            }
            eprintln!("  c. 自定义…");
            eprintln!("  q. 取消");
            let allow_multi = q.allow_multiple;
            let prompt = if allow_multi { "多选(逗号分隔)/c/q > " } else { "单选/c/q > " };
            eprint!("{}", prompt);

            // 阻塞 stdin 读 1 行（spawn_blocking）
            let line = match read_one_line_blocking().await {
                Ok(line) => line,
                Err(_) => {
                    return AskQuestionResult {
                        answers: vec![],
                        cancelled: true,
                    }
                }
            };
            let line = line.trim();
            if line.eq_ignore_ascii_case("q") || cancel_signal.load(Ordering::Relaxed) {
                return AskQuestionResult {
                    answers: vec![],
                    cancelled: true,
                };
            }
            if line.eq_ignore_ascii_case("c") {
                eprint!("自定义内容（1-500 字符）> ");
                let text = match read_one_line_blocking().await {
                    Ok(t) => t.trim().to_string(),
                    Err(_) => {
                        return AskQuestionResult {
                            answers: vec![],
                            cancelled: true,
                        }
                    }
                };
                if text.is_empty() || text.len() > 500 {
                    eprintln!("(无效自定义文本，回到选项)");
                    continue;
                }
                answers.push(Answer {
                    question_id: q.id.clone(),
                    option_ids: vec![CUSTOM_OPTION_ID.into()],
                    custom_text: Some(text),
                    picked_recommended: false,
                });
                continue;
            }
            // 解析编号
            let mut picks: Vec<&QuestionOption> = Vec::new();
            for tok in line.split(',') {
                let tok = tok.trim();
                if tok.is_empty() {
                    continue;
                }
                let Ok(n) = tok.parse::<usize>() else {
                    continue;
                };
                if n >= 1 && n <= q.options.len() {
                    picks.push(&q.options[n - 1]);
                }
            }
            if picks.is_empty() {
                eprintln!("(无效输入，请重试)");
                continue;
            }
            if !allow_multi {
                picks.truncate(1);
            }
            let picked_recommended = picks.iter().any(|p| p.recommended);
            answers.push(Answer {
                question_id: q.id.clone(),
                option_ids: picks.iter().map(|p| p.id.clone()).collect(),
                custom_text: None,
                picked_recommended,
            });
        }
        AskQuestionResult {
            answers,
            cancelled: false,
        }
    }
}

async fn read_one_line_blocking() -> std::io::Result<String> {
    tokio::task::spawn_blocking(|| {
        let mut line = String::new();
        std::io::BufRead::read_line(&mut std::io::BufReader::new(std::io::stdin()), &mut line)?;
        Ok(line)
    })
    .await
    .unwrap_or_else(|_| Ok(String::new()))
}

// ─── IdeAskQuestionPanel（占位） ────────────────────────────────────────────

/// IDE 端 stub。当前直接 cancelled；T2-P0-008 TUI 接入后由实际 panel 替换。
pub struct IdeAskQuestionPanel;

#[async_trait]
impl AskQuestionPanel for IdeAskQuestionPanel {
    async fn ask(
        &self,
        _questions: Vec<Question>,
        _cancel_signal: Arc<AtomicBool>,
    ) -> AskQuestionResult {
        AskQuestionResult {
            answers: vec![],
            cancelled: true,
        }
    }
}

// ─── MockAskQuestionPanel ──────────────────────────────────────────────────

/// 测试专用：构造时给一组预编排的 `AskQuestionResult`；按调用顺序返回。
///
/// **不**在 cargo feature gate 后；plan §测试策略：MockAskQuestionPanel 是
/// L1/L2 集成测必要 fixture，需要随 lib 编译。
pub struct MockAskQuestionPanel {
    queue: parking_lot::Mutex<Vec<AskQuestionResult>>,
    /// 测试用：每次调用前 sleep 多久；可用于验证阻塞语义。
    delay: Option<std::time::Duration>,
    /// 命中 cancel_signal 时是否直接返回 cancelled（默认 true）。
    honor_cancel: bool,
}

impl MockAskQuestionPanel {
    pub fn new(results: Vec<AskQuestionResult>) -> Self {
        Self {
            queue: parking_lot::Mutex::new(results),
            delay: None,
            honor_cancel: true,
        }
    }

    pub fn with_delay(mut self, d: std::time::Duration) -> Self {
        self.delay = Some(d);
        self
    }

    pub fn ignore_cancel(mut self) -> Self {
        self.honor_cancel = false;
        self
    }

    /// 测试断言：未消费的预编排答案数。
    pub fn remaining(&self) -> usize {
        self.queue.lock().len()
    }
}

#[async_trait]
impl AskQuestionPanel for MockAskQuestionPanel {
    async fn ask(
        &self,
        _questions: Vec<Question>,
        cancel_signal: Arc<AtomicBool>,
    ) -> AskQuestionResult {
        if self.honor_cancel && cancel_signal.load(Ordering::Relaxed) {
            return AskQuestionResult {
                answers: vec![],
                cancelled: true,
            };
        }
        if let Some(d) = self.delay {
            // 等待时检测 cancel，避免测试 hang
            let start = std::time::Instant::now();
            while start.elapsed() < d {
                if self.honor_cancel && cancel_signal.load(Ordering::Relaxed) {
                    return AskQuestionResult {
                        answers: vec![],
                        cancelled: true,
                    };
                }
                tokio::time::sleep(std::time::Duration::from_millis(5)).await;
            }
        }
        let mut q = self.queue.lock();
        if q.is_empty() {
            AskQuestionResult {
                answers: vec![],
                cancelled: true,
            }
        } else {
            q.remove(0)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_question() -> Question {
        Question {
            id: "q1".into(),
            prompt: "测试".into(),
            allow_multiple: false,
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
        }
    }

    #[tokio::test]
    async fn mock_panel_returns_queued_answer() {
        let res = AskQuestionResult {
            answers: vec![Answer {
                question_id: "q1".into(),
                option_ids: vec!["a".into()],
                custom_text: None,
                picked_recommended: true,
            }],
            cancelled: false,
        };
        let panel = MockAskQuestionPanel::new(vec![res]);
        let out = panel
            .ask(vec![dummy_question()], Arc::new(AtomicBool::new(false)))
            .await;
        assert!(!out.cancelled);
        assert_eq!(out.answers[0].option_ids[0], "a");
        assert!(out.answers[0].picked_recommended);
        assert_eq!(panel.remaining(), 0);
    }

    #[tokio::test]
    async fn mock_panel_returns_cancelled_when_signal_set() {
        let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
            answers: vec![],
            cancelled: false,
        }]);
        let cancel = Arc::new(AtomicBool::new(true));
        let out = panel.ask(vec![dummy_question()], cancel).await;
        assert!(out.cancelled);
        assert!(out.answers.is_empty());
        assert_eq!(panel.remaining(), 1, "未消费队列");
    }

    #[tokio::test]
    async fn mock_panel_blocks_until_answered_with_delay() {
        let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
            answers: vec![],
            cancelled: false,
        }])
        .with_delay(std::time::Duration::from_millis(50));
        let start = std::time::Instant::now();
        let _ = panel
            .ask(vec![dummy_question()], Arc::new(AtomicBool::new(false)))
            .await;
        assert!(start.elapsed() >= std::time::Duration::from_millis(40));
    }

    #[tokio::test]
    async fn mock_panel_honors_cancel_mid_wait() {
        let panel = MockAskQuestionPanel::new(vec![AskQuestionResult {
            answers: vec![],
            cancelled: false,
        }])
        .with_delay(std::time::Duration::from_secs(5));
        let cancel = Arc::new(AtomicBool::new(false));
        let cancel_clone = Arc::clone(&cancel);
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(30)).await;
            cancel_clone.store(true, Ordering::Relaxed);
        });
        let out = panel.ask(vec![dummy_question()], cancel).await;
        assert!(out.cancelled);
    }
}
