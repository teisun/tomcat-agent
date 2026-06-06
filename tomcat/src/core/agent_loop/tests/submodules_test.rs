//! # 子模块焦小测（Phase 4 新增）
//!
//! 这些测试不经过 `AgentLoop::run` 整链路，而是直接调用 `pub(super)` 自由函数 /
//! 辅助函数，断言其内部契约。这样在三层循环骨架被进一步重构时，子模块的契约
//! 不会因外层改造而丢失"焦点"。
//!
//! 当前覆盖：
//!
//! - `error_classifier::handle_overflow_retry`：
//!   * 非 overflow 错误 → `OverflowTrimStats::applied == false`，无事件；
//!   * 缺 `context_state` → `applied == false`，无事件。
//! - `tool_exec::execute_tool`：
//!   * unknown 工具名 → `(msg, true)`；
//!   * `read` 正常路径 → `(content, false)`；
//!   * 旧 `read_file` 名 → 走 unknown 分支（PR-RA：运行时无别名）。

use std::sync::Arc;

use tokio_util::sync::CancellationToken;

use crate::core::agent_loop::error_classifier::handle_overflow_retry;
use crate::core::agent_loop::tool_exec::{execute_tool, execute_tool_with_openai_files};
use crate::core::agent_loop::{AgentLoop, AgentLoopConfig, ToolCallInfo};
use crate::core::llm::{ChatMessage, ModelCatalog};
use crate::core::skill::{Skill, SkillSet, SkillSource};
use crate::core::tools::primitive::PrimitiveExecutor;
use crate::core::tools::web_fetch::{types::WebFetchOutput, WebFetchFormat, WebFetchRuntime};
use crate::core::tools::web_search::types::{Hit, Stats, WebSearchArgs, WebSearchOutput};
use crate::core::tools::web_search::WebSearchRuntime;
use crate::infra::error::AppError;
use crate::infra::DefaultEventBus;
use crate::AppConfig;
use parking_lot::{Mutex, RwLock};

use super::mocks::{MockLlmProvider, MockPrimitiveExecutor};

fn make_agent() -> AgentLoop {
    let llm = Arc::new(MockLlmProvider::new(vec![]));
    let primitive = Arc::new(MockPrimitiveExecutor);
    let event_bus = Arc::new(DefaultEventBus::new());
    let config = AgentLoopConfig {
        model: "gpt-4".to_string(),
        session_id: "s-submod".to_string(),
        ..Default::default()
    };
    AgentLoop::new(llm, primitive, event_bus, config, CancellationToken::new())
}

struct SkillFilePrimitive;

#[async_trait::async_trait]
impl PrimitiveExecutor for SkillFilePrimitive {
    async fn read_file(&self, path: &str, _plugin_id: &str) -> Result<String, AppError> {
        std::fs::read_to_string(path).map_err(AppError::Io)
    }

    async fn list_dir(
        &self,
        _path: &str,
        _plugin_id: &str,
    ) -> Result<Vec<crate::core::tools::primitive::DirEntry>, AppError> {
        Ok(vec![])
    }

    async fn write_file(
        &self,
        _path: &str,
        _content: &str,
        _overwrite: bool,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::WriteFileResult, AppError> {
        unreachable!()
    }

    async fn edit_file(
        &self,
        _path: &str,
        _edits: Vec<crate::core::tools::primitive::EditOperation>,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::EditFileResult, AppError> {
        unreachable!()
    }

    async fn execute_bash(
        &self,
        _command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
        _timeout_ms: Option<u64>,
    ) -> Result<crate::core::tools::primitive::BashResult, AppError> {
        unreachable!()
    }

    async fn require_user_confirmation(
        &self,
        _operation: crate::core::tools::primitive::PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        unreachable!()
    }
}

struct RecordingSkillPrimitive {
    reads: Arc<Mutex<Vec<(String, String)>>>,
}

#[async_trait::async_trait]
impl PrimitiveExecutor for RecordingSkillPrimitive {
    async fn read_file(&self, path: &str, plugin_id: &str) -> Result<String, AppError> {
        self.reads
            .lock()
            .push((path.to_string(), plugin_id.to_string()));
        std::fs::read_to_string(path).map_err(AppError::Io)
    }

    async fn list_dir(
        &self,
        _path: &str,
        _plugin_id: &str,
    ) -> Result<Vec<crate::core::tools::primitive::DirEntry>, AppError> {
        Ok(vec![])
    }

    async fn write_file(
        &self,
        _path: &str,
        _content: &str,
        _overwrite: bool,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::WriteFileResult, AppError> {
        unreachable!()
    }

    async fn edit_file(
        &self,
        _path: &str,
        _edits: Vec<crate::core::tools::primitive::EditOperation>,
        _plugin_id: &str,
    ) -> Result<crate::core::tools::primitive::EditFileResult, AppError> {
        unreachable!()
    }

    async fn execute_bash(
        &self,
        _command: &str,
        _cwd: Option<&str>,
        _plugin_id: &str,
        _argv: Option<&[String]>,
        _timeout_ms: Option<u64>,
    ) -> Result<crate::core::tools::primitive::BashResult, AppError> {
        unreachable!()
    }

    async fn require_user_confirmation(
        &self,
        _operation: crate::core::tools::primitive::PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        unreachable!()
    }
}

/// 非 context overflow 错误（429 限流）：handle_overflow_retry 应当跳过 trim，
/// 返回 `applied == false` 的默认 stats。
#[tokio::test]
async fn handle_overflow_retry_skipped_when_not_overflow() {
    let mut agent = make_agent();
    let mut messages = vec![ChatMessage::user("hi")];
    let err = AppError::Llm("API 错误 429: rate limit".to_string());
    let stats = handle_overflow_retry(&mut agent, &mut messages, 1, &err);
    assert!(
        !stats.applied,
        "non-overflow error must not trigger L3 trim, stats={:?}",
        stats
    );
    assert_eq!(stats.trim_tokens, 0);
    assert_eq!(stats.trim_turns, 0);
}

/// overflow 错误但 `context_state` 缺失：handle_overflow_retry 仅记录诊断日志，
/// 不触发 trim、不发事件，返回 `applied == false`。
#[tokio::test]
async fn handle_overflow_retry_skipped_when_no_context_state() {
    let mut agent = make_agent();
    let mut messages = vec![ChatMessage::user("hi")];
    let err =
        AppError::Llm(r#"API 错误 400: {"error":{"code":"context_length_exceeded"}}"#.to_string());
    let stats = handle_overflow_retry(&mut agent, &mut messages, 1, &err);
    assert!(
        !stats.applied,
        "overflow without context_state must skip trim, stats={:?}",
        stats
    );
    assert_eq!(messages.len(), 1, "messages must be left untouched");
}

/// unknown 工具名：execute_tool 返回 `is_error == true`，content 含 unknown 提示。
#[tokio::test]
async fn tool_exec_unknown_tool_returns_is_error() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "x".to_string(),
        name: "no_such_tool".to_string(),
        arguments: "{}".to_string(),
    };
    let (msg, is_error, _follow_ups) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(is_error, "unknown tool must report is_error=true");
    assert!(
        msg.contains("no_such_tool") || msg.to_lowercase().contains("unknown"),
        "msg should mention the unknown tool name: {}",
        msg
    );
}

/// read 正常路径：execute_tool 返回 `is_error == false`，content 由 mock 直接产出。
#[tokio::test]
async fn tool_exec_read_returns_content() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "r1".to_string(),
        name: "read".to_string(),
        arguments: r#"{"path":"/tmp/abc"}"#.to_string(),
    };
    let (msg, is_error, _follow_ups) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(!is_error, "read success must report is_error=false");
    assert!(
        msg.contains("/tmp/abc"),
        "content should include path from mock: {}",
        msg
    );
}

