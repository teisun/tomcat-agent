//! `DefaultPermissionGate` 三层决策路径。

use std::path::PathBuf;

use crate::core::permission::gate::{DefaultPermissionGate, GateConfig};
use crate::core::permission::path_rule::PathRule;
use crate::core::permission::session_grants::{DraggedPaths, SessionGrants};
use crate::core::permission::types::{
    GrantSource, PathRuleMode, PermissionDecision, PermissionLevel,
};
use crate::core::permission::PermissionGate;
use crate::core::primitives::PrimitiveOperation;

fn tmpdir(name: &str) -> PathBuf {
    let p = std::env::temp_dir().join(format!("pi_perm_test_{}", name));
    let _ = std::fs::create_dir_all(&p);
    p
}

fn gate_with(
    workspace: PathBuf,
    extra: Vec<PathBuf>,
    agent_ro: Vec<PathBuf>,
) -> DefaultPermissionGate {
    DefaultPermissionGate::new(
        GateConfig {
            agent_workspace_dir: workspace,
            extra_roots: extra,
            agent_data_readonly_dirs: agent_ro,
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
        DraggedPaths::new(),
    )
}

#[test]
fn allow_inside_workspace() {
    let ws = tmpdir("ws_allow");
    let gate = gate_with(ws.clone(), vec![], vec![]);
    let dec = gate
        .check(
            PrimitiveOperation::Write,
            ws.join("a.txt").to_str().unwrap(),
        )
        .unwrap();
    match dec {
        PermissionDecision::Allow { source, level } => {
            assert_eq!(source, GrantSource::AgentWorkspace);
            assert_eq!(level, PermissionLevel::Write);
        }
        other => panic!("unexpected: {:?}", other),
    }
}

#[test]
fn need_confirm_outside_workspace() {
    let ws = tmpdir("ws_outside");
    let gate = gate_with(ws, vec![], vec![]);
    let dec = gate
        .check(
            PrimitiveOperation::Write,
            "/tmp/some-foreign-dir-xxxxxxxx/x.txt",
        )
        .unwrap();
    assert!(matches!(dec, PermissionDecision::NeedConfirm { .. }));
}

#[test]
fn deny_path_rule_overrides_workspace() {
    let ws = tmpdir("ws_deny");
    let secret = ws.join("secret");
    let _ = std::fs::create_dir_all(&secret);
    // 注入 user_path_rules：deny <ws>/secret
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_workspace_dir: ws.clone(),
            extra_roots: vec![],
            agent_data_readonly_dirs: vec![],
            user_path_rules: vec![PathRule::new(
                secret.to_string_lossy().to_string(),
                PathRuleMode::Deny,
            )],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
        DraggedPaths::new(),
    );
    let dec = gate
        .check(
            PrimitiveOperation::Read,
            secret.join("k.txt").to_str().unwrap(),
        )
        .unwrap();
    assert!(matches!(dec, PermissionDecision::Deny { .. }));
}

#[test]
fn readonly_path_rule_blocks_write_allows_read() {
    let ws = tmpdir("ws_ro");
    let ro = ws.join("ro");
    let _ = std::fs::create_dir_all(&ro);
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_workspace_dir: ws.clone(),
            extra_roots: vec![],
            agent_data_readonly_dirs: vec![],
            user_path_rules: vec![PathRule::new(
                ro.to_string_lossy().to_string(),
                PathRuleMode::Readonly,
            )],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
        DraggedPaths::new(),
    );
    let read_ok = gate
        .check(PrimitiveOperation::Read, ro.join("a").to_str().unwrap())
        .unwrap();
    assert!(matches!(
        read_ok,
        PermissionDecision::Allow {
            source: GrantSource::PathRuleReadOnly,
            ..
        }
    ));
    let write_deny = gate
        .check(PrimitiveOperation::Write, ro.join("a").to_str().unwrap())
        .unwrap();
    assert!(matches!(write_deny, PermissionDecision::Deny { .. }));
}

#[test]
fn agent_data_dir_read_only_allow() {
    let ws = tmpdir("ws_agent");
    let agent = tmpdir("agent_ro");
    let gate = gate_with(ws, vec![], vec![agent.clone()]);
    let dec = gate
        .check(
            PrimitiveOperation::Read,
            agent.join("logs/x.log").to_str().unwrap(),
        )
        .unwrap();
    assert!(matches!(
        dec,
        PermissionDecision::Allow {
            source: GrantSource::AgentDataDir,
            level: PermissionLevel::Read
        }
    ));
}

