//! Executor + PermissionGate 集成测试（PR-2）。
//!
//! 覆盖：
//! - gate Allow 直通（read / write）
//! - gate Deny 拦截（path_rules）
//! - gate NeedConfirm + AllowOnce 经 confirm 落 SessionGrants 后放行
//! - gate NeedConfirm + Deny 阻止后审计 user_approved=false
//! - gate bash forbidden 命中拒绝
//! - gate bash approval_required 命中弹 confirm

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::core::confirmation::{ConfirmDecision, UserConfirmationProvider};
use crate::core::permission::{
    DefaultPermissionGate, GateConfig, PathRule, PathRuleMode, SessionGrants,
};
use crate::core::primitives::{PrimitiveExecutor, PrimitiveOperation};
use crate::core::{AllowAllConfirmation, DefaultPrimitiveExecutor};
use crate::infra::error::AppError;
use crate::infra::{PrimitiveConfig, TracingAuditRecorder};

/// 可控 confirm provider：每次 confirm_decision 返回预设值。
#[derive(Debug, Default)]
struct ProgrammableConfirm {
    answers: Mutex<Vec<ConfirmDecision>>,
}

impl ProgrammableConfirm {
    fn new(decisions: Vec<ConfirmDecision>) -> Self {
        Self {
            answers: Mutex::new(decisions),
        }
    }
}

#[async_trait]
impl UserConfirmationProvider for ProgrammableConfirm {
    async fn confirm(
        &self,
        _operation: PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
    ) -> Result<bool, AppError> {
        // 仅给老接口一个备用兜底（不应该走到）。
        Ok(true)
    }

    async fn confirm_decision(
        &self,
        _operation: PrimitiveOperation,
        _preview: &str,
        _plugin_id: &str,
        _suggested_root: Option<PathBuf>,
    ) -> Result<ConfirmDecision, AppError> {
        let mut q = self.answers.lock().unwrap();
        if q.is_empty() {
            return Ok(ConfirmDecision::Deny);
        }
        Ok(q.remove(0))
    }
}

fn workspace_dir(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("pi_wasm_gate_{}", name));
    std::fs::create_dir_all(&p).unwrap();
    p
}

fn make_executor(
    agent_definition_dir: PathBuf,
    user_path_rules: Vec<PathRule>,
    confirm: Arc<dyn UserConfirmationProvider>,
) -> DefaultPrimitiveExecutor {
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir,
            workspace_roots: vec![],
            agent_trail_readonly_dirs: vec![],
            user_path_rules,
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    );
    DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        confirm,
        Arc::new(TracingAuditRecorder),
        gate.into_arc(),
    )
}

#[tokio::test]
async fn gate_allow_inside_workspace_write() {
    let ws = workspace_dir("write_inside");
    let exec = make_executor(ws.clone(), vec![], Arc::new(AllowAllConfirmation));
    let p = ws.join("a.txt");
    let res = exec
        .write_file(&p.to_string_lossy(), "hi", false, "p1")
        .await
        .unwrap();
    assert!(res.written);
    let _ = std::fs::remove_file(&p);
    let _ = std::fs::remove_dir(&ws);
}

#[tokio::test]
async fn gate_deny_path_rule_blocks_write() {
    let ws = workspace_dir("deny_rule");
    let secret = ws.join("secret");
    std::fs::create_dir_all(&secret).unwrap();
    let exec = make_executor(
        ws.clone(),
        vec![PathRule::new(
            secret.to_string_lossy().to_string(),
            PathRuleMode::Deny,
        )],
        Arc::new(AllowAllConfirmation),
    );
    let p = secret.join("k.txt");
    let r = exec
        .write_file(&p.to_string_lossy(), "x", false, "p1")
        .await;
    assert!(matches!(r, Err(AppError::Permission(_))));
    let _ = std::fs::remove_dir_all(&ws);
}

#[tokio::test]
async fn gate_need_confirm_allow_once_succeeds() {
    let ws = workspace_dir("confirm_allow");
    let outside = workspace_dir("confirm_outside");
    let exec = make_executor(
        ws,
        vec![],
        Arc::new(ProgrammableConfirm::new(vec![ConfirmDecision::AllowOnce])),
    );
    let target = outside.join("o.txt");
    let res = exec
        .write_file(&target.to_string_lossy(), "ok", false, "p1")
        .await
        .unwrap();
    assert!(res.written);
    let _ = std::fs::remove_file(&target);
    let _ = std::fs::remove_dir(&outside);
}

#[tokio::test]
async fn gate_need_confirm_deny_blocks() {
    let ws = workspace_dir("confirm_deny_ws");
    let outside = workspace_dir("confirm_deny_outside");
    let exec = make_executor(
        ws,
        vec![],
        Arc::new(ProgrammableConfirm::new(vec![ConfirmDecision::Deny])),
    );
    let target = outside.join("o.txt");
    let r = exec
        .write_file(&target.to_string_lossy(), "ok", false, "p1")
        .await;
    assert!(matches!(r, Err(AppError::Permission(_))));
    let _ = std::fs::remove_dir_all(&outside);
}