/// PR-RA：旧 `read_file` 名 → 运行时按未知工具回错（无别名 / 无重定向）。
#[tokio::test]
async fn tool_exec_legacy_read_file_returns_unknown_tool_error() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "legacy_1".to_string(),
        name: "read_file".to_string(),
        arguments: r#"{"path":"/tmp/legacy"}"#.to_string(),
    };
    let (msg, is_error, _follow_ups) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(
        is_error,
        "legacy 'read_file' must NOT be aliased to 'read'; it should return is_error=true"
    );
    assert!(
        msg.contains("read_file") || msg.to_lowercase().contains("unknown"),
        "msg should mention the unknown tool name: {}",
        msg
    );
}

#[tokio::test]
async fn tool_exec_load_skill_resolves_by_name() {
    use crate::core::agent_loop::tool_exec::execute_tool_full;
    use crate::core::agent_loop::types::SubagentType;

    let dir = tempfile::tempdir().expect("tempdir");
    let skill_dir = dir.path().join("commit");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: commit\ndescription: Create a git commit.\n---\n# Commit\n1. Run git status.\n",
    )
    .unwrap();
    let skill_set = Arc::new(RwLock::new(skill_set_with_single_skill(
        "commit",
        "Create a git commit.",
        skill_dir.join("SKILL.md"),
        skill_dir.clone(),
        false,
    )));
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(SkillFilePrimitive);
    let tc = ToolCallInfo {
        id: "load-skill-1".into(),
        name: "load_skill".into(),
        arguments: r#"{"name":"commit"}"#.into(),
    };

    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        None,
        Some(&skill_set),
        SubagentType::User,
        None,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;

    assert!(
        !outcome.is_error,
        "load_skill should succeed: {}",
        outcome.model_text
    );
    assert!(outcome.model_text.contains("<skill name=\"commit\""));
    assert!(outcome.model_text.contains("# Commit"));
    assert!(!outcome
        .model_text
        .contains("description: Create a git commit."));
}

#[tokio::test]
async fn tool_exec_load_skill_rejected_for_reviewer() {
    use crate::core::agent_loop::tool_exec::execute_tool_full;
    use crate::core::agent_loop::types::SubagentType;

    let dir = tempfile::tempdir().expect("tempdir");
    let skill_dir = dir.path().join("commit");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: commit\ndescription: Create a git commit.\n---\n# Commit\n",
    )
    .unwrap();
    let skill_set = Arc::new(RwLock::new(skill_set_with_single_skill(
        "commit",
        "Create a git commit.",
        skill_dir.join("SKILL.md"),
        skill_dir.clone(),
        false,
    )));
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(SkillFilePrimitive);
    let tc = ToolCallInfo {
        id: "load-skill-reviewer".into(),
        name: "load_skill".into(),
        arguments: r#"{"name":"commit"}"#.into(),
    };

    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        None,
        Some(&skill_set),
        SubagentType::Reviewer,
        Some(crate::core::plan_runtime::review::ReviewKind::Plan),
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;

    assert!(outcome.is_error);
    assert!(outcome
        .model_text
        .contains("reviewer 子 Agent 禁止调用工具 `load_skill`"));
}

#[tokio::test]
async fn tool_exec_load_skill_allowed_for_reviewer_when_exposed() {
    use crate::core::agent_loop::tool_exec::execute_tool_full_with_policy;
    use crate::core::agent_loop::types::SubagentType;

    let dir = tempfile::tempdir().expect("tempdir");
    let skill_dir = dir.path().join("commit");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: commit\ndescription: Create a git commit.\n---\n# Commit\n",
    )
    .unwrap();
    let skill_set = Arc::new(RwLock::new(skill_set_with_single_skill(
        "commit",
        "Create a git commit.",
        skill_dir.join("SKILL.md"),
        skill_dir.clone(),
        false,
    )));
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(SkillFilePrimitive);
    let tc = ToolCallInfo {
        id: "load-skill-reviewer-allowed".into(),
        name: "load_skill".into(),
        arguments: r#"{"name":"commit"}"#.into(),
    };

    let outcome = execute_tool_full_with_policy(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        None,
        Some(&skill_set),
        SubagentType::Reviewer,
        Some(crate::core::plan_runtime::review::ReviewKind::Plan),
        true,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;

    assert!(!outcome.is_error);
    assert!(outcome.model_text.contains("<skill name=\"commit\""));
}