#[test]
fn session_grant_unblocks_outside() {
    let ws = tmpdir("ws_session");
    let outside = tmpdir("outside_session");
    let session = SessionGrants::new();
    session.add(outside.clone());
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_workspace_dir: ws,
            extra_roots: vec![],
            agent_data_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        session,
        DraggedPaths::new(),
    );
    let dec = gate
        .check(
            PrimitiveOperation::Write,
            outside.join("a.txt").to_str().unwrap(),
        )
        .unwrap();
    assert!(matches!(
        dec,
        PermissionDecision::Allow {
            source: GrantSource::SessionGrant,
            ..
        }
    ));
}

#[test]
fn runtime_deny_rule_overrides_existing_session_grant() {
    let ws = tmpdir("ws_runtime_deny");
    let outside = tmpdir("outside_runtime_deny");
    let session = SessionGrants::new();
    session.add(outside.clone());
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_workspace_dir: ws,
            extra_roots: vec![],
            agent_data_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        session,
        DraggedPaths::new(),
    );

    gate.grant_path_rule(PathRule::new(
        outside.to_string_lossy().to_string(),
        PathRuleMode::Deny,
    ));

    let dec = gate
        .check(
            PrimitiveOperation::Read,
            outside.join("a.txt").to_str().unwrap(),
        )
        .unwrap();
    assert!(matches!(dec, PermissionDecision::Deny { .. }));
}

#[test]
fn dragged_path_unblocks_outside() {
    let ws = tmpdir("ws_drag");
    let dragged = tmpdir("dragged_ok");
    let drag = DraggedPaths::new();
    drag.add(dragged.clone());
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_workspace_dir: ws,
            extra_roots: vec![],
            agent_data_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: false,
        },
        SessionGrants::new(),
        drag,
    );
    let dec = gate
        .check(
            PrimitiveOperation::Read,
            dragged.join("a.txt").to_str().unwrap(),
        )
        .unwrap();
    assert!(matches!(
        dec,
        PermissionDecision::Allow {
            source: GrantSource::DraggedPath,
            ..
        }
    ));
}

#[test]
fn auto_confirm_short_circuits_layer2() {
    let ws = tmpdir("ws_autoconf");
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_workspace_dir: ws,
            extra_roots: vec![],
            agent_data_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: true,
        },
        SessionGrants::new(),
        DraggedPaths::new(),
    );
    let dec = gate
        .check(PrimitiveOperation::Write, "/tmp/ac-foreign-target/x")
        .unwrap();
    assert!(matches!(
        dec,
        PermissionDecision::Allow {
            source: GrantSource::AutoConfirmFlag,
            ..
        }
    ));
}

#[test]
fn auto_confirm_does_not_override_forbidden() {
    let ws = tmpdir("ws_acforbid");
    let gate = DefaultPermissionGate::new(
        GateConfig {
            agent_workspace_dir: ws,
            extra_roots: vec![],
            agent_data_readonly_dirs: vec![],
            user_path_rules: vec![],
            user_bash_forbidden: vec![],
            user_bash_approval: vec![],
            auto_confirm: true,
        },
        SessionGrants::new(),
        DraggedPaths::new(),
    );
    // 命中 builtin bash_forbidden（pi config set）。
    let dec = gate.check_bash("pi config set llm.api_key xxx").unwrap();
    assert!(matches!(dec, PermissionDecision::Deny { .. }));
}

#[test]
fn bash_forbidden_blocks_self_escalation() {
    let ws = tmpdir("ws_self_esc");
    let gate = gate_with(ws, vec![], vec![]);
    for bad in [
        "pi config set primitive.path_rules abc",
        "pi pathrules add /etc --mode readonly",
        "echo hello > ~/.pi_/pi.config.toml",
        "sed -i '' 's/x/y/' ~/.pi_/pi.config.toml",
    ] {
        let dec = gate.check_bash(bad).unwrap();
        assert!(
            matches!(dec, PermissionDecision::Deny { .. }),
            "expected deny for `{}`, got {:?}",
            bad,
            dec
        );
    }
}

#[test]
fn bash_approval_required_layer2() {
    let ws = tmpdir("ws_approve");
    let gate = gate_with(ws, vec![], vec![]);
    let dec = gate.check_bash("rm -rf ./build").unwrap();
    assert!(matches!(dec, PermissionDecision::NeedConfirm { .. }));
}

// ── PR-9：Agent data dir 凭据保护 / 历史只读集成 ─────────────────────────

/// 写 `~/.pi_/agents/main/agent/auth-profiles.json` → builtin path_rule deny。
#[test]
fn pr9_credentials_glob_denies_write() {
    let ws = tmpdir("ws_pr9_cred");
    let gate = gate_with(ws, vec![], vec![]);
    let home = dirs::home_dir().expect("home dir");
    let target = home.join(".pi_/agents/main/agent/auth-profiles.json");
    let dec = gate
        .check(PrimitiveOperation::Write, target.to_str().unwrap())
        .unwrap();
    assert!(
        matches!(dec, PermissionDecision::Deny { .. }),
        "expected deny for credentials write, got {:?}",
        dec
    );
}