#[tokio::test]
async fn gate_bash_forbidden_blocks() {
    let ws = workspace_dir("bash_forbid");
    let exec = make_executor(ws.clone(), vec![], Arc::new(AllowAllConfirmation));
    let r = exec
        .execute_bash("pi config set llm.api_key xxx", None, "p1", None)
        .await;
    assert!(matches!(r, Err(AppError::Permission(_))));
    let _ = std::fs::remove_dir(&ws);
}

// ── PR-9：Agent trail dir read-only / 凭据 deny 经 executor 落地 ──

fn make_executor_with_agent_ro(
    agent_definition_dir: PathBuf,
    agent_ro: Vec<PathBuf>,
    confirm: Arc<dyn UserConfirmationProvider>,
) -> DefaultPrimitiveExecutor {
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_definition_dir,
            workspace_roots: vec![],
            agent_trail_readonly_dirs: agent_ro,
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
    );
    DefaultPrimitiveExecutor::new(
        PrimitiveConfig::default(),
        confirm,
        Arc::new(TracingAuditRecorder),
        gate.into_arc(),
    )
}

#[tokio::test]
async fn pr9_executor_reads_agent_trail_dir_succeeds() {
    let ws = workspace_dir("pr9_ad_read_ws");
    let agent_ro = workspace_dir("pr9_ad_read_ro");
    let f = agent_ro.join("hello.log");
    std::fs::write(&f, b"line1").unwrap();
    let exec = make_executor_with_agent_ro(
        ws.clone(),
        vec![agent_ro.clone()],
        Arc::new(AllowAllConfirmation),
    );
    let res = exec
        .read_file(&f.to_string_lossy(), "p1")
        .await
        .expect("read should pass via AgentTrailDir");
    assert!(res.contains("line1"));
    let _ = std::fs::remove_file(&f);
    let _ = std::fs::remove_dir(&agent_ro);
    let _ = std::fs::remove_dir(&ws);
}

#[tokio::test]
async fn pr9_executor_writes_agent_trail_dir_blocked_or_confirms() {
    let ws = workspace_dir("pr9_ad_write_ws");
    let agent_ro = workspace_dir("pr9_ad_write_ro");
    let f = agent_ro.join("blocked.log");
    // confirm 选 Deny —— write 必须失败（gate 不会 Allow agent_trail_readonly_dirs 的 write）。
    let exec = make_executor_with_agent_ro(
        ws.clone(),
        vec![agent_ro.clone()],
        Arc::new(ProgrammableConfirm::new(vec![ConfirmDecision::Deny])),
    );
    let r = exec
        .write_file(&f.to_string_lossy(), "x", false, "p1")
        .await;
    assert!(
        matches!(r, Err(AppError::Permission(_))),
        "write 在 agent trail dir 上应受 confirm/deny 控制，得到: {:?}",
        r
    );
    let _ = std::fs::remove_dir(&agent_ro);
    let _ = std::fs::remove_dir(&ws);
}

#[tokio::test]
async fn pr9_executor_credentials_glob_denies_write() {
    let ws = workspace_dir("pr9_creds_ws");
    // builtin path_rules 自动加载 → 不需要 user_path_rules。
    let exec = make_executor(ws.clone(), vec![], Arc::new(AllowAllConfirmation));
    let home = dirs::home_dir().expect("home");
    let target = home.join(".pi_/agents/main/agent/auth-profiles.json");
    let r = exec
        .write_file(&target.to_string_lossy(), "secret", false, "p1")
        .await;
    assert!(
        matches!(r, Err(AppError::Permission(_))),
        "凭据写入应被 builtin path_rule deny，得到: {:?}",
        r
    );
    let _ = std::fs::remove_dir(&ws);
}

#[tokio::test]
async fn pr9_executor_sessions_glob_denies_write() {
    let ws = workspace_dir("pr9_sess_ws");
    let exec = make_executor(ws.clone(), vec![], Arc::new(AllowAllConfirmation));
    let home = dirs::home_dir().expect("home");
    let target = home.join(".pi_/agents/main/sessions/anything.jsonl");
    let r = exec
        .write_file(&target.to_string_lossy(), "x", false, "p1")
        .await;
    assert!(
        matches!(r, Err(AppError::Permission(_))),
        "sessions 写入应被 builtin readonly path_rule 阻止，得到: {:?}",
        r
    );
    let _ = std::fs::remove_dir(&ws);
}

#[tokio::test]
async fn gate_bash_approval_allow_once() {
    let ws = workspace_dir("bash_approve_allow");
    let target = ws.join("zzz");
    // bash 命中 approval_required → 1 次 confirm；
    // 路径解析得到 "<ws>/zzz" → 在 workspace 内 → 不再 confirm。
    let exec = make_executor(
        ws.clone(),
        vec![],
        Arc::new(ProgrammableConfirm::new(vec![ConfirmDecision::AllowOnce])),
    );
    let cmd = format!("rm -rf {}", target.display());
    let r = exec.execute_bash(&cmd, None, "p1", None).await;
    if let Err(AppError::Permission(msg)) = r {
        panic!("permission denied unexpectedly: {}", msg)
    }
    let _ = std::fs::remove_dir(&ws);
}