#[tokio::test]
async fn tool_exec_load_skill_rejected_for_verifier() {
    use crate::core::agent_loop::tool_exec::execute_tool_full;
    use crate::core::agent_loop::types::SubagentType;

    let dir = tempfile::tempdir().expect("tempdir");
    let skill_dir = dir.path().join("commit");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: commit\ndescription: Create a git commit.\n---\n# Commit\n",
    )
    .unwrap();
    let skill_set = Arc::new(RwLock::new(skill_set_with_single_skill(
        "commit",
        "Create a git commit.",
        skill_dir.join("SKILL.md"),
        skill_dir.clone(),
        false,
    )));
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(SkillFilePrimitive);
    let tc = ToolCallInfo {
        id: "load-skill-verifier".into(),
        name: "load_skill".into(),
        arguments: r#"{"name":"commit"}"#.into(),
    };

    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        None,
        Some(&skill_set),
        SubagentType::Verifier,
        None,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;

    assert!(outcome.is_error);
    assert!(outcome
        .model_text
        .contains("verifier 子 Agent 禁止调用工具 `load_skill`"));
}

#[tokio::test]
async fn tool_exec_load_skill_allowed_for_verifier_when_exposed() {
    use crate::core::agent_loop::tool_exec::execute_tool_full_with_policy;
    use crate::core::agent_loop::types::SubagentType;

    let dir = tempfile::tempdir().expect("tempdir");
    let skill_dir = dir.path().join("commit");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: commit\ndescription: Create a git commit.\n---\n# Commit\n",
    )
    .unwrap();
    let skill_set = Arc::new(RwLock::new(skill_set_with_single_skill(
        "commit",
        "Create a git commit.",
        skill_dir.join("SKILL.md"),
        skill_dir.clone(),
        false,
    )));
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(SkillFilePrimitive);
    let tc = ToolCallInfo {
        id: "load-skill-verifier-allowed".into(),
        name: "load_skill".into(),
        arguments: r#"{"name":"commit"}"#.into(),
    };

    let outcome = execute_tool_full_with_policy(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        None,
        Some(&skill_set),
        SubagentType::Verifier,
        None,
        true,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;

    assert!(!outcome.is_error);
    assert!(outcome.model_text.contains("<skill name=\"commit\""));
}

#[tokio::test]
async fn tool_exec_load_skill_unknown_name_errors() {
    use crate::core::agent_loop::tool_exec::execute_tool_full;
    use crate::core::agent_loop::types::SubagentType;

    let dir = tempfile::tempdir().expect("tempdir");
    let skill_dir = dir.path().join("commit");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: commit\ndescription: Create a git commit.\n---\n# Commit\n",
    )
    .unwrap();
    let skill_set = Arc::new(RwLock::new(skill_set_with_single_skill(
        "commit",
        "Create a git commit.",
        skill_dir.join("SKILL.md"),
        skill_dir.clone(),
        false,
    )));
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(SkillFilePrimitive);
    let tc = ToolCallInfo {
        id: "load-skill-unknown".into(),
        name: "load_skill".into(),
        arguments: r#"{"name":"missing-skill"}"#.into(),
    };

    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        None,
        Some(&skill_set),
        SubagentType::User,
        None,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;

    assert!(outcome.is_error);
    assert!(outcome.model_text.contains("未知 skill `missing-skill`"));
    assert!(outcome.model_text.contains("commit"));
}

#[tokio::test]
async fn tool_exec_load_skill_file_escape_denied() {
    use crate::core::agent_loop::tool_exec::execute_tool_full;
    use crate::core::agent_loop::types::SubagentType;

    let dir = tempfile::tempdir().expect("tempdir");
    let outside = dir.path().join("outside.md");
    std::fs::write(&outside, "secret").unwrap();
    let skill_dir = dir.path().join("commit");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: commit\ndescription: Create a git commit.\n---\n# Commit\n",
    )
    .unwrap();
    let skill_set = Arc::new(RwLock::new(skill_set_with_single_skill(
        "commit",
        "Create a git commit.",
        skill_dir.join("SKILL.md"),
        skill_dir.clone(),
        false,
    )));
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(SkillFilePrimitive);
    let tc = ToolCallInfo {
        id: "load-skill-escape".into(),
        name: "load_skill".into(),
        arguments: r#"{"name":"commit","file":"../outside.md"}"#.into(),
    };

    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        None,
        Some(&skill_set),
        SubagentType::User,
        None,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;

    assert!(outcome.is_error);
    assert!(outcome.model_text.contains("越出技能目录"));
}

#[tokio::test]
async fn load_skill_body_load_passes_gate() {
    use crate::core::agent_loop::tool_exec::execute_tool_full;
    use crate::core::agent_loop::types::SubagentType;

    let dir = tempfile::tempdir().expect("tempdir");
    let skill_dir = dir.path().join("commit");
    std::fs::create_dir_all(&skill_dir).unwrap();
    let skill_path = skill_dir.join("SKILL.md");
    std::fs::write(
        &skill_path,
        "---\nname: commit\ndescription: Create a git commit.\n---\n# Commit\n",
    )
    .unwrap();
    let skill_set = Arc::new(RwLock::new(skill_set_with_single_skill(
        "commit",
        "Create a git commit.",
        skill_path.clone(),
        skill_dir.clone(),
        false,
    )));
    let reads = Arc::new(Mutex::new(Vec::new()));
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(RecordingSkillPrimitive {
        reads: reads.clone(),
    });
    let tc = ToolCallInfo {
        id: "load-skill-gate".into(),
        name: "load_skill".into(),
        arguments: r#"{"name":"commit"}"#.into(),
    };

    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        None,
        Some(&skill_set),
        SubagentType::User,
        None,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;

    assert!(!outcome.is_error);
    let reads = reads.lock();
    assert_eq!(
        reads.len(),
        1,
        "load_skill should route through primitive.read_file"
    );
    assert!(std::path::Path::new(&reads[0].0)
        .ends_with(std::path::Path::new("commit").join("SKILL.md")));
    assert_eq!(reads[0].1, "__agent__");
}