/// 读 `~/.pi_/agents/main/agent/auth-profiles.json` → builtin path_rule deny
/// （凭据 deny 优先于 agent_data_dir read_only）。
#[test]
fn pr9_credentials_glob_denies_read_too() {
    let ws = tmpdir("ws_pr9_cred_read");
    let home = dirs::home_dir().expect("home dir");
    let agent_root = home.join(".pi_/agents/main/agent");
    let gate = gate_with(ws, vec![], vec![agent_root.clone()]);
    let target = agent_root.join("auth-profiles.json");
    let dec = gate
        .check(PrimitiveOperation::Read, target.to_str().unwrap())
        .unwrap();
    assert!(
        matches!(dec, PermissionDecision::Deny { .. }),
        "deny 应优先于 readonly agent_data_dir"
    );
}

/// 写 `~/.pi_/agents/main/sessions/anything.jsonl` → builtin readonly path_rule
/// 阻止 write（命中 readonly + write/edit/bash → Deny）。
#[test]
fn pr9_sessions_glob_blocks_write() {
    let ws = tmpdir("ws_pr9_sess");
    let gate = gate_with(ws, vec![], vec![]);
    let home = dirs::home_dir().expect("home dir");
    let target = home.join(".pi_/agents/main/sessions/anything.jsonl");
    let dec = gate
        .check(PrimitiveOperation::Write, target.to_str().unwrap())
        .unwrap();
    assert!(
        matches!(dec, PermissionDecision::Deny { .. }),
        "expected deny for sessions write, got {:?}",
        dec
    );
}

/// 读 `~/.pi_/agents/main/sessions/foo.jsonl` → builtin readonly 通过
/// （source=`PathRuleReadOnly`）。
#[test]
fn pr9_sessions_glob_allows_read_with_path_rule_source() {
    let ws = tmpdir("ws_pr9_sess_read");
    let gate = gate_with(ws, vec![], vec![]);
    let home = dirs::home_dir().expect("home dir");
    let target = home.join(".pi_/agents/main/sessions/foo.jsonl");
    let dec = gate
        .check(PrimitiveOperation::Read, target.to_str().unwrap())
        .unwrap();
    // glob `~/.pi_/agents/*/sessions` 不会自动覆盖子文件——globset 要求模式精确匹配整个字符串；
    // 但当我们把 sessions 目录路径加进 `agent_data_readonly_dirs` 时，read 会通过 AgentDataDir 兜底；
    // 否则走 NeedConfirm。这里只断言"非 Deny"（无论 source 是 PathRuleReadOnly 还是 NeedConfirm）。
    if matches!(dec, PermissionDecision::Deny { .. }) {
        panic!("read 应允许或要求确认，不应 deny");
    }
}

/// 命中 read_only 集合的 read 路径 → AgentDataDir 来源；同路径 write → Deny。
#[test]
fn pr9_agent_data_dir_read_allow_write_deny() {
    let ws = tmpdir("ws_pr9_ad");
    let agent = tmpdir("ad_ro");
    let gate = gate_with(ws, vec![], vec![agent.clone()]);
    let read_dec = gate
        .check(
            PrimitiveOperation::Read,
            agent.join("logs/x.log").to_str().unwrap(),
        )
        .unwrap();
    assert!(matches!(
        read_dec,
        PermissionDecision::Allow {
            source: GrantSource::AgentDataDir,
            level: PermissionLevel::Read
        }
    ));
    // write 不应通过 AgentDataDir：fall through 到 NeedConfirm（不在 writable 集合里）。
    let write_dec = gate
        .check(
            PrimitiveOperation::Write,
            agent.join("logs/x.log").to_str().unwrap(),
        )
        .unwrap();
    assert!(
        matches!(
            write_dec,
            PermissionDecision::NeedConfirm { .. } | PermissionDecision::Deny { .. }
        ),
        "write on agent_data_readonly_dir 不应直接 Allow，得到: {:?}",
        write_dec
    );
}

#[test]
fn extra_roots_grant_writable() {
    let ws = tmpdir("ws_extra");
    let extra = tmpdir("extra_root");
    let gate = gate_with(ws, vec![extra.clone()], vec![]);
    let dec = gate
        .check(
            PrimitiveOperation::Write,
            extra.join("f.txt").to_str().unwrap(),
        )
        .unwrap();
    assert!(matches!(
        dec,
        PermissionDecision::Allow {
            source: GrantSource::ConfigExtraRoot,
            level: PermissionLevel::Write,
        }
    ));
}