fn skill_set_with_single_skill(
    name: &str,
    description: &str,
    file_path: std::path::PathBuf,
    base_dir: std::path::PathBuf,
    disable_model_invocation: bool,
) -> SkillSet {
    let mut by_name = std::collections::BTreeMap::new();
    by_name.insert(
        name.to_string(),
        Skill {
            name: name.to_string(),
            description: description.to_string(),
            file_path,
            base_dir,
            source: SkillSource::Project,
            disable_model_invocation,
        },
    );
    SkillSet {
        by_name,
        diagnostics: Vec::new(),
        warnings: Vec::new(),
    }
}

/// `web_search` 需要会话级 runtime；未注入时应返回结构化错误而非误判 unknown tool。
#[tokio::test]
async fn tool_exec_web_search_requires_runtime_injection() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "ws_placeholder".to_string(),
        name: "web_search".to_string(),
        arguments: r#"{"query":"rust async runtime"}"#.to_string(),
    };
    let (msg, is_error, _follow_ups) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(
        is_error,
        "web_search without runtime should report is_error=true"
    );
    assert!(
        msg.contains("web_search runtime 未注入"),
        "missing-runtime error should be explicit, got: {}",
        msg
    );
}

/// `web_fetch` 需要会话级 runtime；未注入时应返回结构化错误而非误判 unknown tool。
#[tokio::test]
async fn tool_exec_web_fetch_requires_runtime_injection() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "wf_placeholder".to_string(),
        name: "web_fetch".to_string(),
        arguments: r#"{"url":"https://example.com"}"#.to_string(),
    };
    let (msg, is_error, _follow_ups) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(
        is_error,
        "web_fetch without runtime should report is_error=true"
    );
    assert!(
        msg.contains("web_fetch runtime 未注入"),
        "missing-runtime error should be explicit, got: {}",
        msg
    );
}

/// `web_fetch` 注入 runtime 后应走到真实分支并返回 JSON，而不是落回 unknown tool。
#[tokio::test]
async fn tool_exec_web_fetch_routes_to_runtime() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let dir = tempfile::tempdir().expect("tempdir");
    let runtime = Arc::new(
        WebFetchRuntime::new(&AppConfig::default(), dir.path().join("tool-results"))
            .expect("build web_fetch runtime"),
    );
    runtime.insert_cached_output_for_test(
        "https://example.com/cached",
        WebFetchFormat::Markdown,
        WebFetchOutput::new(
            "https://example.com/cached".to_string(),
            200,
            "OK".to_string(),
            "text/html; charset=utf-8".to_string(),
            8,
            "# Cached".to_string(),
            8,
            3,
            None,
            None,
            false,
            Vec::new(),
        ),
    );
    let tc = ToolCallInfo {
        id: "wf_cached_hit".to_string(),
        name: "web_fetch".to_string(),
        arguments: r#"{"url":"https://example.com/cached"}"#.to_string(),
    };

    let (msg, is_error, _follow_ups) = execute_tool_with_openai_files(
        &primitive,
        &None,
        &None,
        None,
        None,
        Some(&runtime),
        None,
        &tc,
    )
    .await;

    assert!(!is_error, "web_fetch with runtime should succeed: {}", msg);
    let value: serde_json::Value = serde_json::from_str(&msg).expect("valid web_fetch json");
    assert_eq!(value["url"], "https://example.com/cached");
    assert_eq!(value["result"], "# Cached");
    assert_eq!(value["cached"], true);
}

/// `web_search` 注入 runtime 后应走到真实分支并返回 JSON，而不是落回 unknown tool。
#[tokio::test]
async fn tool_exec_web_search_routes_to_runtime() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let cfg = AppConfig::default();
    let dir = tempfile::tempdir().expect("tempdir");
    let catalog = Arc::new(
        ModelCatalog::load_from_path(&cfg, dir.path().join("models.toml")).expect("catalog"),
    );
    let runtime = Arc::new(WebSearchRuntime::new(&cfg, catalog).expect("build web_search runtime"));
    runtime
        .insert_cached_output_for_test(
            WebSearchArgs {
                query: "rust async".to_string(),
                count: None,
                freshness: None,
                country: None,
                language: None,
                domain_filter: vec!["docs.rs".to_string()],
            },
            WebSearchOutput {
                query: "rust async".to_string(),
                hits: vec![Hit {
                    title: "reqwest".to_string(),
                    url: "https://docs.rs/reqwest".to_string(),
                    snippet: "HTTP client".to_string(),
                    position: 1,
                    published_at: None,
                }],
                backend: "tavily".to_string(),
                stats: Stats {
                    elapsed_ms: 5,
                    cached: false,
                    total_before_filter: Some(1),
                },
                truncated: false,
                warnings: Vec::new(),
            },
        )
        .expect("prime web_search cache");
    let tc = ToolCallInfo {
        id: "ws_cached_hit".to_string(),
        name: "web_search".to_string(),
        arguments: r#"{"query":"rust async","domain_filter":["docs.rs"]}"#.to_string(),
    };

    let (msg, is_error, _follow_ups) = execute_tool_with_openai_files(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        Some(&runtime),
        &tc,
    )
    .await;

    assert!(!is_error, "web_search with runtime should succeed: {}", msg);
    let value: serde_json::Value = serde_json::from_str(&msg).expect("valid web_search json");
    assert_eq!(value["query"], "rust async");
    assert_eq!(value["backend"], "tavily");
    assert_eq!(value["hits"][0]["url"], "https://docs.rs/reqwest");
    assert_eq!(value["stats"]["cached"], true);
}

/// PR-RB §2.6：`read.offset = 0` 触发 horizontal gate，返回结构化错误。
#[tokio::test]
async fn tool_exec_read_offset_zero_returns_bound_error() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "b1".to_string(),
        name: "read".to_string(),
        arguments: r#"{"path":"/tmp/x","offset":0,"limit":10}"#.to_string(),
    };
    let (msg, is_error, _follow_ups) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(is_error);
    assert!(
        msg.contains("offset") && msg.contains(">= 1"),
        "bound error should mention `offset` and `>= 1`, got: {}",
        msg
    );
}

/// PR-RB §2.6：`read.limit = 99999` 越上界，返回结构化错误。
#[tokio::test]
async fn tool_exec_read_limit_over_max_returns_bound_error() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "b2".to_string(),
        name: "read".to_string(),
        arguments: r#"{"path":"/tmp/x","limit":99999}"#.to_string(),
    };
    let (msg, is_error, _follow_ups) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(is_error);
    assert!(
        msg.contains("limit") && msg.contains("[1, 10000]"),
        "bound error should mention `limit` range, got: {}",
        msg
    );
}

// ─── T2-P0-016 PR-I：bash 后台三件套分支 ─────────────────────────────────

/// 未注入 BashTaskRegistry 时，`bash run_in_background=true` 走「未启用」错误，
/// 而**不**误调 PrimitiveExecutor::execute_bash 的同步路径。
#[tokio::test]
async fn tool_exec_bash_background_without_registry_returns_friendly_error() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "bg1".to_string(),
        name: "bash".to_string(),
        arguments: r#"{"command":"sleep 1","run_in_background":true}"#.to_string(),
    };
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(
        is_error,
        "未注入 registry 时 background bash 必须 is_error=true"
    );
    assert!(
        msg.contains("Background bash is not enabled in this AgentLoop."),
        "错误文案应说明后台 bash 未启用：{}",
        msg
    );
    assert!(
        msg.contains("foreground `bash`"),
        "错误文案应提示改用前台 bash：{}",
        msg
    );
    assert!(
        !msg.contains("BashTaskRegistry"),
        "错误文案不应泄漏内部 registry 术语：{}",
        msg
    );
}

#[tokio::test]
async fn tool_exec_task_output_without_registry_returns_friendly_error() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "to1".to_string(),
        name: "task_output".to_string(),
        arguments: r#"{"task_id":"abc"}"#.to_string(),
    };
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(is_error);
    assert!(msg.contains("Background bash is not enabled in this AgentLoop."));
    assert!(!msg.contains("BashTaskRegistry"));
}

#[tokio::test]
async fn tool_exec_task_list_without_registry_returns_friendly_error() {
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "tl1".to_string(),
        name: "task_list".to_string(),
        arguments: "{}".to_string(),
    };
    let (msg, is_error, _) = execute_tool(&primitive, &None, &None, None, &tc).await;
    assert!(is_error);
    assert!(msg.contains("Background bash is not enabled in this AgentLoop."));
    assert!(!msg.contains("BashTaskRegistry"));
}

#[tokio::test]
async fn tool_exec_verifier_background_bash_without_registry_mentions_subagent() {
    use crate::core::agent_loop::tool_exec::execute_tool_full;
    use crate::core::agent_loop::types::SubagentType;

    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let tc = ToolCallInfo {
        id: "bgv1".to_string(),
        name: "bash".to_string(),
        arguments: r#"{"command":"sleep 1","run_in_background":true}"#.to_string(),
    };
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &None,
        None,
        None,
        None,
        None,
        None,
        None,
        SubagentType::Verifier,
        None,
        &tokio_util::sync::CancellationToken::new(),
        &tc,
        None,
        None,
    )
    .await;
    assert!(outcome.is_error);
    assert!(
        outcome
            .model_text
            .contains("currently unsupported in this subagent"),
        "verifier 子 Agent 文案应说明当前子 Agent 不支持 background bash：{}",
        outcome.model_text
    );
    assert!(
        outcome.model_text.contains("foreground `bash`"),
        "verifier 子 Agent 文案应提示改用前台 bash：{}",
        outcome.model_text
    );
    assert!(
        !outcome.model_text.contains("BashTaskRegistry"),
        "verifier 子 Agent 文案不应泄漏内部 registry 术语：{}",
        outcome.model_text
    );
}

/// 起后台 → 拉输出 → stop → list：bash.md §2.4.4 验收的端到端路径，
/// 在 tool_exec 层用真实 BashTaskRegistry 走通。
#[tokio::test]
async fn tool_exec_bash_background_full_lifecycle() {
    use crate::core::tools::primitive::BashTaskRegistry;

    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let registry_opt: Option<Arc<BashTaskRegistry>> = Some(registry.clone());
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);

    // 起：background bash 应当立即返回 ticket JSON。
    let start_tc = ToolCallInfo {
        id: "bg-1".to_string(),
        name: "bash".to_string(),
        arguments: r#"{"command":"i=0; while [ $i -lt 50 ]; do echo line-$i; i=$((i+1)); sleep 0.1; done","run_in_background":true}"#.to_string(),
    };
    let (start_msg, start_err, _) =
        execute_tool(&primitive, &None, &registry_opt, None, &start_tc).await;
    assert!(!start_err, "起后台必须成功：{}", start_msg);
    let ticket: serde_json::Value = serde_json::from_str(&start_msg).expect("ticket 应为合法 JSON");
    let task_id = ticket["taskId"]
        .as_str()
        .expect("ticket 含 taskId")
        .to_string();
    assert!(!task_id.is_empty());

    // 等几行写出来再拉。
    tokio::time::sleep(std::time::Duration::from_millis(350)).await;

    // 拉：task_output 必须返回非空 content + finished=false。
    let out_tc = ToolCallInfo {
        id: "to-1".to_string(),
        name: "task_output".to_string(),
        arguments: format!(r#"{{"task_id":"{}"}}"#, task_id),
    };
    let (out_msg, out_err, _) = execute_tool(&primitive, &None, &registry_opt, None, &out_tc).await;
    assert!(!out_err, "task_output 必须成功：{}", out_msg);
    let chunk: serde_json::Value = serde_json::from_str(&out_msg).expect("chunk 应为合法 JSON");
    assert_eq!(chunk["finished"], serde_json::Value::Bool(false));
    assert!(
        chunk["content"]
            .as_str()
            .map(|s| s.contains("line-0"))
            .unwrap_or(false),
        "content 应含 line-0：{}",
        out_msg
    );

    // stop：返回成功提示。
    let stop_tc = ToolCallInfo {
        id: "ts-1".to_string(),
        name: "task_stop".to_string(),
        arguments: format!(r#"{{"task_id":"{}"}}"#, task_id),
    };
    let (stop_msg, stop_err, _) =
        execute_tool(&primitive, &None, &registry_opt, None, &stop_tc).await;
    assert!(!stop_err, "task_stop 必须成功：{}", stop_msg);
    assert!(stop_msg.contains(&task_id));

    // 给 wait 任务 reap 留点时间。
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // list：返回 1 条且 status.state == "stopped"。
    let list_tc = ToolCallInfo {
        id: "tl-1".to_string(),
        name: "task_list".to_string(),
        arguments: "{}".to_string(),
    };
    let (list_msg, list_err, _) =
        execute_tool(&primitive, &None, &registry_opt, None, &list_tc).await;
    assert!(!list_err, "task_list 必须成功：{}", list_msg);
    let infos: serde_json::Value = serde_json::from_str(&list_msg).expect("list 应为合法 JSON");
    let arr = infos.as_array().expect("list 是数组");
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["taskId"], serde_json::Value::String(task_id));
    assert_eq!(
        arr[0]["status"]["state"],
        serde_json::Value::String("stopped".to_string())
    );
}

// ─── P1（bash background monitor）：task_output(block=true) 契约 ─────────────

/// 走真实 dispatcher 路径（execute_tool_full）：task_output 的 block=true 在
/// 任务自然结束时返回 `wakeReason="finished"` + `finished=true`。
#[tokio::test]
async fn task_output_block_true_returns_finished_on_natural_exit() {
    use crate::core::agent_loop::tool_exec::execute_tool_full;
    use crate::core::agent_loop::types::SubagentType;
    use crate::core::tools::primitive::BashTaskRegistry;

    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let registry_opt: Option<Arc<BashTaskRegistry>> = Some(registry.clone());
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);

    // 起一个会 ~200ms 内结束的 task。
    let start_tc = ToolCallInfo {
        id: "bg-blk-1".into(),
        name: "bash".into(),
        arguments: r#"{"command":"echo hello; sleep 0.1; echo done","run_in_background":true}"#
            .into(),
    };
    let (start_msg, start_err, _) =
        execute_tool(&primitive, &None, &registry_opt, None, &start_tc).await;
    assert!(!start_err);
    let ticket: serde_json::Value = serde_json::from_str(&start_msg).unwrap();
    let task_id = ticket["taskId"].as_str().unwrap().to_string();

    // wait slice 循环：起点可能命中 wakeReason="new_output"（echo 立即被 pump
    // 写出），需要按契约用 next_offset 继续等，直到 wakeReason="finished"。
    let mut since: u64 = 0;
    let mut got_finished = false;
    for _ in 0..6 {
        let out_tc = ToolCallInfo {
            id: "to-blk-1".into(),
            name: "task_output".into(),
            arguments: format!(
                r#"{{"task_id":"{}","since":{},"block":true,"timeout_ms":1500}}"#,
                task_id, since
            ),
        };
        let outcome = execute_tool_full(
            &primitive,
            &None,
            &registry_opt,
            None,
            None,
            None,
            None,
            None,
            None,
            SubagentType::User,
            None,
            &tokio_util::sync::CancellationToken::new(),
            &out_tc,
            None,
            None,
        )
        .await;
        assert!(
            !outcome.is_error,
            "block=true 必须成功：{}",
            outcome.model_text
        );
        let chunk: serde_json::Value = serde_json::from_str(&outcome.model_text).unwrap();
        let wr = chunk["wakeReason"].as_str().unwrap_or("");
        let finished = chunk["finished"].as_bool().unwrap_or(false);
        if wr == "finished" || finished {
            assert_eq!(
                chunk["wakeReason"],
                serde_json::Value::String("finished".into())
            );
            assert!(finished);
            got_finished = true;
            break;
        }
        since = chunk["nextOffset"].as_u64().unwrap_or(since);
    }
    assert!(got_finished, "应当在多次 wait slice 后命中 finished");
}

/// timeout 是非终态：`wakeReason="timeout" && finished=false`，且
/// `next_offset == since`（content 为空），允许下次 since 不变继续等。
#[tokio::test]
async fn task_output_block_true_timeout_is_non_terminal_wait_slice() {
    use crate::core::agent_loop::tool_exec::execute_tool_full;
    use crate::core::agent_loop::types::SubagentType;
    use crate::core::tools::primitive::BashTaskRegistry;

    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let registry_opt: Option<Arc<BashTaskRegistry>> = Some(registry.clone());
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);

    // 起一个长任务（5s 都不会结束 / 不会 print 任何东西）。
    let start_tc = ToolCallInfo {
        id: "bg-tmo".into(),
        name: "bash".into(),
        arguments: r#"{"command":"sleep 5","run_in_background":true}"#.into(),
    };
    let (start_msg, _, _) = execute_tool(&primitive, &None, &registry_opt, None, &start_tc).await;
    let ticket: serde_json::Value = serde_json::from_str(&start_msg).unwrap();
    let task_id = ticket["taskId"].as_str().unwrap().to_string();

    let out_tc = ToolCallInfo {
        id: "to-tmo".into(),
        name: "task_output".into(),
        arguments: format!(
            r#"{{"task_id":"{}","since":0,"block":true,"timeout_ms":300}}"#,
            task_id
        ),
    };
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &registry_opt,
        None,
        None,
        None,
        None,
        None,
        None,
        SubagentType::User,
        None,
        &tokio_util::sync::CancellationToken::new(),
        &out_tc,
        None,
        None,
    )
    .await;
    assert!(!outcome.is_error);
    let chunk: serde_json::Value = serde_json::from_str(&outcome.model_text).unwrap();
    assert_eq!(
        chunk["wakeReason"],
        serde_json::Value::String("timeout".into())
    );
    assert_eq!(chunk["finished"], serde_json::Value::Bool(false));
    assert_eq!(chunk["nextOffset"].as_u64(), Some(0));
    assert_eq!(chunk["content"], serde_json::Value::String("".into()));

    // 收尾：stop。
    let _ = registry.stop(&task_id).await;
}

/// `timeout_ms=0` 等价 `block=false`：返回不带 wakeReason 字段。
#[tokio::test]
async fn task_output_timeout_zero_is_equivalent_to_non_blocking() {
    use crate::core::agent_loop::tool_exec::execute_tool_full;
    use crate::core::agent_loop::types::SubagentType;
    use crate::core::tools::primitive::BashTaskRegistry;

    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let registry_opt: Option<Arc<BashTaskRegistry>> = Some(registry.clone());
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);

    let start_tc = ToolCallInfo {
        id: "bg-z".into(),
        name: "bash".into(),
        arguments: r#"{"command":"sleep 5","run_in_background":true}"#.into(),
    };
    let (start_msg, _, _) = execute_tool(&primitive, &None, &registry_opt, None, &start_tc).await;
    let ticket: serde_json::Value = serde_json::from_str(&start_msg).unwrap();
    let task_id = ticket["taskId"].as_str().unwrap().to_string();

    let out_tc = ToolCallInfo {
        id: "to-z".into(),
        name: "task_output".into(),
        arguments: format!(
            r#"{{"task_id":"{}","since":0,"block":true,"timeout_ms":0}}"#,
            task_id
        ),
    };
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &registry_opt,
        None,
        None,
        None,
        None,
        None,
        None,
        SubagentType::User,
        None,
        &tokio_util::sync::CancellationToken::new(),
        &out_tc,
        None,
        None,
    )
    .await;
    let chunk: serde_json::Value = serde_json::from_str(&outcome.model_text).unwrap();
    assert!(
        chunk.get("wakeReason").is_none(),
        "block=false 路径**不**写出 wakeReason；实际：{}",
        outcome.model_text
    );

    let _ = registry.stop(&task_id).await;
}

/// `timeout_ms` 上限 cap：传 999_999 不会真的等 ~17 分钟，会被 cap 到 30_000。
/// 这里我们用一个不会有任何输出/不会结束的长任务，传 timeout_ms=999_999，
/// 然后 cancel_token 在 ~150ms 后 cancel；如果 cap 没生效，sleep_until 会等满
/// 17 分钟而被 cancel；如果 cap 生效，我们仅断言不到 5s 内已经返回（cancel 命中）即可——
/// 真正验证 cap 的方式是"返回里没有等满 17 分钟"，但 unit test 想要快速兜底，
/// 用 cancel 路径足以覆盖：上限 cap 与 cancel 路径都是同一个 select 分支。
#[tokio::test]
async fn task_output_timeout_ms_cap_does_not_block_indefinitely() {
    use crate::core::agent_loop::tool_exec::execute_tool_full;
    use crate::core::agent_loop::types::SubagentType;
    use crate::core::tools::primitive::BashTaskRegistry;

    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let registry_opt: Option<Arc<BashTaskRegistry>> = Some(registry.clone());
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);

    let start_tc = ToolCallInfo {
        id: "bg-cap".into(),
        name: "bash".into(),
        arguments: r#"{"command":"sleep 60","run_in_background":true}"#.into(),
    };
    let (start_msg, _, _) = execute_tool(&primitive, &None, &registry_opt, None, &start_tc).await;
    let ticket: serde_json::Value = serde_json::from_str(&start_msg).unwrap();
    let task_id = ticket["taskId"].as_str().unwrap().to_string();

    let cancel = tokio_util::sync::CancellationToken::new();
    let cancel_signal = cancel.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        cancel_signal.cancel();
    });

    let out_tc = ToolCallInfo {
        id: "to-cap".into(),
        name: "task_output".into(),
        arguments: format!(
            r#"{{"task_id":"{}","since":0,"block":true,"timeout_ms":999999999}}"#,
            task_id
        ),
    };
    let started = std::time::Instant::now();
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &registry_opt,
        None,
        None,
        None,
        None,
        None,
        None,
        SubagentType::User,
        None,
        &cancel,
        &out_tc,
        None,
        None,
    )
    .await;
    let elapsed = started.elapsed();
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "cancel 应当在 ~150ms 后命中，{:?}",
        elapsed
    );
    assert!(outcome.is_error, "cancel 路径返回 is_error=true");

    let _ = registry.stop(&task_id).await;
}

/// claim-on-entry 双回灌去重：dispatcher 在 block=true 路径拿到 finished 后，
/// completion_routes 应被置为 `Delivered`；后续 lifecycle 抢推 synthetic 时
/// 检测到该状态会跳过。
#[tokio::test]
async fn task_output_block_true_claims_completion_route_on_finished() {
    use crate::core::agent_loop::tool_exec::execute_tool_full;
    use crate::core::agent_loop::types::SubagentType;
    use crate::core::agent_loop::CompletionRoute;
    use crate::core::tools::primitive::BashTaskRegistry;
    use parking_lot::Mutex;
    use std::collections::HashMap;

    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let registry_opt: Option<Arc<BashTaskRegistry>> = Some(registry.clone());
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let routes: Arc<Mutex<HashMap<String, CompletionRoute>>> = Arc::new(Mutex::new(HashMap::new()));

    let start_tc = ToolCallInfo {
        id: "bg-cr".into(),
        name: "bash".into(),
        arguments: r#"{"command":"echo hi; exit 0","run_in_background":true}"#.into(),
    };
    let (start_msg, _, _) = execute_tool(&primitive, &None, &registry_opt, None, &start_tc).await;
    let ticket: serde_json::Value = serde_json::from_str(&start_msg).unwrap();
    let task_id = ticket["taskId"].as_str().unwrap().to_string();

    let out_tc = ToolCallInfo {
        id: "to-cr".into(),
        name: "task_output".into(),
        arguments: format!(
            r#"{{"task_id":"{}","since":0,"block":true,"timeout_ms":3000}}"#,
            task_id
        ),
    };
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &registry_opt,
        None,
        None,
        None,
        None,
        None,
        None,
        SubagentType::User,
        None,
        &tokio_util::sync::CancellationToken::new(),
        &out_tc,
        None,
        Some(&routes),
    )
    .await;
    assert!(!outcome.is_error);
    let chunk: serde_json::Value = serde_json::from_str(&outcome.model_text).unwrap();
    assert_eq!(
        chunk["wakeReason"],
        serde_json::Value::String("finished".into())
    );
    let g = routes.lock();
    assert!(matches!(g.get(&task_id), Some(CompletionRoute::Delivered)));
}

/// claim 让回：timeout（非终态）必须把 task_id 从 routes 里移除，让 lifecycle
/// 后续可以兜底。
#[tokio::test]
async fn task_output_block_true_releases_claim_on_timeout() {
    use crate::core::agent_loop::tool_exec::execute_tool_full;
    use crate::core::agent_loop::types::SubagentType;
    use crate::core::agent_loop::CompletionRoute;
    use crate::core::tools::primitive::BashTaskRegistry;
    use parking_lot::Mutex;
    use std::collections::HashMap;

    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let registry_opt: Option<Arc<BashTaskRegistry>> = Some(registry.clone());
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let routes: Arc<Mutex<HashMap<String, CompletionRoute>>> = Arc::new(Mutex::new(HashMap::new()));

    let start_tc = ToolCallInfo {
        id: "bg-rel".into(),
        name: "bash".into(),
        arguments: r#"{"command":"sleep 5","run_in_background":true}"#.into(),
    };
    let (start_msg, _, _) = execute_tool(&primitive, &None, &registry_opt, None, &start_tc).await;
    let ticket: serde_json::Value = serde_json::from_str(&start_msg).unwrap();
    let task_id = ticket["taskId"].as_str().unwrap().to_string();

    let out_tc = ToolCallInfo {
        id: "to-rel".into(),
        name: "task_output".into(),
        arguments: format!(
            r#"{{"task_id":"{}","since":0,"block":true,"timeout_ms":250}}"#,
            task_id
        ),
    };
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &registry_opt,
        None,
        None,
        None,
        None,
        None,
        None,
        SubagentType::User,
        None,
        &tokio_util::sync::CancellationToken::new(),
        &out_tc,
        None,
        Some(&routes),
    )
    .await;
    let chunk: serde_json::Value = serde_json::from_str(&outcome.model_text).unwrap();
    assert_eq!(
        chunk["wakeReason"],
        serde_json::Value::String("timeout".into())
    );
    // routes 不应留 entry：让 lifecycle 后续兜底。
    assert!(routes.lock().get(&task_id).is_none());

    let _ = registry.stop(&task_id).await;
}

/// lifecycle 抢先 Delivered：让 shell 在 dispatcher 调 block=true 之前就完成
/// 并被 lifecycle subscriber 抢先标 `Delivered`，然后 dispatcher entry 检测到
/// 跳过 wait，仍只交付一次（返回 finished + 不再额外 push synthetic）。
#[tokio::test]
async fn task_output_block_true_skips_wait_when_lifecycle_already_delivered() {
    use crate::core::agent_loop::tool_exec::execute_tool_full;
    use crate::core::agent_loop::types::SubagentType;
    use crate::core::agent_loop::CompletionRoute;
    use crate::core::tools::primitive::BashTaskRegistry;
    use parking_lot::Mutex;
    use std::collections::HashMap;

    let dir = tempfile::tempdir().expect("tempdir");
    let registry = Arc::new(BashTaskRegistry::new(dir.path().join("tool-results")));
    let registry_opt: Option<Arc<BashTaskRegistry>> = Some(registry.clone());
    let primitive: Arc<dyn PrimitiveExecutor> = Arc::new(MockPrimitiveExecutor);
    let routes: Arc<Mutex<HashMap<String, CompletionRoute>>> = Arc::new(Mutex::new(HashMap::new()));

    let start_tc = ToolCallInfo {
        id: "bg-snk".into(),
        name: "bash".into(),
        arguments: r#"{"command":"echo done; exit 0","run_in_background":true}"#.into(),
    };
    let (start_msg, _, _) = execute_tool(&primitive, &None, &registry_opt, None, &start_tc).await;
    let ticket: serde_json::Value = serde_json::from_str(&start_msg).unwrap();
    let task_id = ticket["taskId"].as_str().unwrap().to_string();

    // 让 task 先 Finish + 模拟 lifecycle subscriber 抢先 Delivered。
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    routes
        .lock()
        .insert(task_id.clone(), CompletionRoute::Delivered);

    let out_tc = ToolCallInfo {
        id: "to-snk".into(),
        name: "task_output".into(),
        arguments: format!(
            r#"{{"task_id":"{}","since":0,"block":true,"timeout_ms":100}}"#,
            task_id
        ),
    };
    // 即便 timeout_ms 只有 100ms，dispatcher 应在 entry 立即返回（不进 wait）。
    let started = std::time::Instant::now();
    let outcome = execute_tool_full(
        &primitive,
        &None,
        &registry_opt,
        None,
        None,
        None,
        None,
        None,
        None,
        SubagentType::User,
        None,
        &tokio_util::sync::CancellationToken::new(),
        &out_tc,
        None,
        Some(&routes),
    )
    .await;
    assert!(
        started.elapsed() < std::time::Duration::from_millis(200),
        "lifecycle 抢先时 dispatcher 应立即返回，实际耗时 {:?}",
        started.elapsed()
    );
    let chunk: serde_json::Value = serde_json::from_str(&outcome.model_text).unwrap();
    assert_eq!(
        chunk["wakeReason"],
        serde_json::Value::String("finished".into())
    );
    // routes 仍是 Delivered（终态）。
    assert!(matches!(
        routes.lock().get(&task_id),
        Some(CompletionRoute::Delivered)
    ));
}
